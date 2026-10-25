//! Postgres implementation of the directory: organizations, users,
//! memberships, teams, resource owners and sign-ins.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::{PostgresStore, json, now, parse};
use crate::*;

fn org(r: &PgRow) -> Result<Org> {
    Ok(Org {
        id: r.try_get("id")?,
        slug: r.try_get("slug")?,
        name: r.try_get("name")?,
        settings: parse(&r.try_get::<String, _>("settings")?)?,
        created_at: r.try_get("created_at")?,
    })
}

fn user(r: &PgRow) -> Result<User> {
    Ok(User {
        id: r.try_get("id")?,
        email: r.try_get("email")?,
        name: r.try_get("name")?,
        subject: r.try_get("subject")?,
        disabled: r.try_get("disabled")?,
        created_at: r.try_get("created_at")?,
        last_seen_at: r.try_get("last_seen_at")?,
    })
}

fn role(s: &str) -> Result<Role> {
    s.parse().map_err(StoreError::Corrupt)
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Viewer => "viewer",
        Role::Member => "member",
        Role::Builder => "builder",
        Role::Admin => "admin",
        Role::Owner => "owner",
    }
}

fn membership(r: &PgRow) -> Result<Membership> {
    Ok(Membership {
        org_id: r.try_get("org_id")?,
        org_slug: r.try_get("org_slug")?,
        org_name: r.try_get("org_name")?,
        user_id: r.try_get("user_id")?,
        email: r.try_get("email")?,
        name: r.try_get("name")?,
        role: role(&r.try_get::<String, _>("role")?)?,
        created_at: r.try_get("created_at")?,
    })
}

fn unique(e: sqlx::Error, what: String) -> StoreError {
    match e.as_database_error() {
        Some(d) if d.is_unique_violation() => StoreError::Conflict(what),
        _ => e.into(),
    }
}

const MEMBERSHIPS: &str = "SELECT m.org_id, o.slug AS org_slug, o.name AS org_name, m.user_id, u.email, u.name, m.role, m.created_at
     FROM memberships m JOIN orgs o ON o.id = m.org_id JOIN users u ON u.id = m.user_id";

impl PostgresStore {
    async fn teams_where(&self, org_id: &str, team: Option<&str>) -> Result<Vec<Team>> {
        let rows = sqlx::query(
            "SELECT * FROM teams WHERE org_id = $1 AND ($2::TEXT IS NULL OR id = $2 OR name = $2) ORDER BY name",
        )
        .bind(org_id)
        .bind(team)
        .fetch_all(&self.pool)
        .await?;
        let members = sqlx::query(
            "SELECT tm.team_id, tm.user_id FROM team_members tm JOIN teams t ON t.id = tm.team_id
             WHERE t.org_id = $1 ORDER BY tm.user_id",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                let id: String = r.try_get("id")?;
                let mut m = Vec::new();
                for row in &members {
                    if row.try_get::<String, _>("team_id")? == id {
                        m.push(row.try_get("user_id")?);
                    }
                }
                Ok(Team {
                    org_id: r.try_get("org_id")?,
                    name: r.try_get("name")?,
                    created_at: r.try_get("created_at")?,
                    members: m,
                    id,
                })
            })
            .collect()
    }
}

#[async_trait]
impl DirectoryStore for PostgresStore {
    async fn create_org(&self, slug: &str, name: &str) -> Result<Org> {
        let id = gai_core::new_id("org");
        sqlx::query("INSERT INTO orgs (id, slug, name, created_at) VALUES ($1, $2, $3, $4)")
            .bind(&id)
            .bind(slug)
            .bind(name)
            .bind(now())
            .execute(&self.pool)
            .await
            .map_err(|e| unique(e, format!("an organization {slug} exists")))?;
        self.get_org(&id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("organization {slug}")))
    }

    async fn get_org(&self, id_or_slug: &str) -> Result<Option<Org>> {
        let row = sqlx::query("SELECT * FROM orgs WHERE id = $1 OR slug = $1")
            .bind(id_or_slug)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(org).transpose()
    }

    async fn list_orgs(&self) -> Result<Vec<Org>> {
        let rows = sqlx::query("SELECT * FROM orgs ORDER BY created_at, slug")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(org).collect()
    }

    async fn update_org(
        &self,
        id: &str,
        name: Option<&str>,
        settings: Option<&Value>,
    ) -> Result<Org> {
        let r = sqlx::query(
            "UPDATE orgs SET name = COALESCE($1::TEXT, name), settings = COALESCE($2::TEXT, settings) WHERE id = $3",
        )
        .bind(name)
        .bind(settings.map(json))
        .bind(id)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("organization {id}")));
        }
        self.get_org(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("organization {id}")))
    }

    async fn delete_org(&self, id: &str) -> Result<bool> {
        if id == DEFAULT_ORG {
            return Err(StoreError::Conflict(
                "the default organization can't be deleted".into(),
            ));
        }
        let mut tx = self.write_tx().await?;
        sqlx::query("DELETE FROM api_keys WHERE org_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let r = sqlx::query("DELETE FROM orgs WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }

    async fn create_user(&self, email: &str, name: Option<&str>) -> Result<User> {
        let email = email.trim().to_lowercase();
        if let Some(u) = self.find_user_by_email(&email).await? {
            return Ok(u);
        }
        let id = gai_core::new_id("usr");
        sqlx::query("INSERT INTO users (id, email, name, created_at) VALUES ($1, $2, $3, $4)")
            .bind(&id)
            .bind(&email)
            .bind(name)
            .bind(now())
            .execute(&self.pool)
            .await
            .map_err(|e| unique(e, format!("a user {email} exists")))?;
        self.get_user(&id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("user {email}")))
    }

    async fn sign_in_user(&self, subject: &str, email: &str, name: Option<&str>) -> Result<User> {
        let email = email.trim().to_lowercase();
        let mut tx = self.write_tx().await?;
        let ts = now();
        let known: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE subject = $1")
            .bind(subject)
            .fetch_optional(&mut *tx)
            .await?;
        let id = match known {
            Some(id) => {
                sqlx::query("UPDATE users SET name = COALESCE($1::TEXT, name), last_seen_at = $2 WHERE id = $3")
                    .bind(name)
                    .bind(ts)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
                id
            }
            None => {
                let row = sqlx::query("SELECT id, subject FROM users WHERE lower(email) = $1")
                    .bind(&email)
                    .fetch_optional(&mut *tx)
                    .await?;
                match row {
                    Some(r) => {
                        if r.try_get::<Option<String>, _>("subject")?.is_some() {
                            return Err(StoreError::Conflict(format!(
                                "{email} already signs in with another account"
                            )));
                        }
                        let id: String = r.try_get("id")?;
                        sqlx::query(
                            "UPDATE users SET subject = $1, name = COALESCE(name, $2::TEXT), last_seen_at = $3 WHERE id = $4",
                        )
                        .bind(subject)
                        .bind(name)
                        .bind(ts)
                        .bind(&id)
                        .execute(&mut *tx)
                        .await?;
                        id
                    }
                    None => {
                        let id = gai_core::new_id("usr");
                        sqlx::query(
                            "INSERT INTO users (id, email, name, subject, created_at, last_seen_at) VALUES ($1, $2, $3, $4, $5, $6)",
                        )
                        .bind(&id)
                        .bind(&email)
                        .bind(name)
                        .bind(subject)
                        .bind(ts)
                        .bind(ts)
                        .execute(&mut *tx)
                        .await?;
                        id
                    }
                }
            }
        };
        tx.commit().await?;
        self.get_user(&id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("user {id}")))
    }

    async fn get_user(&self, id: &str) -> Result<Option<User>> {
        let row = sqlx::query("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(user).transpose()
    }

    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let row = sqlx::query("SELECT * FROM users WHERE lower(email) = $1")
            .bind(email.trim().to_lowercase())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(user).transpose()
    }

    async fn list_users(&self) -> Result<Vec<User>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY email")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(user).collect()
    }

    async fn set_user_disabled(&self, id: &str, disabled: bool) -> Result<bool> {
        let mut tx = self.write_tx().await?;
        let r = sqlx::query("UPDATE users SET disabled = $1 WHERE id = $2")
            .bind(disabled)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if disabled {
            sqlx::query("DELETE FROM logins WHERE user_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }

    async fn set_membership(&self, org_id: &str, user_id: &str, role: Role) -> Result<()> {
        sqlx::query(
            "INSERT INTO memberships (org_id, user_id, role, created_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (org_id, user_id) DO UPDATE SET role = excluded.role",
        )
        .bind(org_id)
        .bind(user_id)
        .bind(role_name(role))
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_membership(&self, org_id: &str, user_id: &str) -> Result<bool> {
        let mut tx = self.write_tx().await?;
        sqlx::query(
            "DELETE FROM team_members WHERE user_id = $1 AND team_id IN (SELECT id FROM teams WHERE org_id = $2)",
        )
        .bind(user_id)
        .bind(org_id)
        .execute(&mut *tx)
        .await?;
        let r = sqlx::query("DELETE FROM memberships WHERE org_id = $1 AND user_id = $2")
            .bind(org_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_members(&self, org_id: &str) -> Result<Vec<Membership>> {
        let rows = sqlx::query(&format!(
            "{MEMBERSHIPS} WHERE m.org_id = $1 ORDER BY u.email"
        ))
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(membership).collect()
    }

    async fn user_memberships(&self, user_id: &str) -> Result<Vec<Membership>> {
        let rows = sqlx::query(&format!(
            "{MEMBERSHIPS} WHERE m.user_id = $1 ORDER BY o.created_at, o.slug"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(membership).collect()
    }

    async fn create_team(&self, org_id: &str, name: &str) -> Result<Team> {
        let id = gai_core::new_id("team");
        sqlx::query("INSERT INTO teams (id, org_id, name, created_at) VALUES ($1, $2, $3, $4)")
            .bind(&id)
            .bind(org_id)
            .bind(name)
            .bind(now())
            .execute(&self.pool)
            .await
            .map_err(|e| unique(e, format!("a team {name} exists")))?;
        self.get_team(org_id, &id)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("team {name}")))
    }

    async fn get_team(&self, org_id: &str, id_or_name: &str) -> Result<Option<Team>> {
        Ok(self
            .teams_where(org_id, Some(id_or_name))
            .await?
            .into_iter()
            .next())
    }

    async fn list_teams(&self, org_id: &str) -> Result<Vec<Team>> {
        self.teams_where(org_id, None).await
    }

    async fn delete_team(&self, org_id: &str, team_id: &str) -> Result<bool> {
        let mut tx = self.write_tx().await?;
        // A team's keys would otherwise act for the whole organization.
        sqlx::query("UPDATE api_keys SET revoked_at = $1 WHERE team_id = $2 AND org_id = $3 AND revoked_at IS NULL")
            .bind(now())
            .bind(team_id)
            .bind(org_id)
            .execute(&mut *tx)
            .await?;
        let r = sqlx::query("DELETE FROM teams WHERE org_id = $1 AND id = $2")
            .bind(org_id)
            .bind(team_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(r.rows_affected() > 0)
    }

    async fn add_team_member(&self, team_id: &str, user_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO team_members (team_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(team_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_team_member(&self, team_id: &str, user_id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM team_members WHERE team_id = $1 AND user_id = $2")
            .bind(team_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn set_owner(
        &self,
        org_id: &str,
        kind: &str,
        name: &str,
        team_id: Option<&str>,
    ) -> Result<()> {
        let Some(team) = team_id else {
            sqlx::query(
                "DELETE FROM resource_owners WHERE org_id = $1 AND kind = $2 AND name = $3",
            )
            .bind(org_id)
            .bind(kind)
            .bind(name)
            .execute(&self.pool)
            .await?;
            return Ok(());
        };
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM teams WHERE id = $1 AND org_id = $2")
            .bind(team)
            .bind(org_id)
            .fetch_one(&self.pool)
            .await?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("team {team}")));
        }
        sqlx::query(
            "INSERT INTO resource_owners (org_id, kind, name, team_id) VALUES ($1, $2, $3, $4)
             ON CONFLICT (org_id, kind, name) DO UPDATE SET team_id = excluded.team_id",
        )
        .bind(org_id)
        .bind(kind)
        .bind(name)
        .bind(team)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_owner(&self, org_id: &str, kind: &str, name: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT team_id FROM resource_owners WHERE org_id = $1 AND kind = $2 AND name = $3",
        )
        .bind(org_id)
        .bind(kind)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn list_owners(&self, org_id: &str) -> Result<Vec<(String, String, String)>> {
        let rows = sqlx::query(
            "SELECT kind, name, team_id FROM resource_owners WHERE org_id = $1 ORDER BY kind, name",
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok((
                    r.try_get("kind")?,
                    r.try_get("name")?,
                    r.try_get("team_id")?,
                ))
            })
            .collect()
    }

    async fn create_login(
        &self,
        hash: &str,
        user_id: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO logins (hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(hash)
        .bind(user_id)
        .bind(now())
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_login(&self, hash: &str) -> Result<Option<(Login, User)>> {
        let Some(r) =
            sqlx::query("SELECT user_id, created_at, expires_at FROM logins WHERE hash = $1")
                .bind(hash)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(None);
        };
        let login = Login {
            user_id: r.try_get("user_id")?,
            created_at: r.try_get("created_at")?,
            expires_at: r.try_get("expires_at")?,
        };
        if login.expires_at <= now() {
            self.delete_login(hash).await?;
            return Ok(None);
        }
        let Some(u) = self.get_user(&login.user_id).await?.filter(|u| !u.disabled) else {
            return Ok(None);
        };
        sqlx::query("UPDATE users SET last_seen_at = $1 WHERE id = $2")
            .bind(now())
            .bind(&u.id)
            .execute(&self.pool)
            .await?;
        Ok(Some((login, u)))
    }

    async fn delete_login(&self, hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM logins WHERE hash = $1")
            .bind(hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
