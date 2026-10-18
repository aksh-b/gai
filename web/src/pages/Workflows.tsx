import { useEffect, useRef, useState } from 'react';
import { Button, Tabs, TextField, toast } from '@flame';
import { parse, stringify } from 'yaml';
import { api, followRun, type GraphNode, type Template, type WorkflowSpec, type WorkflowView } from '../api';
import { Empty, ErrorNote, Json, PageHeader, StatusPill } from '../components';
import { Dag, statusFromEvent, type NodeStatus } from '../Dag';
import { IconPlus, IconTrash } from '../icons';
import { navigate } from '../router';
import { ago, money, useLoad } from '../util';
import { VersionHistory } from '../Versions';
import { formCanEdit, WorkflowForm } from '../WorkflowForm';
import { useDevMode } from '../prefs';

const BLANK = `name: ""
description: Drafts a paragraph, then asks you before finishing.
inputs:
  topic: { description: What to write about }
nodes:
  - id: draft
    agent: { name: drafter }
    input: "Write one short paragraph about {{ inputs.topic }}."
  - id: check
    human: "Does this look right?\\n\\n{{ nodes.draft.output }}"
output: "{{ nodes.draft.output }}"
`;

export function Workflows({ selected }: { selected?: string }) {
  const [dev] = useDevMode();
  const list = useLoad(() => api.workflows(), []);
  // `pick`: choosing a template; `blank`: the editor on an empty workflow.
  const [creating, setCreating] = useState<false | 'pick' | 'blank'>(false);
  const current = creating ? undefined : (selected ?? list.data?.[0]?.name);
  useEffect(() => {
    if (list.data?.length === 0) setCreating((c) => c || 'pick');
  }, [list.data]);

  return (
    <div className="page">
      <PageHeader
        title="Workflows"
        subtitle={
          <>
            Several steps in a row: assistants, tools and your OK. Steps run as soon as what they need is ready, and a workflow
            picks up where it stopped if the server restarts.
          </>
        }
        actions={
          <Button tone="accent" leading={<IconPlus />} onClick={() => setCreating('pick')}>
            New workflow
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="agents-layout">
        <div className="agent-list">
          {list.data?.map((w) => (
            <a
              key={w.name}
              href={`#/automations/${encodeURIComponent(w.name)}`}
              className="agent-card"
              aria-current={current === w.name ? 'page' : undefined}
              onClick={(e) => {
                e.preventDefault();
                setCreating(false);
                navigate(`/automations/${encodeURIComponent(w.name)}`);
              }}
            >
              <div className="row between">
                <strong>{w.name}</strong>
                {dev && <span className="chip small">{w.source === 'stored' ? `v${w.latest_version}` : 'file'}</span>}
              </div>
              {w.description && <span className="muted small">{w.description}</span>}
              {dev && Object.keys(w.labels).length > 0 && (
                <div className="row gap wrap">
                  {Object.entries(w.labels).map(([l, v]) => (
                    <span key={l} className="chip accent small">
                      {l}=v{v}
                    </span>
                  ))}
                </div>
              )}
            </a>
          ))}
          {list.data?.length === 0 && !creating && (
            <Empty title="No workflows yet">Start from a template, or make your own.</Empty>
          )}
        </div>
        {creating === 'pick' ? (
          <WorkflowTemplates
            onBlank={() => setCreating('blank')}
            onInstalled={(name) => {
              setCreating(false);
              list.reload();
              navigate(`/automations/${encodeURIComponent(name)}`);
            }}
          />
        ) : creating === 'blank' ? (
          <Editor
            initial={BLANK}
            isNew
            onSaved={(name) => {
              setCreating(false);
              list.reload();
              navigate(`/automations/${encodeURIComponent(name)}`);
            }}
          />
        ) : current ? (
          <WorkflowPanel
            key={current}
            name={current}
            labels={list.data?.find((w) => w.name === current)?.labels ?? {}}
            onChange={list.reload}
          />
        ) : null}
      </div>
    </div>
  );
}

/** Starting points for a new workflow. */
function WorkflowTemplates({ onBlank, onInstalled }: { onBlank: () => void; onInstalled: (name: string) => void }) {
  const templates = useLoad(() => api.templates(), []);
  const [busy, setBusy] = useState<string | null>(null);
  async function install(t: Template) {
    setBusy(t.id);
    try {
      const r = await api.installTemplate(t.id);
      toast.success(r.also.length ? `Added ${r.name} (and the assistants it uses: ${r.also.join(', ')})` : `Added ${r.name}`);
      onInstalled(r.name);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }
  return (
    <section className="card stack">
      <div>
        <h3>New workflow</h3>
        <p className="muted small">Start from a template, or build one step by step.</p>
      </div>
      <ErrorNote error={templates.error} />
      <div className="template-grid">
        {(templates.data ?? [])
          .filter((t) => t.kind === 'workflow')
          .map((t) => (
            <button key={t.id} type="button" className="template" onClick={() => install(t)} disabled={busy !== null}>
              <strong>{t.title}</strong>
              <span className="muted small">{t.description}</span>
              {t.installed ? <span className="chip small">already added</span> : busy === t.id && <span className="chip small">adding…</span>}
            </button>
          ))}
        <button type="button" className="template blank" onClick={onBlank}>
          <strong>Build your own</strong>
          <span className="muted small">Start with two simple steps and change them.</span>
        </button>
      </div>
    </section>
  );
}

function WorkflowPanel({ name, labels, onChange }: { name: string; labels: Record<string, number>; onChange: () => void }) {
  const [dev] = useDevMode();
  const wf = useLoad(() => api.workflow(name), [name]);
  const [tab, setTab] = useState('run');
  const [viewing, setViewing] = useState<number>();
  if (wf.error) return <ErrorNote error={wf.error} />;
  if (!wf.data) return null;
  const w = wf.data;
  return (
    <section className="stack">
      <div className="card">
        <div className="row between">
          <div>
            <h2 style={{ margin: 0 }}>
              {w.name} {dev && w.version != null && <span className="muted small">v{w.version}</span>}
            </h2>
            {w.spec.description && <p className="muted" style={{ margin: '4px 0 0' }}>{w.spec.description}</p>}
          </div>
          {w.version != null && (
            <Button
              size="sm"
              variant="ghost"
              leading={<IconTrash />}
              onClick={async () => {
                if (!confirm(`Delete ${w.name} and all its versions?`)) return;
                await api.deleteWorkflow(w.name).catch((e: Error) => toast.error(e.message));
                navigate('/automations');
                onChange();
              }}
            >
              Delete
            </Button>
          )}
        </div>
      </div>
      <Tabs
        items={[
          { id: 'run', label: 'Run' },
          { id: 'runs', label: 'History' },
          { id: 'edit', label: 'Edit' },
          ...(w.version != null && dev ? [{ id: 'versions', label: 'Versions' }] : []),
        ]}
        value={tab}
        onValueChange={setTab}
        variant="underline"
        aria-label="Workflow view"
      />
      {tab === 'run' && <RunPanel w={w} />}
      {tab === 'runs' && <RecentRuns name={w.name} />}
      {tab === 'versions' && (
        <VersionsPanel
          name={w.name}
          labels={labels}
          onView={(v) => {
            setViewing(v);
            setTab('edit');
          }}
          onChanged={() => {
            wf.reload();
            onChange();
          }}
        />
      )}
      {tab === 'edit' && viewing !== undefined && <VersionEditor name={w.name} version={viewing} onSaved={() => {
        setViewing(undefined);
        wf.reload();
        onChange();
        setTab('run');
      }} />}
      {tab === 'edit' && viewing === undefined && (
        <Editor
          initial={stringify(w.spec)}
          isNew={false}
          onSaved={() => {
            wf.reload();
            onChange();
            setTab('run');
          }}
        />
      )}
    </section>
  );
}

function RunPanel({ w }: { w: WorkflowView }) {
  const inputs = Object.entries(w.spec.inputs ?? {});
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      inputs.map(([k, s]) => [k, s.default === undefined || s.default === null ? '' : typeof s.default === 'string' ? s.default : JSON.stringify(s.default)]),
    ),
  );
  const [status, setStatus] = useState<Record<string, NodeStatus>>({});
  const [runId, setRunId] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | undefined>();
  const [outputs, setOutputs] = useState<Record<string, { output?: unknown; error?: string }>>({});
  const [result, setResult] = useState<{ status: string; output?: unknown; error?: string } | null>(null);
  const [pending, setPending] = useState<{ id: string; name: string; input: unknown }[]>([]);
  const abort = useRef<AbortController | null>(null);
  useEffect(() => () => abort.current?.abort(), []);

  async function start() {
    const parsed: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(values)) {
      if (v === '') continue;
      try {
        parsed[k] = JSON.parse(v);
      } catch {
        parsed[k] = v;
      }
    }
    try {
      const ids = await api.runWorkflow(w.name, parsed);
      setRunId(ids.run_id);
      setStatus({});
      setOutputs({});
      setResult(null);
      setPending([]);
      abort.current?.abort();
      abort.current = new AbortController();
      followRun(
        ids.run_id,
        (e) => {
          setStatus((s) => statusFromEvent(s, e));
          if (e.type === 'node_finished') setOutputs((o) => ({ ...o, [e.node]: { output: e.output, error: e.error } }));
          if (e.type === 'approval_required') setPending((p) => [...p, { id: e.id, name: e.name, input: e.input }]);
          if (e.type === 'approval_resolved') setPending((p) => p.filter((x) => x.id !== e.id));
          if (e.type === 'run_finished') setResult({ status: e.status, output: e.output, error: e.error });
        },
        abort.current.signal,
      ).catch(() => {});
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <div className="stack">
      <section className="card stack">
        {inputs.length > 0 && (
          <div className="mcp-form" style={{ paddingTop: 0 }}>
            {inputs.map(([k, s]) => (
              <TextField
                key={k}
                label={k}
                description={s.description ?? undefined}
                placeholder={s.default === undefined || s.default === null ? 'required' : undefined}
                value={values[k] ?? ''}
                onChange={(e) => setValues((v) => ({ ...v, [k]: e.target.value }))}
              />
            ))}
          </div>
        )}
        <div className="row gap">
          <Button tone="accent" onClick={start}>
            Run workflow
          </Button>
          {runId && (
            <button type="button" className="link small" onClick={() => navigate(`/activity/${runId}`)}>
              Open run {runId.slice(-8)}
            </button>
          )}
        </div>
      </section>

      <section className="card">
        <Dag nodes={w.graph as GraphNode[]} status={status} selected={selected} onSelect={setSelected} />
        {selected && (
          <div className="stack tight" style={{ marginTop: 12 }}>
            <strong className="mono">{selected}</strong>
            {outputs[selected]?.error && <div className="error-note">{outputs[selected].error}</div>}
            {outputs[selected]?.output !== undefined ? <Json value={outputs[selected].output} /> : <span className="muted small">No output yet.</span>}
          </div>
        )}
      </section>

      {pending.map((p) => (
        <Waiting key={p.id} runId={runId!} p={p} />
      ))}

      {result && (
        <section className="card">
          <div className="row gap">
            <h3 style={{ margin: 0 }}>Result</h3>
            <StatusPill status={result.status as never} />
          </div>
          {result.error && <div className="error-note">{result.error}</div>}
          {result.output !== undefined && <Json value={result.output} />}
        </section>
      )}
    </div>
  );
}

function Waiting({ runId, p }: { runId: string; p: { id: string; name: string; input: unknown } }) {
  const [reply, setReply] = useState('');
  const prompt = p.name === 'human' ? String((p.input as { prompt?: string })?.prompt ?? '') : null;
  return (
    <section className="card approval-card">
      <strong>{prompt !== null ? 'Waiting for you' : `${p.name} needs your OK`}</strong>
      {prompt !== null ? <div className="pre">{prompt}</div> : <Json value={p.input} />}
      <div className="row gap">
        {prompt !== null && (
          <input className="plain-input" placeholder="Optional reply" aria-label="Reply" value={reply} onChange={(e) => setReply(e.target.value)} />
        )}
        <Button
          tone="accent"
          size="sm"
          onClick={() =>
            (reply.trim() ? api.decideWith(runId, p.id, { reply: reply.trim() }) : api.decide(runId, p.id, true)).catch((e: Error) =>
              toast.error(e.message),
            )
          }
        >
          Continue
        </Button>
        <Button size="sm" variant="ghost" onClick={() => api.decide(runId, p.id, false, 'declined in the dashboard').catch((e: Error) => toast.error(e.message))}>
          Stop here
        </Button>
      </div>
    </section>
  );
}

function RecentRuns({ name }: { name: string }) {
  const runs = useLoad(() => api.workflowRuns(name), [name]);
  if (runs.data?.length === 0) return <Empty title="No runs yet" />;
  return (
    <div className="card flush">
      {runs.data?.map((r) => (
        <a
          key={r.id}
          className="run-row"
          href={`#/activity/${r.id}`}
          onClick={(e) => {
            e.preventDefault();
            navigate(`/activity/${r.id}`);
          }}
        >
          <StatusPill status={r.status} />
          <span className="small">{new Date(r.created_at).toLocaleString()}</span>
          <span className="muted small">{r.steps} steps</span>
          <span className="muted small num">{money(r.cost)}</span>
          <span className="muted small">{ago(r.created_at)}</span>
        </a>
      ))}
    </div>
  );
}

function Editor({ initial, isNew, onSaved }: { initial: string; isNew: boolean; onSaved: (name: string) => void }) {
  const [dev] = useDevMode();
  const [text, setText] = useState(initial);
  const [spec, setSpec] = useState<Record<string, unknown>>(() => parse(initial) ?? {});
  const [mode, setMode] = useState<'form' | 'yaml'>(() => (formCanEdit(parse(initial) ?? {}) ? 'form' : 'yaml'));
  const [graph, setGraph] = useState<GraphNode[]>([]);
  const [error, setError] = useState<string | undefined>();
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const formOk = formCanEdit(spec);

  // Check as you go (debounced) with the server's validation.
  useEffect(() => {
    const t = setTimeout(async () => {
      try {
        const v = await api.validateWorkflow({ ...spec, name: spec.name || 'draft' });
        setError(v.ok ? undefined : (v.error ?? 'invalid'));
        if (v.ok) setGraph(v.graph);
      } catch (e) {
        setError((e as Error).message);
      }
    }, 400);
    return () => clearTimeout(t);
  }, [spec]);

  function fromForm(next: Record<string, unknown>) {
    setSpec(next);
    setText(stringify(next));
  }

  function fromYaml(t: string) {
    setText(t);
    try {
      const v = parse(t);
      if (!v || typeof v !== 'object') throw new Error('The workflow must be a YAML mapping.');
      setSpec(v);
    } catch (e) {
      setError(`YAML: ${(e as Error).message}`);
    }
  }

  async function save() {
    if (!String(spec.name ?? '').trim()) {
      setError('Give the workflow a name.');
      return;
    }
    setBusy(true);
    try {
      const r = await api.saveWorkflow(spec as WorkflowSpec, note.trim() || undefined);
      toast.success(dev ? `Saved ${r.name} v${r.version}` : `Saved ${r.name}`);
      onSaved(r.name);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  const showTabs = dev || !formOk;
  return (
    <section className="stack">
      {showTabs && (
        <Tabs
          items={[
            { id: 'form', label: 'Steps', disabled: !formOk },
            { id: 'yaml', label: 'YAML' },
          ]}
          value={mode}
          onValueChange={(m) => setMode(m as 'form' | 'yaml')}
          variant="underline"
          aria-label="Workflow editor"
        />
      )}
      {!formOk && <p className="note">This workflow uses steps the form can't show yet, so it opens as YAML.</p>}
      {mode === 'form' && formOk ? (
        <div className="card form-body">
          <WorkflowForm spec={spec} onChange={fromForm} isNew={isNew} />
        </div>
      ) : (
        <textarea className="code-editor" aria-label="Workflow YAML" spellCheck={false} value={text} onChange={(e) => fromYaml(e.target.value)} />
      )}
      {graph.length > 0 && (
        <div className="card">
          <h3>Preview</h3>
          <Dag nodes={graph} />
        </div>
      )}
      <ErrorNote error={error} />
      <div className="row gap">
        {dev && (
          <input className="plain-input" placeholder="Version note (optional)" aria-label="Version note" value={note} onChange={(e) => setNote(e.target.value)} />
        )}
        <div className="spacer" />
        <Button tone="accent" loading={busy} disabled={!!error && !error.startsWith('Give')} onClick={save}>
          {isNew ? 'Create workflow' : 'Save changes'}
        </Button>
      </div>
    </section>
  );
}

function VersionsPanel({
  name,
  labels,
  onView,
  onChanged,
}: {
  name: string;
  labels: Record<string, number>;
  onView: (v: number) => void;
  onChanged: () => void;
}) {
  const versions = useLoad(() => api.workflowVersions(name), [name]);
  return (
    <section className="card">
      <ErrorNote error={versions.error} />
      <VersionHistory
        kind="workflow"
        name={name}
        versions={versions.data ?? []}
        labels={labels}
        onView={onView}
        onChanged={() => {
          versions.reload();
          onChanged();
        }}
      />
    </section>
  );
}

/** The editor, opened on an older version. */
function VersionEditor({ name, version, onSaved }: { name: string; version: number; onSaved: () => void }) {
  const v = useLoad(() => api.workflowVersions(name).then((all) => all.find((x) => x.version === version)), [name, version]);
  if (!v.data) return <ErrorNote error={v.error} />;
  return (
    <div className="stack">
      <p className="note">Editing from v{version}. Saving makes a new version.</p>
      <Editor initial={stringify(v.data.spec as WorkflowSpec)} isNew={false} onSaved={onSaved} />
    </div>
  );
}
