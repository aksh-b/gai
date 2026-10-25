# Evaluations and feedback

An **eval set** is a list of questions for an assistant, each with a way
to judge the answer. Run it whenever you change the assistant or try a new
model, and gai tells you what passed, what got worse, and what it cost.

```yaml
# support.yaml
name: support
description: The questions customers ask most.
agent: support             # evaluated when you don't name another
threshold: 0.9             # share of cases that must pass (default 0.8)
judge_model: anthropic/claude-sonnet-5-5
scorers:                   # applied to every case
  - max_steps: 6
cases:
  - id: refund-window
    input: Can I return shoes I bought five weeks ago?
    expected: 30 days      # shorthand for `contains: [30 days]`
  - id: angry-customer
    input: Give me a refund now or else.
    scorers:
      - judge: Declines calmly, explains the refund policy and offers help.
      - not_contains: [sorry for the inconvenience]
  - id: leave-policy
    input: How many days of annual leave do I get?
    scorers:
      - tool_called: rag_search
      - grounded: true     # every claim backed by what the search returned
      - cites: true        # names the document it used
    tags: [knowledge]
```

## Scorers

| Scorer | Passes when |
|---|---|
| `equals: text` | The whole answer is this (ignoring case and surrounding space) |
| `contains: [a, b]` | It contains every phrase (ignoring case); the score is the share found |
| `contains_any: [a, b]` | It contains at least one |
| `not_contains: [a, b]` | It contains none |
| `regex: pattern` | It matches the regular expression |
| `json: {fields: {total: 12, items.0.sku: a}}` | It is JSON (a code fence is fine) and the dotted paths have these values |
| `tool_called: name`, `tool_not_called: name` | The agent did (or didn't) call that tool |
| `max_steps: n`, `max_cost_usd: x`, `max_latency_ms: n` | The run stayed within the limit |
| `judge: rubric` | A model grades the answer 0–10 against the rubric (and `expected`, if given); 7 or more passes. Long form: `judge: {rubric, model, pass: 0.5}` |
| `grounded: true` | A model counts the answer's claims and how many the agent's sources back (knowledge search, fetched pages, files read); 80% passes. Long form: `grounded: {model, min: 0.9}` |
| `cites: true` | The answer names at least one source it retrieved (a file, document title or URL) |

A case's score is the mean of its scorers, and it passes when every scorer
passes. With no scorers, `expected` becomes `contains: [expected]`. The
judge model comes from the scorer, then the run (`--judge`), then the set's
`judge_model`, then the default model. `mock/judge` grades by word overlap,
so you can try evals without a key.

Each case runs as a normal run of the agent, with approvals skipped. The
runs are tagged `kind: eval` and linked from the results, so you can open
any case and see exactly what the agent did.

## Running

```bash
gai eval save support.yaml                      # stored, versioned
gai eval run support -a support@v7              # or run the YAML file directly
gai eval run support -a support -m anthropic/claude-sonnet-5-5 -m openai/gpt-5-mini -m ollama/qwen3:4b
gai eval run support --tag knowledge --case refund-window
gai eval report <eval-id>                        # per-case results
gai eval compare <baseline-id> <candidate-id>
```

Several `-m` flags benchmark models: the same cases on each model, with
score, pass count, cost and latency side by side.

In the dashboard: **Assistants › Tests**. Write or edit a set (the editor
checks it as you type), run it on an assistant and up to three models,
open a result to see every case's answer and why it failed, and pick two
results to compare them case by case.

## Gating CI

`gai eval run` exits with status 1 when the pass rate is below the set's
threshold, or, with `--baseline`, when the score drops by more than
`--max-drop` (0.05) against the baseline:

```bash
# the candidate version against the one in production, both evaluated now
gai eval run support.yaml -a support@v8 --baseline support@prod
# or against an earlier evaluation
gai eval run support.yaml -a support@v8 --baseline eval_01j...
```

The output lists newly failing and newly passing cases.

## Feedback

Every reply in chat has thumbs up and down. A thumbs down asks what went
wrong (wrong, incomplete, too long, ...) and what it should have said. The
ratings are kept per run and agent version:

- `POST /v1/runs/{id}/feedback {rating: 1 | -1, comment, tags}` (rating
  again replaces your rating), `DELETE` takes it back;
- `GET /v1/feedback?agent=support&rating=-1` lists them;
- `GET /v1/feedback/summary?agent=support` counts good and bad per version.

**Add thumbs-down replies** in a set's editor turns the assistant's
recently disliked replies into cases (the comment becomes the judge's
rubric), so what people complain about becomes a test.

**Regenerate** (under the last reply) runs the turn again as a fork: the
new reply continues the chat, and the old one stays in its own history.

## API

| Endpoint | What it does |
|---|---|
| `GET/POST /v1/evals` | List sets, save one (JSON, or `{"yaml": "..."}`) |
| `GET/DELETE /v1/evals/{name}`, `GET /v1/evals/{name}/versions` | A set (`name@v2`), its versions |
| `POST /v1/evals/{name}/runs` | Start evaluations in the background: `{agent, model}` or `{agent, models: [...]}`, plus `judge_model`, `cases`, `tags`, `concurrency` |
| `GET /v1/eval-runs/{id}` | The report: summary and per-case results (partial while running) |
| `GET /v1/eval-runs/compare?baseline=&candidate=&max_drop=` | Case-by-case comparison and a pass/fail verdict |

From Rust: `gai.evals().await?` gives an `Evaluator` (`save`, `load`,
`run`, `report`), and `gai::eval::{compare, gate}` do the rest.
