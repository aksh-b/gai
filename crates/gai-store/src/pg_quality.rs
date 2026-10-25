//! Postgres implementation of feedback and eval storage.

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::{PostgresStore, json, now, parse};
use crate::*;

fn feedback(r: &PgRow) -> Result<Feedback> {
    Ok(Feedback {
        id: r.try_get("id")?,
        run_id: r.try_get("run_id")?,
        session_id: r.try_get("session_id")?,
        agent: r.try_get("agent")?,
        agent_version: r
            .try_get::<Option<i64>, _>("agent_version")?
            .map(|v| v as u32),
        user_id: r.try_get("user_id")?,
        rating: r.try_get::<i64, _>("rating")? as i8,
        comment: r.try_get("comment")?,
        tags: parse(&r.try_get::<String, _>("tags")?)?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

#[async_trait]
impl FeedbackStore for PostgresStore {
    async fn put_feedback(
        &self,
        run: &Run,
        user_id: &str,
        rating: i8,
        comment: Option<&str>,
        tags: &[String],
    ) -> Result<Feedback> {
        let rating = rating.signum();
        if rating == 0 {
            return Err(StoreError::Conflict(
                "a rating is 1 (good) or -1 (bad)".into(),
            ));
        }
        let ts = now();
        sqlx::query(
            "INSERT INTO feedback (id, run_id, session_id, agent, agent_version, user_id, rating, comment, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT(run_id, user_id) DO UPDATE SET rating = excluded.rating,
                 comment = excluded.comment, tags = excluded.tags, updated_at = excluded.updated_at",
        )
        .bind(gai_core::new_id("fb"))
        .bind(&run.id)
        .bind(&run.session_id)
        .bind(&run.agent)
        .bind(run.agent_version.map(|v| v as i64))
        .bind(user_id)
        .bind(rating as i64)
        .bind(comment)
        .bind(json(&tags))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        let row = sqlx::query("SELECT * FROM feedback WHERE run_id = $1 AND user_id = $2")
            .bind(&run.id)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?;
        feedback(&row)
    }

    async fn delete_feedback(&self, run_id: &str, user_id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM feedback WHERE run_id = $1 AND user_id = $2")
            .bind(run_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_feedback(&self, f: FeedbackFilter) -> Result<Vec<Feedback>> {
        let rows = sqlx::query(
            "SELECT * FROM feedback
             WHERE ($1 IS NULL OR agent = $1) AND ($2 IS NULL OR run_id = $2) AND ($3 IS NULL OR rating = $3)
             ORDER BY created_at DESC LIMIT $4",
        )
        .bind(f.agent)
        .bind(f.run_id)
        .bind(f.rating.map(i64::from))
        .bind(if f.limit == 0 { 100 } else { f.limit as i64 })
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(feedback).collect()
    }

    async fn feedback_summary(&self, agent: Option<&str>) -> Result<Vec<FeedbackSummary>> {
        let rows = sqlx::query(
            "SELECT agent, agent_version,
                    SUM(CASE WHEN rating > 0 THEN 1 ELSE 0 END)::BIGINT AS good,
                    SUM(CASE WHEN rating < 0 THEN 1 ELSE 0 END)::BIGINT AS bad
             FROM feedback WHERE ($1 IS NULL OR agent = $1)
             GROUP BY agent, agent_version ORDER BY agent, agent_version",
        )
        .bind(agent)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(FeedbackSummary {
                    agent: r.try_get("agent")?,
                    agent_version: r
                        .try_get::<Option<i64>, _>("agent_version")?
                        .map(|v| v as u32),
                    good: r.try_get::<i64, _>("good")? as u64,
                    bad: r.try_get::<i64, _>("bad")? as u64,
                })
            })
            .collect()
    }
}

fn set_version(r: &PgRow) -> Result<EvalSetVersion> {
    Ok(EvalSetVersion {
        name: r.try_get("name")?,
        version: r.try_get::<i64, _>("version")? as u32,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        note: r.try_get("note")?,
        created_at: r.try_get("created_at")?,
    })
}

fn eval_run(r: &PgRow) -> Result<EvalRun> {
    Ok(EvalRun {
        id: r.try_get("id")?,
        eval_set: r.try_get("eval_set")?,
        eval_version: r.try_get::<i64, _>("eval_version")? as u32,
        target: parse(&r.try_get::<String, _>("target")?)?,
        status: r.try_get("status")?,
        summary: parse(&r.try_get::<String, _>("summary")?)?,
        error: r.try_get("error")?,
        created_at: r.try_get("created_at")?,
        finished_at: r.try_get("finished_at")?,
    })
}

fn eval_result(r: &PgRow) -> Result<EvalResult> {
    Ok(EvalResult {
        eval_run_id: r.try_get("eval_run_id")?,
        case_id: r.try_get("case_id")?,
        run_id: r.try_get("run_id")?,
        output: r.try_get("output")?,
        scores: parse(&r.try_get::<String, _>("scores")?)?,
        score: r.try_get("score")?,
        passed: r.try_get::<i64, _>("passed")? != 0,
        cost: r.try_get("cost")?,
        latency_ms: r.try_get::<i64, _>("latency_ms")? as u64,
        error: r.try_get("error")?,
    })
}

#[async_trait]
impl EvalStore for PostgresStore {
    async fn put_eval_set(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32> {
        let mut tx = self.write_tx().await?;
        let latest: Option<(i64, String)> = sqlx::query_as(
            "SELECT v.version, v.spec FROM eval_sets s JOIN eval_set_versions v ON v.name = s.name AND v.version = s.latest_version WHERE s.name = $1",
        )
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((v, s)) = &latest
            && parse::<Value>(s)? == *spec
        {
            return Ok(*v as u32);
        }
        let v = latest.map(|(v, _)| v + 1).unwrap_or(1);
        let ts = now();
        let cases = spec["cases"].as_array().map(Vec::len).unwrap_or(0) as i64;
        sqlx::query(
            "INSERT INTO eval_sets (name, latest_version, description, cases, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT(name) DO UPDATE SET latest_version = excluded.latest_version,
                 description = excluded.description, cases = excluded.cases, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(v)
        .bind(spec["description"].as_str())
        .bind(cases)
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO eval_set_versions (name, version, spec, note, created_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(name)
        .bind(v)
        .bind(json(spec))
        .bind(note)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(v as u32)
    }

    async fn get_eval_set(
        &self,
        name: &str,
        version: Option<u32>,
    ) -> Result<Option<EvalSetVersion>> {
        let row = match version {
            Some(v) => {
                sqlx::query("SELECT * FROM eval_set_versions WHERE name = $1 AND version = $2")
                    .bind(name)
                    .bind(v as i64)
                    .fetch_optional(&self.pool)
                    .await?
            }
            None => {
                sqlx::query(
                    "SELECT v.* FROM eval_sets s JOIN eval_set_versions v ON v.name = s.name AND v.version = s.latest_version WHERE s.name = $1",
                )
                .bind(name)
                .fetch_optional(&self.pool)
                .await?
            }
        };
        row.as_ref().map(set_version).transpose()
    }

    async fn list_eval_sets(&self) -> Result<Vec<EvalSetSummary>> {
        let rows = sqlx::query("SELECT * FROM eval_sets ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(EvalSetSummary {
                    name: r.try_get("name")?,
                    latest_version: r.try_get::<i64, _>("latest_version")? as u32,
                    description: r.try_get("description")?,
                    cases: r.try_get::<i64, _>("cases")? as u32,
                    labels: parse(&r.try_get::<String, _>("labels")?)?,
                    created_at: r.try_get("created_at")?,
                    updated_at: r.try_get("updated_at")?,
                })
            })
            .collect()
    }

    async fn list_eval_set_versions(&self, name: &str) -> Result<Vec<EvalSetVersion>> {
        let rows =
            sqlx::query("SELECT * FROM eval_set_versions WHERE name = $1 ORDER BY version DESC")
                .bind(name)
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(set_version).collect()
    }

    async fn delete_eval_set(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM eval_sets WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn create_eval_run(
        &self,
        eval_set: &str,
        eval_version: u32,
        target: &Value,
    ) -> Result<EvalRun> {
        let id = gai_core::new_id("eval");
        sqlx::query(
            "INSERT INTO eval_runs (id, eval_set, eval_version, target, status, created_at) VALUES ($1, $2, $3, $4, 'running', $5)",
        )
        .bind(&id)
        .bind(eval_set)
        .bind(eval_version as i64)
        .bind(json(target))
        .bind(now())
        .execute(&self.pool)
        .await?;
        self.get_eval_run(&id)
            .await?
            .ok_or(StoreError::NotFound(id))
    }

    async fn finish_eval_run(
        &self,
        id: &str,
        status: &str,
        summary: &Value,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE eval_runs SET status = $1, summary = $2, error = $3, finished_at = $4 WHERE id = $5",
        )
        .bind(status)
        .bind(json(summary))
        .bind(error)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_eval_run(&self, id: &str) -> Result<Option<EvalRun>> {
        let row = sqlx::query("SELECT * FROM eval_runs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(eval_run).transpose()
    }

    async fn list_eval_runs(&self, eval_set: Option<&str>, limit: u32) -> Result<Vec<EvalRun>> {
        let rows = sqlx::query(
            "SELECT * FROM eval_runs WHERE ($1 IS NULL OR eval_set = $1) ORDER BY created_at DESC LIMIT $2",
        )
        .bind(eval_set)
        .bind(if limit == 0 { 50 } else { limit as i64 })
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(eval_run).collect()
    }

    async fn delete_eval_run(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM eval_runs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn put_eval_result(&self, x: &EvalResult) -> Result<()> {
        sqlx::query(
            "INSERT INTO eval_results (eval_run_id, case_id, run_id, output, scores, score, passed, cost, latency_ms, error)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT(eval_run_id, case_id) DO UPDATE SET run_id = excluded.run_id, output = excluded.output,
                 scores = excluded.scores, score = excluded.score, passed = excluded.passed,
                 cost = excluded.cost, latency_ms = excluded.latency_ms, error = excluded.error",
        )
        .bind(&x.eval_run_id)
        .bind(&x.case_id)
        .bind(&x.run_id)
        .bind(&x.output)
        .bind(json(&x.scores))
        .bind(x.score)
        .bind(x.passed as i64)
        .bind(x.cost)
        .bind(x.latency_ms as i64)
        .bind(&x.error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_eval_results(&self, eval_run_id: &str) -> Result<Vec<EvalResult>> {
        let rows = sqlx::query("SELECT * FROM eval_results WHERE eval_run_id = $1 ORDER BY ord")
            .bind(eval_run_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(eval_result).collect()
    }
}
