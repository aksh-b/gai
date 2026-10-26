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

## Analysis

The **data-analysis** skill (installed with the Database analyst template,
or on its own from the template gallery) gives agents four tested Python
functions over query results, run with `code_exec`:

| Function | Answers |
| --- | --- |
| `drivers` | why a number changed: the last N days against the N before, broken down by each dimension, with each value's share of the change |
| `forecast` | what comes next: Holt-Winters with a weekly (or other) season and 80%/95% ranges |
| `anomalies` | which points are far off the rolling median |
| `quality` | empty or constant columns, duplicates, missing days, thin days, the newest date |

Each prints a short summary and saves a chart. Ask the analyst "why did
signups drop last week?": it reads the schema, queries signups by day and
segment, runs `drivers`, checks `quality` in case the drop is a data
problem, and answers with the numbers and a chart.

## Dashboards and alerts

A dashboard is panels of a query and a chart, refreshed on a schedule:

```yaml
# growth.yaml
name: growth
title: Growth
refresh_minutes: 60
notify: { email: [ana@example.com], webhook: https://hooks.example.com/gai }
panels:
  - title: Signups per day
    connection: app
    sql: SELECT day, COUNT(*) AS signups FROM signups GROUP BY day ORDER BY day
    chart: line            # line | bar | area | pie | number | table
    x: day
    y: [signups]
    alert: { below: 50 }   # a KPI monitor: the last row's signups
  - title: Signups today
    connection: app
    sql: SELECT COUNT(*) FROM signups WHERE day = CURRENT_DATE
    chart: number
```

```bash
gai data save-dashboard growth.yaml
gai data refresh growth
```

Or ask an agent with `save_dashboard` in its tools (the Database analyst
has it) to "make a dashboard of daily signups and tell me if it drops
under 50", or write one under **Data › Dashboards**. While `gai serve`
runs, dashboards with `refresh_minutes` refresh on their own. An alert is
sent when its number crosses the limit, not again while it stays there,
and again after it has come back.

## Scheduled reports

Any trigger (`gai triggers`, **Automations › Schedules & webhooks**) can deliver what its run produced: the final
answer as the message, and the files it wrote (`write_document`'s Word or
PDF, charts) as attachments.

```yaml
name: weekly-signups
kind: cron
schedule: "0 8 * * MON"
target: { agent: sql-analyst }
input: |
  Write last week's signups report: the total against the week before,
  what drove the change, and a chart. Save it as a PDF with write_document.
deliver:
  email: [team@example.com]
  subject: Weekly signups
  # webhook: https://hooks.example.com/reports
  # webhook_secret_env: REPORT_WEBHOOK_SECRET   # signs it: X-Gai-Signature: sha256=<hmac>
```

Email goes through `[notify]` in gai.toml:

```toml
[notify]
smtp_url = "smtps://gai%40example.com@smtp.example.com:465"   # smtp://host:587 for STARTTLS
password_env = "GAI_SMTP_PASSWORD"
from = "gai <gai@example.com>"
# smtp_url = "file:///var/mail/gai"   # write .eml files instead, for trying it out
```

Webhooks get JSON: `subject`, `text`, `attachments` (name, media type and
base64 data), and `kind`, `run_id` or `dashboard`.

## API

| | |
| --- | --- |
| `GET, POST /v1/connections` | list and save connections |
| `GET, DELETE /v1/connections/{name}` | one connection, with its last schema |
| `GET /v1/connections/{name}/schema` | tables and columns (`?refresh=true` reads them again) |
| `POST /v1/connections/{name}/query` | `{"sql": "...", "limit": 100, "dry_run": false}` |
| `GET, POST /v1/dashboards`, `GET, DELETE /v1/dashboards/{name}` | dashboards and their last results |
| `POST /v1/dashboards/{name}/refresh` | run its queries now |
