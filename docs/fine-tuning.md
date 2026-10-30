# Fine-tuning

gai can train a model on your own assistants' best answers: the runs
people rated thumbs up. Start under **Settings › AI models › Fine-tuned
models**. When training is done the model is in the catalog, and you pick
it like any other.

## The training set

Each run rated thumbs up becomes one example: what the model was sent on
the run's last model call (the system prompt, the conversation so far,
and the tool definitions) followed by its answer, tool calls included. It
is written in the chat format OpenAI and Together take, one conversation
per line:

```json
{"messages": [{"role": "system", "content": "You help customers."}, {"role": "user", "content": "..."}, {"role": "assistant", "content": "..."}], "tools": [...]}
```

Runs are skipped when they have images or audio, or when their model
calls weren't recorded (`record_requests = false`). The form says how many
examples there are before you start, and **Download the training set**
saves it as a `.jsonl` file to inspect or use elsewhere.

## Where it trains

| Trainer | Base models | Needs |
|---|---|---|
| `openai` | OpenAI's fine-tunable models (`gpt-4.1-mini-2025-04-14`, ...); at least 10 examples | the OpenAI key |
| `together` | Together's fine-tunable open models; trains a LoRA adapter | `TOGETHER_API_KEY` (or a key saved for `together`) |
| `local` | A Hugging Face model id | an NVIDIA GPU and Docker or Podman (see [model servers](model-servers.md)) |

A `local` job runs in a container on the GPU (`pytorch/pytorch` with
transformers, peft and trl), trains a LoRA adapter on the set, and keeps
it with vLLM's downloaded weights. When it finishes, gai starts a vLLM
[model server](model-servers.md) named `ft-<name>` that serves the base
model with the adapter, as `deploy-ft-<name>/<name>`.

gai checks running jobs every minute (**Check now** asks straight away).
A finished OpenAI or Together model is added to that provider's catalog,
for example `openai/ft:gpt-4.1-mini-2025-04-14:acme:support:abc123`, and
kept across restarts.

## API

```yaml
name: support-v2
trainer: openai            # openai | together | local
base_model: gpt-4.1-mini-2025-04-14
dataset:
  agent: support           # only this assistant's runs
  since: 2026-10-01T00:00:00Z
  limit: 2000
  without_tools: false
epochs: 3
learning_rate: 1.8         # OpenAI: a multiplier; others: the rate
lora_rank: 16              # together and local
```

| Method | Path | |
|---|---|---|
| GET | `/v1/finetunes` | Jobs, newest first |
| POST | `/v1/finetunes` | Build the set and start training. Admin |
| POST | `/v1/finetunes/dataset` | What the set would hold: counts, skipped runs, a few examples. Admin |
| POST | `/v1/finetunes/dataset.jsonl` | The set itself. Admin |
| GET, DELETE | `/v1/finetunes/{name}` | One job; deleting forgets a finished job, not its model. Admin |
| POST | `/v1/finetunes/{name}/cancel`, `/check` | Admin |
