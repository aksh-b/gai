# Knowledge at scale

Knowledge collections are documents split into chunks that agents search
with `rag_search`. Out of the box everything lives in gai's database
(SQLite, or Postgres) and search scans it. This page covers the parts for
bigger setups: an external **vector index**, **reranking**, and **sources**
that keep a collection in step with a folder, a website, GitHub,
Confluence or Notion.

## Scanned documents (OCR)

Images (PNG, JPEG, TIFF, WebP and others) and PDFs without a text layer
are read by OCR when they are added, from uploads, folders, sources and
`gai ingest` alike:

```toml
[knowledge.ocr]
backend = "auto"              # auto (default) | tesseract | model | off
# model = "openai/gpt-4o-mini" # a vision model: for `model`, and `auto` without Tesseract
# languages = "eng+deu"        # Tesseract's language packs
```

`auto` uses [Tesseract](https://github.com/tesseract-ocr/tesseract) when
it is installed (the Docker image has it), else the vision model when one
is set, else nothing (images are then skipped in folders and refused on
upload). Scanned PDFs are rendered page by page with poppler's `pdftoppm`
when it is installed, else their embedded scans are read directly. The
document's metadata says what read it (`"ocr": "tesseract"`).

## Whole documents

Agents can work with whole documents, not just search hits. Add these to
an agent's `tools` (the **Contract reviewer** template has them all):

| Tool | Does |
| --- | --- |
| `summarize_document` | summarizes a document of any length (in parts, then together), with an optional focus |
| `compare_documents` | what two documents share, where they differ (both wordings), and what only one has |
| `extract_fields` | reads the same fields from every document in a collection into a table, saved as CSV |
| `write_document` | saves Markdown as a Markdown, Word (`.docx`) or PDF file the user can download |

A document is named by its title, source or id in a collection, or as a
file in the run's workspace. They use the agent's model, and respect who
may find what ([acl](users.md#knowledge-only-some-may-find)).

Extraction works over HTTP too:

```bash
curl -X POST "http://localhost:8484/v1/collections/contracts/extract?format=csv" \
  -H "content-type: application/json" \
  -d '{"fields": {"party": "the other party", "end_date": "when it ends", "fee": "the monthly fee"}}'
```

`fields` is either `{"name": "what it is"}` or a JSON Schema with
`properties`; leave out `?format=csv` for JSON rows. Fields a document
doesn't state come back empty. In the dashboard, a collection's **Table**
tab does the same and downloads the CSV.

## Knowledge graph

A model can read a collection for the people, organizations, places,
products, events and amounts it mentions, and the relations between them
("Globex —supplies→ Acme"). Search then also finds passages through those
connections: asking who leads the company Globex supplies finds the
passage about Acme's CEO, though it never names Globex.

```bash
gai collections graph contracts                 # read every passage into the graph
gai collections search contracts "who runs Acme" --mode graph
```

(or `POST /v1/collections/{name}/graph?wait=true`, or **Build the graph**
on the collection's **Graph** tab, which also draws it). Builds read only
passages they haven't read, so run them again after adding documents, or
create the collection with `"metadata": {"graph": true}` to have it done
after every ingest. The model is `[knowledge.graph] model`, else the
default model; each call reads a few passages.

Agents' `rag_search` uses the graph when a collection has one, and adds the
connections it found to the results. Re-ingesting a document drops what
was learned from it; `DELETE /v1/collections/{name}/graph` forgets the
whole graph. A collection with documents only some may find shows its
graph to admins only, and searches show others only the connections their
own passages bear out.

## Vector indexes

The store stays the source of truth; an index only answers "which chunks
are nearest". Ingesting, replacing and deleting documents keep it in sync.

```toml
[knowledge.vector_index]
backend = "qdrant"               # builtin (default) | qdrant | pgvector
url = "http://localhost:6333"
# api_key_env = "QDRANT_API_KEY"
# prefix = "gai_"                # Qdrant collection names: gai_<collection>
```

`pgvector` uses the database when it is Postgres, or `url =
"postgres://…"` for another one. It keeps one table (`gai_vectors`) with an
HNSW index per embedding size.

Collections that already have documents need one reindex after switching:

```bash
gai collections reindex handbook
```

(or `POST /v1/collections/{name}/reindex`). If the index can't be reached,
search falls back to the store and logs a warning.

## Reranking

Hybrid search (keywords plus vectors) finds candidates; a reranker reads
each one against the question and reorders them.

```toml
[knowledge.rerank]
backend = "http"                 # off (default) | http | llm
url = "https://api.cohere.com/v2/rerank"
model = "rerank-v3.5"
api_key_env = "COHERE_API_KEY"
candidates = 30                  # how many fused results are reranked
```

`http` speaks the Cohere, Jina, Voyage and Hugging Face TEI rerank APIs.
`llm` asks a chat model (`model = "ollama/qwen3:8b"`) to grade each
passage, which is slower but needs no extra service. Hits carry a
`rerank_score`; if the reranker fails, the fused order is kept.

## Sources

A source fills a collection from somewhere else and re-syncs only what
changed: new items are added, changed ones replaced, and items gone from
the source removed from the collection. Each chunk's metadata keeps the
`source`, the item's `url` and its version.

```yaml
# sources.yaml
- name: handbook
  collection: handbook
  kind: folder
  path: /srv/handbook
  every_minutes: 60

- name: docs-site
  collection: product-docs
  kind: web
  sitemap: https://docs.example.com/sitemap.xml
  # or urls: [https://docs.example.com/], depth: 2
  max_pages: 500

- name: runbooks
  collection: ops
  kind: github
  repo: acme/runbooks
  path: docs                     # only this folder
  token_env: GITHUB_TOKEN        # private repositories

- name: eng-wiki
  collection: wiki
  kind: confluence
  base_url: https://acme.atlassian.net/wiki
  space: ENG
  email_env: CONFLUENCE_EMAIL    # leave out on Data Center (bearer token)
  token_env: CONFLUENCE_TOKEN

- name: notes
  collection: notes
  kind: notion
  token_env: NOTION_TOKEN
  database_id: 0f3c…             # or page_ids: [...], or neither for every page shared with the integration
```

```bash
gai sources save sources.yaml
gai sources sync handbook        # 3 added, 0 updated, 0 removed, 0 unchanged, 0 failed
gai sources list
```

Over HTTP: `POST /v1/sources` (the spec as JSON, or `{"yaml": "..."}`),
`GET /v1/sources`, `GET|DELETE /v1/sources/{name}`, and `POST
/v1/sources/{name}/sync` (in the background, or `?wait=true` for the
counts). Changing sources needs an `admin` key.

While `gai serve` runs, sources with `every_minutes` sync on their own.
An item that fails (a page that 404s, a file that can't be read) is
counted and retried on the next sync; the rest still go in. Deleting a
source leaves its documents in the collection.

Add `acl: ["team:eng"]` (or `user:<email>`, `role:<role>`) to a source to
let only those people find what it brings in; see
[users.md](users.md#knowledge-only-some-may-find).

Web sources stay on the sites they start from and refuse localhost and
private addresses unless `allow_private: true`. Tokens are read from the
environment variables you name, never stored.

Not built yet: Google Drive, SharePoint and S3 sources, and reading each
item's permissions from the system it came from (the `acl` is set per
source).
