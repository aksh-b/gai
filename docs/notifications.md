# Notifications

Notification rules say who hears about what happens in gai, and how.
Set them up under **Settings › Notifications**, or with the API.

## Events

| Event | When |
|---|---|
| `approval` | A tool call or a workflow's human step waits for someone to approve it |
| `run_failed` | A run ended with an error (chats, scheduled runs, workflows, app runs) |
| `run_finished` | A run completed |
| `alert` | A dashboard panel's alert started firing |

Only top-level runs count: an agent's delegates and a workflow's steps
don't send their own, and evaluation runs never do.

## Rules

```yaml
name: on-call
on: [approval, run_failed]
agents: [triage, weekly-report]   # every run when left out; dashboards by name for alerts
email: [ops@example.com]
slack_env: OPS_SLACK_WEBHOOK      # or slack: https://hooks.slack.com/services/...
webhook: https://example.com/hooks/gai
webhook_secret_env: GAI_HOOK_SECRET
push: true                        # every browser that turned notifications on
enabled: true
```

- **Email** needs `[notify] smtp_url` in gai.toml (see below).
- **Slack** takes an [incoming webhook](https://api.slack.com/messaging/webhooks)
  URL. It is a secret, so prefer `slack_env`, the name of an environment
  variable holding it.
- **Webhooks** get the notification as JSON: `subject`, `text`, `kind`,
  `rule`, `url` and details such as `run_id`, `agent`, `tool` or
  `dashboard`. With `webhook_secret_env` the body is signed:
  `X-Gai-Signature: sha256=<hex HMAC-SHA256 of the body>`.
- **Browser** notifications use web push. In **Settings › Notifications**
  press **Turn on** in each browser that should get them. They show up
  even when gai is closed, and clicking one opens the run or dashboard.
  Approval notifications stay on screen until dismissed. Browsers need
  https, or `localhost`.

**Send a test** sends a sample through a rule straight away.

Dashboards keep their own `notify:` for their alerts. A rule `on: [alert]`
also hears about them, so one rule can cover every dashboard.

## Settings

```toml
[notify]
smtp_url = "smtps://gai%40example.com@smtp.example.com:465"
password_env = "GAI_SMTP_PASSWORD"
from = "gai <gai@example.com>"
dashboard_url = "https://gai.example.com"   # makes links in notifications absolute
push_subject = "mailto:ops@example.com"     # who push services may contact; default: from
push_key_env = "GAI_PUSH_KEY"               # the web push key; see below
```

Web push signs messages with a key (VAPID). Without `push_key_env` gai
makes one the first time it is needed and keeps it in the organization's
database. If you run several servers on separate databases, or want to
keep the key outside the database, set `push_key_env` to a P-256 private
key in base64url PKCS#8. Changing the key means every browser has to
turn notifications on again.

## API

| Method | Path | |
|---|---|---|
| GET | `/v1/notifications/rules` | The rules |
| POST | `/v1/notifications/rules` | Save one (JSON, or `{"yaml": "..."}`), replacing one of the same name. Admin |
| GET, DELETE | `/v1/notifications/rules/{name}` | One rule. Deleting is admin |
| POST | `/v1/notifications/rules/{name}/test` | Send a sample through it. Admin |
| GET | `/v1/notifications/push` | The web push public key and the subscribed browsers |
| POST | `/v1/notifications/push` | Subscribe a browser (a `PushSubscription` as JSON) |
| POST | `/v1/notifications/push/unsubscribe` | `{"id": "<device id or endpoint>"}` |

Each organization has its own rules and subscriptions.
