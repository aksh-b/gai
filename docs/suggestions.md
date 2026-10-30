# Suggestions and proactive assistants

**Activity › Suggestions** is an inbox of things worth doing. Each
suggestion says what to do and why, and has a button that does it:
starts an assistant with a message, adds a template, or opens a page.
**Dismiss** removes it.

## From how gai is used

gai looks at the last month of use and suggests:

- **improving an assistant** when at least 5 of its runs finished and
  more than 30% failed, or when more than 30% of at least 5 ratings were
  thumbs down (open it, turn the runs into a test set, press **Improve**);
- **the Database analyst** when a database is connected but no assistant
  queries it and there are no dashboards;
- **the documents helper** when there are documents in the knowledge base
  but no assistant searches them;
- **the Code assistant** when GitHub is connected, and **the Email
  drafter** when Gmail or Outlook is, but no assistant uses them;
- **a schedule** for a question asked of the same assistant 4 or more
  times.

Each of these is made once. A dismissed one doesn't come back.

## From proactive assistants

Any agent with the `suggest` tool can add a suggestion:

```json
{
  "title": "Restock SKU 1182",
  "details": "Only 3 left; 40 sold last week.",
  "action": {"kind": "run_agent", "agent": "buyer", "input": "Draft a purchase order for SKU 1182"}
}
```

`action` is optional and is one of `run_agent` (`agent`, `input`),
`install_template` (`template`) or `open` (`link`, a dashboard page such
as `#/dashboards/sales`). An open suggestion with the same title from the
same agent isn't added twice.

The **Proactive watcher** template is such an agent. Give it a schedule
under **Automations › Schedules**, with a message saying what to watch:

```yaml
name: morning-check
kind: cron
schedule: "0 8 * * 1-5"
target: { agent: watcher }
input: "Check yesterday's orders and stock levels in the shop database."
```

Rules `on: [suggestion]` under **Settings › Notifications** send new
suggestions by email, Slack or to the browser (see
[notifications](notifications.md)).

## API

| Method | Path | |
|---|---|---|
| GET | `/v1/suggestions` | Open suggestions, newest first |
| POST | `/v1/suggestions/{id}/accept` | Do what it says; answers with the `run_id`, what was `installed`, and a `link` |
| POST | `/v1/suggestions/{id}/dismiss` | Dismiss it |

Accepting an `install_template` suggestion needs the build scope.
