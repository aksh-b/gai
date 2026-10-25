# Users, teams and organizations

One gai server can serve several **organizations** that never see each
other's data. People sign in with your identity provider (OIDC) or through
a proxy that signs them in, and get a **role** in each organization they
belong to. **Teams** own assistants, workflows and knowledge, so only their
builders change them. API keys keep working as before: every key belongs
to one organization, and optionally to a team.

Nothing here is needed for one person on a laptop: without SSO settings or
other organizations, gai works exactly as it did, with access keys.

## Roles

| Role | Can |
| --- | --- |
| viewer | see everything in the organization |
| member | also chat, run assistants and workflows, answer approvals |
| builder | also change assistants, personas, workflows, skills, knowledge, schedules, sources and tests |
| admin | also keys, MCP servers, providers, settings, people, teams, secrets, the audit log and deleting data |
| owner | also rename, brand or delete the organization, and make other owners |

Keys have the matching scopes: `read`, `run`, `build` and `admin`. A key
can't be given more than the person making it has.

## Signing in

### OIDC

```toml
[server.sso]
admins = ["ada@example.com"]        # owners of the default organization
default_role = "member"             # people nobody invited join as this (leave out: invited only)
allowed_domains = ["example.com"]   # only these emails sign in (leave out: any)

[server.sso.oidc]
issuer = "https://accounts.example.com"
client_id = "gai"
client_secret_env = "GAI_OIDC_CLIENT_SECRET"   # leave out for a public client (PKCE alone)
redirect_url = "https://gai.example.com/v1/auth/callback"
# label = "Sign in with Okta"
```

Register `https://<your server>/v1/auth/callback` as the redirect URL with
your provider (Google, Microsoft Entra, Okta, Auth0, Keycloak, Authentik and
others). The dashboard then shows a **Sign in** button. gai uses the
authorization code flow with PKCE, checks the ID token's issuer, audience,
expiry and nonce, and refuses unverified emails. A sign-in lasts a week
(`session_hours`) in an HttpOnly cookie; signing out ends it.

People are created at their first sign-in. Someone you added by email
(`gai members set ada@example.com --role builder`, or **Settings › People
& teams**) gets that role then.

### A proxy that signs people in (SAML and the like)

Behind oauth2-proxy, a SAML gateway or similar, let it pass the signed-in
email in a header:

```toml
[server.sso.trusted_header]
email = "X-Forwarded-Email"
name = "X-Forwarded-User"
```

Only do this when the proxy removes that header from incoming requests;
otherwise anyone could claim to be anyone.

## Organizations

The server starts with the **default** organization, whose data is in the
main database. Admins of the default organization are the server's admins:
they create others.

```bash
gai orgs create acme --name "Acme" --owner ana@acme.example
gai orgs list
```

(or `POST /v1/orgs`, or **Settings › People & teams** as a server admin).

Each other organization gets a database of its own: a SQLite file in
`<data_dir>/orgs/<slug>/`, or a Postgres schema `org_<slug>` in the same
database. Its files, workspaces and vector index (`gai_<slug>_` collections
in Qdrant, `gai_vectors_<slug>` in pgvector) are its own too. People pick
the organization they work in at the top of the dashboard; scripts send
`X-Gai-Org: acme` with a sign-in, while a key always acts in its own
organization.

Organizations share the server's models and gai.toml settings, so they are
**confined** to keep them from reaching the server itself:

- no `shell` tool (it runs as the server's user);
- no private network addresses for `http_fetch`, the browser, web sources
  and MCP servers;
- MCP servers over HTTP only, without `${VAR}` (that would read the
  server's environment), and none of the MCP servers or A2A agents from
  gai.toml;
- folder sources and file triggers only inside `<data_dir>/orgs/<slug>/files`;
- source tokens (`token_env`, `email_env`) from the organization's own
  **secrets** instead of the server's environment.

```bash
curl -X PUT https://gai.example.com/v1/secrets/GITHUB_TOKEN \
  -H "Authorization: Bearer $ACME_ADMIN_KEY" -d '{"value": "ghp_..."}'
```

The `/mcp` endpoint (gai as an MCP server) and A2A agent cards serve the
default organization.

Deleting an organization (`gai orgs delete acme`) removes its people,
teams and keys and stops serving it. Its data stays on disk for you to
archive or remove.

### Limits and branding

Owners brand their organization: product name, logo and accent colour,
shown in the dashboard. Server admins set its limits:

```bash
curl -X PATCH https://gai.example.com/v1/orgs/acme -H "Authorization: Bearer $ADMIN_KEY" \
  -d '{"settings": {"limits": {"budget_usd_per_month": 200, "requests_per_minute": 300},
                    "branding": {"product_name": "Acme Assist", "accent": "#d6336c"}}}'
```

Once an organization has spent its monthly budget, new runs are refused
(402) until the month ends; reading still works.

## Teams

```bash
gai teams create support --org acme
gai teams add support sam@acme.example --org acme
```

Give a resource to a team with `PUT /v1/owners/<kind>/<name>` and
`{"team": "support"}` (kinds: agent, workflow, persona, skill, trigger,
source, eval, collection). From then on, only admins and the team's
builders change it; everyone can still see and use it. A **team key**
(`gai keys create --name ci --scope build --team support --org acme`)
changes only what its team owns, or what no team owns.

## Knowledge only some may find

A source can say who may find what it brings in:

```yaml
name: eng-wiki
collection: wiki
kind: confluence
# ...
acl: ["team:eng", "role:admin"]   # team:<name>, user:<email>, role:<role> (that role or higher)
```

Searches (`POST /v1/collections/{name}/search`), document lists and
documents leave those out for everyone else, and so does `rag_search`
inside a run: a run remembers who started it, and the agents it delegates
to and a workflow's steps inherit that. Admins find everything. Runs
started by schedules and webhooks find everything too, since builders set
them up. Documents you add with an `acl` in their metadata
(`POST /v1/collections/{name}/documents`) work the same way. Changing a
source's `acl` re-applies it at its next sync.

## Working together

Everyone in an organization sees its conversations, assistants and
workflows; roles decide who changes them.

- **Search chats** (**Activity › Search chats**, `GET /v1/sessions/search?q=`)
  finds words across every conversation, by assistant, person and date.
- **By assistant** (**Activity › By assistant**, `GET /v1/analytics/agents`)
  shows each version's runs, how often it finished well, steps, median and
  slowest latency, cost, tokens and ratings. Tests aren't counted.
- **@mentions** in a chat hand that turn to another assistant: "@researcher
  find the 2024 numbers" asks `researcher` in the same conversation.
- **Who changed what**: an assistant's page says who changed it last, and
  `GET /v1/changes?target=agents/helper` lists its changes for anyone in
  the organization (the full audit log stays with admins).

## API

| | |
| --- | --- |
| `GET /v1/me` | who you are, your organization, memberships and teams |
| `GET /v1/auth/config`, `/v1/auth/login`, `/v1/auth/callback`, `POST /v1/auth/logout` | signing in |
| `GET, POST /v1/orgs`, `GET, PATCH, DELETE /v1/orgs/{slug}` | organizations |
| `GET /v1/members`, `PUT, DELETE /v1/members/{email}` | people and roles |
| `GET, POST /v1/teams`, `DELETE /v1/teams/{team}`, `PUT, DELETE /v1/teams/{team}/members/{email}` | teams |
| `GET /v1/owners`, `PUT /v1/owners/{kind}/{name}` | which team owns what |
| `GET /v1/secrets`, `PUT, DELETE /v1/secrets/{name}` | the organization's secrets |
| `GET /v1/sessions/search` | find words in conversations |
| `GET /v1/analytics/agents` | how each agent version does |
| `GET /v1/changes?target=` | who changed one thing, and when |
