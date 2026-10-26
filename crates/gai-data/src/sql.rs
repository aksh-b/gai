//! Checking SQL before it reaches a database: one statement, and for
//! queries, a `SELECT` (or `WITH ... SELECT`, `VALUES`). Databases that
//! have read-only transactions enforce it again on their side.

use sqlparser::ast::Statement;
use sqlparser::dialect::{
    BigQueryDialect, Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect, SnowflakeDialect,
};
use sqlparser::parser::Parser;

use crate::{DataError, Result};

fn dialect(kind: &str) -> Box<dyn Dialect> {
    match kind {
        "mysql" => Box::new(MySqlDialect {}),
        "sqlite" => Box::new(SQLiteDialect {}),
        "bigquery" => Box::new(BigQueryDialect {}),
        "snowflake" => Box::new(SnowflakeDialect {}),
        _ => Box::new(PostgreSqlDialect {}),
    }
}

/// The statement without trailing semicolons and space.
fn trimmed(sql: &str) -> String {
    sql.trim()
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
        .to_owned()
}

/// Whether the first word makes it look like a query (for SQL the parser
/// doesn't know).
fn looks_like_query(sql: &str) -> bool {
    let first = sql
        .trim_start_matches(|c: char| c == '(' || c.is_whitespace())
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(first.as_str(), "select" | "with" | "values")
}

/// One read-only query, ready to run.
pub fn check_query(sql: &str, kind: &str) -> Result<String> {
    let s = trimmed(sql);
    if s.is_empty() {
        return Err(DataError::Invalid("the query is empty".into()));
    }
    match Parser::parse_sql(dialect(kind).as_ref(), &s) {
        Ok(stmts) if stmts.len() != 1 => Err(DataError::Refused(format!(
            "send one statement at a time (got {})",
            stmts.len()
        ))),
        Ok(stmts) => match &stmts[0] {
            Statement::Query(_) => Ok(s),
            other => Err(DataError::Refused(format!(
                "only queries (SELECT) run here; {} changes or controls the database",
                kind_of(other)
            ))),
        },
        // Dialect corners the parser doesn't cover: allow what reads like
        // one query; the database's read-only mode is the backstop, so
        // engines without one (Snowflake) need a parseable query.
        Err(e) if kind != "snowflake" && looks_like_query(&s) && !s.contains(';') => {
            tracing::debug!(error = %e, "SQL the parser doesn't know; letting the database check it");
            Ok(s)
        }
        Err(e) => Err(DataError::Invalid(format!("the SQL doesn't parse: {e}"))),
    }
}

/// One statement of any kind (for connections that allow writes).
pub fn check_single(sql: &str, kind: &str) -> Result<String> {
    let s = trimmed(sql);
    match Parser::parse_sql(dialect(kind).as_ref(), &s) {
        Ok(stmts) if stmts.len() == 1 => Ok(s),
        Ok(stmts) => Err(DataError::Refused(format!(
            "send one statement at a time (got {})",
            stmts.len()
        ))),
        Err(e) => Err(DataError::Invalid(format!("the SQL doesn't parse: {e}"))),
    }
}

fn kind_of(s: &Statement) -> String {
    let text = s.to_string();
    let first: Vec<&str> = text.split_whitespace().take(2).collect();
    first.join(" ").to_uppercase()
}

/// `name` quoted as an identifier for `kind`.
pub(crate) fn quote(kind: &str, name: &str) -> String {
    match kind {
        "mysql" | "bigquery" => format!("`{}`", name.replace('`', "``")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_queries_pass() {
        assert_eq!(check_query("SELECT 1;  ", "postgres").unwrap(), "SELECT 1");
        assert!(check_query("WITH a AS (SELECT 1 AS x) SELECT x FROM a", "postgres").is_ok());
        assert!(check_query("select * from `p.d.t` limit 5", "bigquery").is_ok());
        assert!(check_query("SELECT 1; DROP TABLE users", "postgres").is_err());
        for bad in [
            "DELETE FROM users",
            "UPDATE t SET a = 1",
            "DROP TABLE t",
            "INSERT INTO t VALUES (1)",
            "CREATE TABLE x (a int)",
            "GRANT ALL ON t TO bob",
        ] {
            let e = check_query(bad, "postgres").unwrap_err();
            assert!(matches!(e, DataError::Refused(_)), "{bad}: {e}");
        }
        assert!(check_query("", "sqlite").is_err());
        assert!(check_query("DELETE FROM t", "snowflake").is_err());
        assert_eq!(
            check_single("UPDATE t SET a = 1;", "mysql").unwrap(),
            "UPDATE t SET a = 1"
        );
        assert!(check_single("UPDATE t SET a = 1; DELETE FROM t", "mysql").is_err());
        assert_eq!(quote("mysql", "a`b"), "`a``b`");
        assert_eq!(quote("postgres", "a\"b"), "\"a\"\"b\"");
    }
}
