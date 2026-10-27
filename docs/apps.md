# Apps and the workflow designer

An app turns an assistant or a workflow into something other people can
use without knowing gai: a form with its own link, and an endpoint with an
input and output schema. The workflow designer lets you build the workflow
behind it by dragging steps on a canvas. Neither needs YAML.

## Building a workflow on the canvas

**Automations › Workflows › Edit** opens the **Designer**:

- **Add a step** with the buttons under the canvas: ask an assistant, use
  a tool, ask a person, or put text together.
- **Click a step** to edit it below the canvas. Buttons insert an input or
  another step's result into its text.
- **Drag from a step's right edge to another step** to make the second
  wait for the first. A solid line is such a wait (`needs` in YAML); a
  dashed line means the step uses the other's result in its text, so it
  waits for it anyway. Dashed lines go away when you edit the text.
- **Select a step or a solid line and press Delete** to remove it.
- **Drag steps** to arrange them. Where they sit is saved with the
  workflow under `layout`, which only the dashboard reads:

```yaml
layout:
  research: [0, 40]
  outline: [240, 40]
```

The **List** tab shows the same steps top to bottom, and **YAML** (with
developer details on) the text; changes in one show in the others.

## Making an app

**Automations › Apps › New app**:

1. Give it a title. Its address (`summarize-a-page`) becomes part of the
   link.
2. Pick the assistant or workflow it runs.
3. Choose what people fill in: short or long text, a number, yes/no, or a
   choice from a list, each with a label, help text and a default. A
   workflow app's fields are the workflow's inputs (**Use the workflow's
   inputs** fills them in); without fields, the form asks for all of them.
4. For an assistant, write the task with the fields in it: `Summarize
   {{ url }} in {{ length }} bullet points.` Without a task, the assistant
   gets the fields as `Label: value` lines.
5. Choose the result: text, or data (JSON) in a shape you describe with a
   JSON Schema. An assistant is asked to answer in that shape; a workflow's
   result is checked against it.

**Try it** runs the app as people will see it.

## Sharing

People in your organization can use any app from the dashboard. For
everyone else, **Share** gives the app a link:

- **Make a key** to get a link with the app's own key in it
  (`#/app/summarize?key=gai_app_…`). The key runs this app and nothing
  else: not other apps, not the API. It's shown once; **New key** replaces
  it and stops the old link working, **Remove key** closes the app again.
- Or turn on **Anyone with the link**, and the link works without a key.

Runs started through an app are charged to the organization and count
towards its monthly budget and rate limit, and each app is also limited to
`server.rate_limit_per_minute` runs a minute for callers without a key of
their own. A step that asks a person waits in **Needs your OK** as usual;
the app's page says it's waiting and picks up the result once it's
approved. Runs show in **Activity** with `app` in their metadata.

## The endpoint

Every app is also an API:

```bash
curl -X POST http://localhost:8484/v1/apps/summarize/run \
  -H "Authorization: Bearer gai_app_…" \
  -H "content-type: application/json" \
  -d '{"url": "https://example.com", "length": "short"}'
```

```json
{"run_id": "run_…", "status": "completed", "done": true, "output": "…", "error": null}
```

- The body is the fields as a JSON object. Unknown names, missing required
  fields and values of the wrong type are refused with a 400; numbers and
  yes/no values may also be sent as text.
- `output` is the result: the JSON your schema describes, or text. If a
  finished result doesn't follow the schema, `status` is `invalid_output`
  and `error` says where.
- `?mode=background` answers at once with `{"run_id"}`; fetch
  `GET /v1/apps/summarize/runs/<run_id>` until `done` is true.
- `GET /v1/apps/summarize/openapi.json` is an OpenAPI document for this
  endpoint alone, with its input and output schemas, to hand to whoever
  calls it.
- Your own access keys (with the run scope) and dashboard sign-ins work
  too; a public app needs no key at all.

Apps are managed under `/v1/apps` (saving and keys need the build scope).
An app is JSON (or the same in YAML):

```yaml
name: summarize
title: Summarize a page
description: Paste a link, get the gist.
agent: researcher              # or workflow: brief
fields:
  - { name: url, label: Web address }
  - { name: length, type: select, options: [short, long], default: short }
prompt: "Read {{ url }} and write a {{ length }} summary."
output_schema:                 # optional
  type: object
  required: [summary]
  properties: { summary: { type: string } }
public: false
submit_label: Summarize
```

Field types are `text`, `long_text`, `number`, `boolean` and `select`.
Fields are required unless they have a default or `required: false`.
