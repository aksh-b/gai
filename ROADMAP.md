# gai roadmap: phases 13–21

Phases 1–12 are built (see [STATUS.md](STATUS.md)). These phases cover the
62 features from the GenAI feature list that gai has only partly (37) or
not at all (25). Numbers in brackets are that list's numbers. The phases
are ordered so each builds on the ones before: quality and safety first,
because they make everything after them measurable and safe to open up;
Postgres before multi-user, because tenants need a real database.

## 13. Quality loop (done, 2026-10-25)

Know whether agents are good, and catch regressions. Built as described,
except that citations are checked by the `cites` scorer but not yet shown
as links in chat.

- **Feedback and ratings** [74]: thumbs up/down and comments on any reply, stored per run and step, shown in run history.
- **Regenerate** [10]: one click in chat, built on fork-from-step.
- **Evaluation framework** [62]: eval sets (inputs, expected output or rubric), scorers (exact, regex, JSON schema, LLM judge), runs over agent versions and models.
- **Regression testing** [89]: `gai eval run` in CI; fail when a new agent version scores below the labelled one.
- **Model benchmarking** [59]: the same eval set across models, with quality, cost and latency side by side.
- **Grounding checks** [17, 63]: answers from knowledge must cite their sources (links in chat); a judge flags claims not supported by the retrieved text.

Done when an agent version can be evaluated, compared and blocked in CI from the CLI and the dashboard.

## 14. Safety and governance (done, 2026-10-25)

- **Prompt-injection protection** [71]: tool output and fetched pages are marked as untrusted, with a detector hook (heuristics plus an optional classifier model) that can warn, strip or stop.
- **Sensitive data detection** [70]: PII detectors (emails, phones, cards, IDs) on inputs, outputs and tool results, with mask, block or allow per agent.
- **Guardrail policies** [72]: declarative policies (topics, output rules, tool limits) checked before and after model calls, on top of the existing hooks.
- **Audit log** [69]: who changed which agent, key, setting or MCP server, and who approved which tool call, queryable and exportable.
- **Privacy and retention** [73]: per-agent retention (delete runs and messages after N days), redaction on export, and "forget this user".

Done when every policy decision is visible in traces and the audit log.

## 15. Storage and search at scale — done (2026-10-25)

Built: Postgres for everything, Qdrant and pgvector indexes, HTTP and LLM
reranking, and sources for folders, websites, GitHub, Confluence and
Notion. Not built: Google Drive, SharePoint and S3 sources; per-source
permissions are kept on chunks (`acl`) but enforced only with phase 16.

- **Postgres** (groundwork exists): port the remaining repositories, run the conformance suite on every backend, and allow `database = "postgres://…"`.
- **Vector databases** [85]: a `VectorStore` backend for pgvector and Qdrant beside the built-in one.
- **Reranking** [86]: an optional cross-encoder or LLM reranker after hybrid search.
- **Enterprise connectors** [14, 77]: sync sources into collections (Google Drive, SharePoint, Confluence, Notion, GitHub, S3), with incremental re-sync and per-source permissions kept on chunks.

Done when a server runs on Postgres with an external vector store and synced sources.

## 16. Users, teams and tenants — done (2026-10-25)

Built: users with roles, OIDC and proxy sign-in, teams owning resources and
team keys, organizations each in their own database (budgets, rate limits,
branding), conversation search, per-agent analytics, @mentions of
assistants, who changed what, and source permissions on search. Not built:
notifying people mentioned in chats, private chats, and per-item
permissions read from the source systems.

- **Users and RBAC** [67]: user accounts with roles (owner, admin, builder, member, viewer) and per-resource permissions, beyond API key scopes.
- **SSO** [68]: OIDC (and SAML through a proxy) sign-in for the dashboard, with users created on first sign-in.
- **Teams and organizations** [80]: organizations with teams; agents, knowledge and keys owned by a team.
- **Multi-tenancy** [81]: tenant isolation in the store (a tenant id on every row, enforced in the repositories), per-tenant quotas and budgets.
- **Collaborative workspace** [55]: shared chats and agents, mentions, and seeing who edited what.
- **Conversation search** [75]: full-text search over sessions and messages, filtered by agent, user and date.
- **Per-agent analytics** [87]: success rate, steps, latency, cost and ratings per agent and version.
- **Branding** [99]: logo, colours and product name for the dashboard, per tenant.

Done when two organizations share one server without seeing each other's data.

## 17. Documents and knowledge plus — done (2026-10-26)

Built: OCR (Tesseract or a vision model), summarize/compare/extract/write
document tools and a contract-reviewer template, extraction to a table
(API, tool, dashboard), and knowledge graphs with graph search and a
graph view. Not built: merging near-duplicate entities, and questions
answered from the graph alone (counts, paths).

- **OCR** [37]: scanned PDFs and images on ingest, through a local engine (Tesseract) or a vision model.
- **Document tools** [12, 13, 19]: `summarize_document`, `compare_documents` and `write_document` (Markdown, DOCX and PDF output saved as files), plus templates that use them.
- **Information extraction** [18]: extract a JSON schema's fields from a document or a whole collection into a table.
- **Knowledge graph** [20, 98]: entity and relation extraction on ingest, graph-aware retrieval (GraphRAG), and a graph view in the dashboard.

Done when a folder of scanned contracts can be searched, compared and turned into a table.

## 18. Data and BI — done (2026-10-26)

Built: read-only connections to Postgres, MySQL, SQLite, BigQuery and
Snowflake with schemas; `sql_schema`/`sql_query`/`sql_execute`;
dashboards with schedules and KPI alerts; reports delivered by email or
webhook from triggers; a data-analysis skill (drivers, forecasts,
anomalies, data quality). Not checked: the end-to-end "why did signups
drop" answer with a real model.

- **Database connectors** [40]: Postgres, MySQL, SQLite, BigQuery and Snowflake as read-only sources with schema introspection.
- **Text-to-SQL** [41, 93]: a `sql_query` tool with schema context, validation and a dry run, row limits and approval for anything that isn't a SELECT.
- **Dashboards** [44]: agents build dashboards (charts with their queries) that refresh on a schedule.
- **Reports** [45]: scheduled report workflows that send a document by email or webhook.
- **Analytics skills** [46, 47, 48, 49, 50]: forecasting, anomaly detection, KPI monitors with alerts, root-cause drill-downs and data-quality checks, as sandbox skills over query results.

Done when "why did signups drop last week?" gets a correct, charted answer from a connected database.

## 19. Real-time and rich media — done (2026-10-27)

Built: voice conversations in the chat and over `/v1/voice` (sentence-by-
sentence speech, barge-in, realtime speech-to-speech relay with tools);
speakers, timestamps, language and sentiment for recordings; meeting notes
with decisions and action items and templates; video summaries from frames
and speech; `gai notes`. Not checked: a real model and a live realtime
session end to end.

- **Real-time voice** [35, 39]: streaming speech in and out over WebRTC or WebSocket, with barge-in, through providers' realtime APIs.
- **Audio intelligence** [38]: speaker labels, language detection and sentiment on transcripts.
- **Meetings** [95]: transcribe a recording, then produce a summary, decisions and action items, with a template.
- **Video summarization** [36]: sample frames and transcribe the audio, then summarize them with a vision model.

Done when you can talk to an agent in the playground and drop in a meeting recording.

## 20. Builders and marketplace — parts 1 and 2 done (2026-10-27)

Part 1 built: the workflow designer (a canvas that round-trips with the
YAML through `needs` and `layout`), apps (a form with its own link and
key, or public) and their endpoints (`/v1/apps/{name}/run` with input and
output schemas and a per-app OpenAPI document).

Part 2 built: the marketplace (listings with versions and ratings on
every server, other servers as registries, install with a dry run),
versioned prompt templates that agents use with `prompt:` and bundles
carry, and the prompt optimizer (rewrites from failing cases, kept only
when they score higher, applied as a new version). Not checked: the
optimizer with a real model. Still to do: proactive suggestions,
notifications and the specialist assistants.

- **Visual workflow designer** [24]: drag-and-drop editing on the xyflow graph, round-tripping with the YAML.
- **App builder** [51]: publish an agent or workflow as a small hosted app (form in, result out) with its own link and key.
- **Custom endpoints** [78]: give an agent or workflow its own REST endpoint with an input and output schema.
- **Template library and marketplace** [53, 54]: browse, rate and install bundles from a registry; versioned prompt templates usable from any agent.
- **Prompt optimizer** [7]: rewrite instructions to raise an eval score (uses phase 13).
- **Proactive assistant and recommendations** [57, 58]: agents that watch triggers and data and suggest actions; suggested agents, tools and templates from usage.
- **Notifications** [90]: email, Slack and web push for approvals, finished runs and alerts.
- **Specialist assistants** [91, 92, 94, 96]: code generation and review (repository access plus sandbox), translation, and email/message drafting with Gmail and Outlook through MCP, as templates with the tools they need.

Done when a non-developer can build, publish and share an app without YAML.

## 21. Models and localization

- **Custom model deployment** [82]: start and stop local model servers (Ollama, vLLM, llama.cpp) from the dashboard, with health and GPU status.
- **Fine-tuning** [83]: build training sets from rated runs (phase 13), start provider fine-tunes (OpenAI, Together) or local LoRA jobs, and register the result as a model.
- **Multilingual** [6]: a translated dashboard (i18n), and an agent language setting that applies to prompts and replies.

Done when a model fine-tuned on your own rated runs can be picked like any other.
