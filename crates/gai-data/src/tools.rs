//! `sql_schema`, `sql_query` and `sql_execute`: what agents use to answer
//! questions from databases. Query results are also saved as CSV files in
//! the workspace, so `code_exec` can chart or analyse them.

use std::sync::Arc;

use gai_tools::{SharedTool, ToolCtx, ToolError, ToolOutput, ToolPolicy, typed_tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{Data, DataError};

fn err(e: DataError) -> ToolError {
    match e {
        DataError::Invalid(m) | DataError::Refused(m) | DataError::NotFound(m) => {
            ToolError::InvalidInput(m)
        }
        e => ToolError::Failed(e.to_string()),
    }
}

#[derive(Deserialize, JsonSchema)]
struct SchemaArgs {
    /// The connection; leave it out to list the connections.
    #[serde(default)]
    connection: Option<String>,
    /// Only these tables.
    #[serde(default)]
    tables: Vec<String>,
    /// Read the schema from the database again, not from what was kept.
    #[serde(default)]
    refresh: bool,
}

#[derive(Deserialize, JsonSchema)]
struct QueryArgs {
    connection: String,
    /// One SELECT in the connection's SQL dialect.
    sql: String,
    /// Rows to return at most (default and maximum: the connection's limit).
    #[serde(default)]
    limit: Option<usize>,
    /// Only check it and show the plan (BigQuery: the bytes it would scan).
    #[serde(default)]
    dry_run: bool,
    /// Also save the rows as this CSV file in the workspace (default: a
    /// name from the connection).
    #[serde(default)]
    save_as: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ExecuteArgs {
    connection: String,
    /// One statement that changes data (INSERT, UPDATE, DELETE, ...).
    sql: String,
}

fn csv_name(given: Option<&str>, connection: &str) -> String {
    let base = given.map(|g| {
        g.rsplit(['/', '\\'])
            .next()
            .unwrap_or(g)
            .trim_end_matches(".csv")
            .to_owned()
    });
    let base =
        base.unwrap_or_else(|| format!("{connection}-{}", chrono::Utc::now().format("%H%M%S")));
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("data/{clean}.csv")
}

/// The SQL tools over `data`.
pub fn tools(data: Data) -> Vec<SharedTool> {
    let d = data.clone();
    let schema = typed_tool(
        "sql_schema",
        "See the databases you can query and their tables and columns. Call it without a connection to list connections, then with one (and optionally some tables) before writing SQL.",
        move |_ctx, a: SchemaArgs| {
            let d = d.clone();
            async move {
                let Some(name) = a.connection else {
                    let list = d.list().await.map_err(err)?;
                    if list.is_empty() {
                        return Ok(ToolOutput::text("There are no database connections yet."));
                    }
                    let lines: Vec<String> = list
                        .iter()
                        .map(|c| {
                            let desc = c.spec["description"].as_str().map(|s| format!(": {s}")).unwrap_or_default();
                            let ro = if c.spec["read_only"] == false { "" } else { ", read-only" };
                            format!("- {} ({}{ro}){desc}", c.name, c.kind)
                        })
                        .collect();
                    return Ok(ToolOutput::text(format!("Connections:\n{}", lines.join("\n"))));
                };
                let s = d.schema(&name, a.refresh).await.map_err(err)?;
                let text = s.describe(&a.tables);
                Ok(ToolOutput::text(if text.is_empty() {
                    format!("{name} has no tables{}.", if a.tables.is_empty() { "" } else { " by those names" })
                } else {
                    format!("Tables in {name} ({}):\n{text}", d.spec(&name).await.map(|s| s.kind.name()).unwrap_or("sql"))
                }))
            }
        },
    )
    .idempotent();

    let d = data.clone();
    let query = typed_tool(
        "sql_query",
        "Run one read-only SQL query (SELECT) on a database connection and get the rows, also saved as a CSV file in the workspace for code_exec. Look at the tables with sql_schema first. Aggregate in SQL (GROUP BY, date_trunc) rather than fetching raw rows.",
        move |ctx: ToolCtx, a: QueryArgs| {
            let d = d.clone();
            async move {
                if a.dry_run {
                    let plan = d.dry_run(&a.connection, &a.sql).await.map_err(err)?;
                    return Ok(ToolOutput::text(format!("The query is valid. Plan:\n{plan}")));
                }
                let r = d.query(&a.connection, &a.sql, a.limit).await.map_err(err)?;
                let rel = csv_name(a.save_as.as_deref(), &a.connection);
                let saved = ctx
                    .resolve_path(&rel)
                    .ok()
                    .and_then(|p| {
                        std::fs::create_dir_all(p.parent()?).ok()?;
                        std::fs::write(&p, r.csv()).ok()
                    })
                    .map(|_| format!(" Saved as {rel}."))
                    .unwrap_or_default();
                let shown = r.rows.len().min(50);
                let more = if r.truncated {
                    format!(" More rows were left out (the limit is {}); aggregate or filter to see everything.", r.rows.len())
                } else if shown < r.rows.len() {
                    format!(" Showing the first {shown}; the CSV has them all.")
                } else {
                    String::new()
                };
                Ok(ToolOutput::text(format!(
                    "{} rows in {} ms.{saved}{more}\n\n{}",
                    r.rows.len(),
                    r.elapsed_ms,
                    r.markdown(50)
                )))
            }
        },
    )
    .idempotent();

    let d = data;
    let execute = typed_tool(
        "sql_execute",
        "Run one statement that changes data (INSERT, UPDATE, DELETE, CREATE ...) on a connection that allows writes. The user approves each one.",
        move |_ctx, a: ExecuteArgs| {
            let d = d.clone();
            async move {
                let n = d.execute(&a.connection, &a.sql).await.map_err(err)?;
                Ok(ToolOutput::text(format!("Done: {n} rows affected.")))
            }
        },
    )
    .policy(ToolPolicy::Approve);

    vec![Arc::new(schema), Arc::new(query), Arc::new(execute)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_names_stay_in_data() {
        assert_eq!(
            csv_name(Some("../../etc/signups.csv"), "app"),
            "data/signups.csv"
        );
        assert_eq!(
            csv_name(Some("weekly signups"), "app"),
            "data/weekly-signups.csv"
        );
        assert!(csv_name(None, "app").starts_with("data/app-"));
    }
}
