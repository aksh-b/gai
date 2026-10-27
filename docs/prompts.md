# Prompt templates and the prompt optimizer

Instructions can live on their own as **prompt templates**: versioned like
agents, shared by as many assistants as you like, and improved by the
**prompt optimizer**, which rewrites them to score higher on a test set.

## Prompt templates

**Assistants › Prompts › New prompt**, or `POST /v1/prompts`:

```yaml
name: support-tone
title: Support tone
text: |
  You answer customers of {{ company }}. Be brief and kind, and say what
  happens next.
variables:
  company: { description: Who we are, default: Acme }
```

The text is a template with the same variables as an agent's `system`
(`{{ date }}`, `{{ agent }}`, run `vars`…). `variables` describe your own
and give defaults that a run's `vars` can override. Saving makes a new
version; the **Versions** tab diffs versions, rolls back and labels them.

An assistant uses one with **Start from a prompt** in its form, or in YAML:

```yaml
name: support
model: anthropic/claude-sonnet-5-5
prompt: support-tone@prod     # or support-tone (latest), support-tone@v3
system: Never promise refunds over $500.   # added after the prompt
```

The prompt's text comes first, then the agent's own `system`. Point the
`prod` label at a version to change every assistant that follows it at
once, and back again if it goes wrong. Each run records the prompt and
version it used (`metadata.prompt`). A prompt that an assistant uses
can't be deleted.

Bundles (and so the marketplace) carry the prompts their assistants use,
with their labels, so `support-tone@prod` works on the server it is
installed on.

API: `GET|POST /v1/prompts`, `GET|DELETE /v1/prompts/{name}`
(`?version=`), `/versions`, `/diff`, `/rollback`,
`POST|DELETE /v1/prompts/{name}/labels/{label}`, and
`POST /v1/prompts/render` to see the text with variables filled in.
Saving needs the build scope.

## The prompt optimizer

On a test set (**Assistants › Tests › Improve**, or
`POST /v1/evals/{name}/optimize`), the optimizer:

1. runs the set against the assistant as it is;
2. shows a model the instructions and the cases that failed: the input,
   the expected answer, what the assistant said and why it was marked
   down;
3. asks for better instructions, and runs the set again with each;
4. keeps a rewrite only when it scores higher than the best so far, and
   goes again from there, for a few rounds or until every case passes.

Nothing changes until you press **Save as a new version**
(`POST /v1/optimizations/{id}/apply`): the best instructions become the
assistant's next version, or, when it uses a prompt template, the
template's. The version note records the scores (`score 0.50 → 1.00`).

```bash
curl -X POST localhost:8484/v1/evals/support-basics/optimize \
  -H "Authorization: Bearer $KEY" -H "content-type: application/json" \
  -d '{"rounds": 3, "candidates": 2, "writer_model": "anthropic/claude-opus-5-5"}'
# → {"id": "job_…"}; follow GET /v1/optimizations/job_…
```

| Option | |
|---|---|
| `agent`, `model` | The assistant (default: the set's) and the model it runs on (default: its own) |
| `writer_model` | The model that rewrites (default: the default model). A strong one works best |
| `rounds` | Rounds of rewriting, 1 to 10 (default 3) |
| `candidates` | Rewrites tried per round, 1 to 5 (default 2) |
| `target_score` | Stop once the score reaches this (default 1.0) |
| `judge_model`, `cases`, `tags`, `concurrency` | As for an evaluation |

Each evaluation it runs is kept under **Results** (marked with
`optimizer.round`), so you can look at any attempt case by case. Rewrites
keep template variables such as `{{ company }}` as they are. Cost: every
attempt runs the whole set, so `rounds × candidates + 1` evaluations at
most.

Offline, `mock/rewriter` (as the writer) and `mock/obey` (as the
assistant's model) show the whole loop without a real model.
