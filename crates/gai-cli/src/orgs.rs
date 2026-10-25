//! `gai orgs`, `gai members` and `gai teams`: who shares this server and
//! what they may do. These work on the main database directly, like
//! `gai keys`.

use clap::Subcommand;
use gai::Gai;
use gai::store::{DEFAULT_ORG, Role, SharedStore};

use crate::style::{dim, green};

#[derive(Subcommand)]
pub enum OrgsCmd {
    /// List organizations
    List,
    /// Create an organization; it gets a database and files of its own
    Create {
        /// Lowercase letters, digits and '-'
        slug: String,
        #[arg(long)]
        name: Option<String>,
        /// Make this person its owner (they join at their first sign-in)
        #[arg(long)]
        owner: Option<String>,
    },
    /// Delete an organization's members, teams and keys. Its data stays in
    /// <data_dir>/orgs/<slug> (or the Postgres schema org_<slug>)
    Delete { slug: String },
}

#[derive(Subcommand)]
pub enum MembersCmd {
    /// List members and their roles
    List {
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
    /// Add someone by email, or change their role
    Set {
        email: String,
        /// owner, admin, builder, member or viewer
        #[arg(long, default_value = "member")]
        role: Role,
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
    /// Take someone out of the organization (and its teams)
    Remove {
        email: String,
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
}

#[derive(Subcommand)]
pub enum TeamsCmd {
    /// List teams and their members
    List {
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
    Create {
        name: String,
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
    /// Delete a team; what it owned goes back to the organization
    Delete {
        name: String,
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
    /// Put a member in a team
    Add {
        team: String,
        email: String,
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
    /// Take a member out of a team
    Remove {
        team: String,
        email: String,
        #[arg(long, default_value = DEFAULT_ORG)]
        org: String,
    },
}

/// An organization's id from its slug or id.
pub async fn org_id(store: &SharedStore, slug: &str) -> anyhow::Result<String> {
    Ok(store
        .get_org(slug)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no organization {slug} (gai orgs list)"))?
        .id)
}

async fn control(gai: &Gai) -> anyhow::Result<SharedStore> {
    Ok(gai.runtime().await?.store().clone())
}

pub async fn orgs(gai: &Gai, cmd: OrgsCmd) -> anyhow::Result<()> {
    let store = control(gai).await?;
    match cmd {
        OrgsCmd::List => {
            for o in store.list_orgs().await? {
                let members = store.list_members(&o.id).await?.len();
                println!(
                    "{:<20} {:<24} {}",
                    o.slug,
                    o.name,
                    dim(&format!("{members} members"))
                );
            }
        }
        OrgsCmd::Create { slug, name, owner } => {
            let valid = (2..=40).contains(&slug.len())
                && slug.starts_with(|c: char| c.is_ascii_lowercase())
                && slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !valid || slug == DEFAULT_ORG {
                anyhow::bail!(
                    "slugs use 2-40 lowercase letters, digits and '-', starting with a letter"
                );
            }
            let o = store
                .create_org(&slug, name.as_deref().unwrap_or(&slug))
                .await?;
            if let Some(email) = owner {
                let u = store.create_user(&email, None).await?;
                store.set_membership(&o.id, &u.id, Role::Owner).await?;
            }
            // Its database is created now rather than at the first request.
            gai.for_org(&o.slug)?.runtime().await?;
            println!("{} {}", green("created"), o.slug);
        }
        OrgsCmd::Delete { slug } => {
            let id = org_id(&store, &slug).await?;
            store.delete_org(&id).await?;
            println!(
                "{} {slug} {}",
                green("deleted"),
                dim(&format!(
                    "(its data is still in {})",
                    gai.config().data_dir().join("orgs").join(&slug).display()
                ))
            );
        }
    }
    Ok(())
}

pub async fn members(gai: &Gai, cmd: MembersCmd) -> anyhow::Result<()> {
    let store = control(gai).await?;
    match cmd {
        MembersCmd::List { org } => {
            for m in store.list_members(&org_id(&store, &org).await?).await? {
                println!(
                    "{:<32} {:<8} {}",
                    m.email,
                    format!("{:?}", m.role).to_lowercase(),
                    dim(m.name.as_deref().unwrap_or(""))
                );
            }
        }
        MembersCmd::Set { email, role, org } => {
            let id = org_id(&store, &org).await?;
            let u = store.create_user(&email, None).await?;
            store.set_membership(&id, &u.id, role).await?;
            println!("{} {} is {:?} in {org}", green("ok"), u.email, role);
        }
        MembersCmd::Remove { email, org } => {
            let id = org_id(&store, &org).await?;
            let u = store
                .find_user_by_email(&email)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no one with {email}"))?;
            if !store.remove_membership(&id, &u.id).await? {
                anyhow::bail!("{email} isn't in {org}");
            }
            println!("{} {email} from {org}", green("removed"));
        }
    }
    Ok(())
}

pub async fn teams(gai: &Gai, cmd: TeamsCmd) -> anyhow::Result<()> {
    let store = control(gai).await?;
    match cmd {
        TeamsCmd::List { org } => {
            let id = org_id(&store, &org).await?;
            let members = store.list_members(&id).await?;
            for t in store.list_teams(&id).await? {
                let emails: Vec<&str> = t
                    .members
                    .iter()
                    .filter_map(|u| {
                        members
                            .iter()
                            .find(|m| &m.user_id == u)
                            .map(|m| m.email.as_str())
                    })
                    .collect();
                println!("{:<20} {}", t.name, dim(&emails.join(", ")));
            }
        }
        TeamsCmd::Create { name, org } => {
            let t = store
                .create_team(&org_id(&store, &org).await?, &name)
                .await?;
            println!("{} {}", green("created"), t.name);
        }
        TeamsCmd::Delete { name, org } => {
            let id = org_id(&store, &org).await?;
            let t = store
                .get_team(&id, &name)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no team {name}"))?;
            store.delete_team(&id, &t.id).await?;
            println!("{} {name}", green("deleted"));
        }
        TeamsCmd::Add { team, email, org } => {
            let (t, user) = team_and_member(&store, &org, &team, &email).await?;
            store.add_team_member(&t, &user).await?;
            println!("{} {email} to {team}", green("added"));
        }
        TeamsCmd::Remove { team, email, org } => {
            let (t, user) = team_and_member(&store, &org, &team, &email).await?;
            store.remove_team_member(&t, &user).await?;
            println!("{} {email} from {team}", green("removed"));
        }
    }
    Ok(())
}

/// A team's id and a member's user id; team members must belong to the
/// organization.
async fn team_and_member(
    store: &SharedStore,
    org: &str,
    team: &str,
    email: &str,
) -> anyhow::Result<(String, String)> {
    let id = org_id(store, org).await?;
    let t = store
        .get_team(&id, team)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no team {team}"))?;
    let m = store
        .list_members(&id)
        .await?
        .into_iter()
        .find(|m| m.email.eq_ignore_ascii_case(email))
        .ok_or_else(|| {
            anyhow::anyhow!("{email} isn't in {org}: gai members set {email} --org {org}")
        })?;
    Ok((t.id, m.user_id))
}
