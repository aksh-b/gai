# Databases

Agents can answer questions from your databases: they look at the tables,
write SQL, run it, and chart the results. gai connects to **Postgres**,
**MySQL** (and MariaDB), **SQLite**, **BigQuery** and **Snowflake**.

Connections are **read-only** unless you say otherwise: every query is
checked to be a single `SELECT` before it is sent, and runs in a read-only
transaction with a time limit where the database has them. Passwords and
tokens never go into gai: a connection names the environment variable
that holds them.

## Connecting

```yaml
# app.yaml
name: app
kind: postgres
url: postgres://reader@db.internal:5432/app
password_env: APP_DB_PASSWORD
description: Users, signups and orders
# read_only: true     # the default
# max_rows: 1000      # rows one query returns at most
# timeout_secs: 30
```

```bash
gai data save app.yaml          # connects and reads the tables
gai data schema app
gai data query app "SELECT channel, COUNT(*) FROM signups GROUP BY 1"
```

Or in the dashboard under **Data**, or `POST /v1/connections` (admins).

| kind | Needs |
| --- | --- |
| `postgres`, `mysql` | `url` (without the password), `password_env` |
| `sqlite` | `path` to the database file |
| `bigquery` | `project`, optional `dataset` and `location`; `credentials_env` (a service account's JSON key) or `token_env` (an access token); `max_bytes_billed` (default 10 GB) |
| `snowflake` | `account`, `token_env` (a programmatic access token, or `token_type: OAUTH`), optional `database`, `schema`, `warehouse`, `role` |

Give the database user (or Snowflake role, or BigQuery service account)
read access only, as a second lock. BigQuery queries are dry-run first: a
query that would scan more than `max_bytes_billed` is refused before it
costs anything. Snowflake has no read-only transactions, so its SQL must
parse as a single query.

To let agents change data, set `read_only: false`: then `sql_execute`
runs `INSERT`, `UPDATE`, `DELETE` and the like, and the user approves
each statement.

In an [organization](users.md#organizations) other than the default one,
passwords come from the organization's secrets, SQLite files must be in its
own `files` folder, and databases on private network addresses are refused.

## The tools

| Tool | Does |
| --- | --- |
| `sql_schema` | lists the connections, or a connection's tables and columns |
| `sql_query` | runs one `SELECT` and returns the rows, also saved as `data/<name>.csv` in the workspace; `dry_run` checks it and shows the plan |
| `sql_execute` | runs one statement that changes data, on connections that allow it; needs approval |

The **Database analyst** template uses them with `code_exec`, so it can
chart results and dig into why a number changed ("why did signups drop
last week?": the change overall, then by channel, plan, country and so on).

## API

| | |
| --- | --- |
| `GET, POST /v1/connections` | list and save connections |
| `GET, DELETE /v1/connections/{name}` | one connection, with its last schema |
| `GET /v1/connections/{name}/schema` | tables and columns (`?refresh=true` reads them again) |
| `POST /v1/connections/{name}/query` | `{"sql": "...", "limit": 100, "dry_run": false}` |
