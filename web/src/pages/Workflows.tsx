import { useEffect, useRef, useState } from 'react';
import { HandIcon, HistoryIcon, InfoIcon, WorkflowIcon } from 'lucide-react';
import { toast } from 'sonner';
import { TabBar, TextField } from '@/components/fields';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldGroup } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Item, ItemContent, ItemDescription, ItemFooter, ItemGroup, ItemMedia, ItemTitle } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { Textarea } from '@/components/ui/textarea';
import { parse, stringify } from 'yaml';
import { api, followRun, type GraphNode, type Template, type WorkflowSpec, type WorkflowView } from '../api';
import { ConfirmAction, EmptyState, ErrorNote, Json, LinkItem, Page, PageHeader, StatusPill } from '@/components/shared';
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
    <Page wide>
      <PageHeader
        title="Workflows"
        subtitle={
          <>
            Several steps in a row: assistants, tools and your OK. Steps run as soon as what they need is ready, and a workflow picks up
            where it stopped if the server restarts.
          </>
        }
        actions={
          <Button onClick={() => setCreating('pick')}>
            <IconPlus data-icon="inline-start" />
            New workflow
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[19rem_minmax(0,1fr)]">
        <ItemGroup className="gap-2">
          {list.data?.map((w) => (
            <LinkItem
              key={w.name}
              href={`#/automations/${encodeURIComponent(w.name)}`}
              active={current === w.name}
              onClick={() => setCreating(false)}
              title={
                <>
                  <span className="truncate">{w.name}</span>
                  {dev && (
                    <Badge variant="outline" className="ml-auto">
                      {w.source === 'stored' ? `v${w.latest_version}` : 'file'}
                    </Badge>
                  )}
                </>
              }
            >
              {w.description && <ItemDescription>{w.description}</ItemDescription>}
              {dev && Object.keys(w.labels).length > 0 && (
                <div className="flex flex-wrap gap-1 pt-1">
                  {Object.entries(w.labels).map(([l, v]) => (
                    <Badge key={l} variant="outline">
                      {l}=v{v}
                    </Badge>
                  ))}
                </div>
              )}
            </LinkItem>
          ))}
          {list.data?.length === 0 && !creating && (
            <EmptyState title="No workflows yet" icon={<WorkflowIcon />}>
              Start from a template, or make your own.
            </EmptyState>
          )}
        </ItemGroup>
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
    </Page>
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
    <Card>
      <CardHeader>
        <CardTitle>New workflow</CardTitle>
        <CardDescription>Start from a template, or build one step by step.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <ErrorNote error={templates.error} />
        <ItemGroup className="grid gap-2 sm:grid-cols-2">
          {(templates.data ?? [])
            .filter((t) => t.kind === 'workflow')
            .map((t) => (
              <Item key={t.id} asChild variant="outline">
                <button type="button" className="text-left" onClick={() => install(t)} disabled={busy !== null}>
                  <ItemContent>
                    <ItemTitle>
                      {t.title}
                      {t.installed ? (
                        <Badge variant="secondary">Already added</Badge>
                      ) : (
                        busy === t.id && (
                          <Badge variant="outline">
                            <Spinner data-icon="inline-start" />
                            Adding
                          </Badge>
                        )
                      )}
                    </ItemTitle>
                    <ItemDescription>{t.description}</ItemDescription>
                  </ItemContent>
                </button>
              </Item>
            ))}
          <Item asChild variant="outline">
            <button type="button" className="text-left" onClick={onBlank}>
              <ItemContent>
                <ItemTitle>Build your own</ItemTitle>
                <ItemDescription>Start with two simple steps and change them.</ItemDescription>
              </ItemContent>
            </button>
          </Item>
        </ItemGroup>
      </CardContent>
    </Card>
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
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg">
            {w.name}
            {dev && w.version != null && <Badge variant="outline">v{w.version}</Badge>}
          </CardTitle>
          {w.spec.description && <CardDescription>{w.spec.description}</CardDescription>}
          {w.version != null && (
            <CardAction>
              <ConfirmAction
                title={`Delete ${w.name}?`}
                description="All its versions are deleted too. Runs it already made are kept."
                action="Delete"
                destructive
                onConfirm={async () => {
                  await api.deleteWorkflow(w.name).catch((e: Error) => toast.error(e.message));
                  navigate('/automations');
                  onChange();
                }}
              >
                <Button variant="ghost" size="sm">
                  <IconTrash data-icon="inline-start" />
                  Delete
                </Button>
              </ConfirmAction>
            </CardAction>
          )}
        </CardHeader>
      </Card>
      <TabBar
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
      {tab === 'edit' && viewing !== undefined && (
        <VersionEditor
          name={w.name}
          version={viewing}
          onSaved={() => {
            setViewing(undefined);
            wf.reload();
            onChange();
            setTab('run');
          }}
        />
      )}
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
    </div>
  );
}

function RunPanel({ w }: { w: WorkflowView }) {
  const inputs = Object.entries(w.spec.inputs ?? {});
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      inputs.map(([k, s]) => [
        k,
        s.default === undefined || s.default === null ? '' : typeof s.default === 'string' ? s.default : JSON.stringify(s.default),
      ]),
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
    <div className="flex flex-col gap-4">
      <Card>
        {inputs.length > 0 && (
          <CardContent>
            <FieldGroup className="grid gap-4 sm:grid-cols-2">
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
            </FieldGroup>
          </CardContent>
        )}
        <CardFooter className="gap-2">
          <Button onClick={start}>Run workflow</Button>
          {runId && (
            <Button variant="link" size="sm" onClick={() => navigate(`/activity/${runId}`)}>
              Open run {runId.slice(-8)}
            </Button>
          )}
        </CardFooter>
      </Card>

      <Card>
        <CardContent className="flex flex-col gap-3">
          <Dag nodes={w.graph as GraphNode[]} status={status} selected={selected} onSelect={setSelected} />
          {selected && (
            <Item variant="muted">
              <ItemContent className="min-w-0 gap-2">
                <ItemTitle className="font-mono">{selected}</ItemTitle>
                <ErrorNote error={outputs[selected]?.error} />
                {outputs[selected]?.output !== undefined ? (
                  <Json value={outputs[selected].output} />
                ) : (
                  <ItemDescription>No output yet.</ItemDescription>
                )}
              </ItemContent>
            </Item>
          )}
        </CardContent>
      </Card>

      {pending.map((p) => (
        <Waiting key={p.id} runId={runId!} p={p} />
      ))}

      {result && (
        <Card>
          <CardHeader>
            <CardTitle>Result</CardTitle>
            <CardAction>
              <StatusPill status={result.status as never} />
            </CardAction>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <ErrorNote error={result.error} />
            {result.output !== undefined && <Json value={result.output} />}
          </CardContent>
        </Card>
      )}
    </div>
  );
}

function Waiting({ runId, p }: { runId: string; p: { id: string; name: string; input: unknown } }) {
  const [reply, setReply] = useState('');
  const prompt = p.name === 'human' ? String((p.input as { prompt?: string })?.prompt ?? '') : null;
  return (
    <Item variant="outline">
      <ItemMedia variant="icon">
        <HandIcon />
      </ItemMedia>
      <ItemContent className="min-w-0">
        <ItemTitle>{prompt !== null ? 'Waiting for you' : `${p.name} needs your OK`}</ItemTitle>
        {prompt !== null ? <p className="text-sm whitespace-pre-wrap">{prompt}</p> : <Json value={p.input} />}
      </ItemContent>
      <ItemFooter className="justify-start gap-2">
        {prompt !== null && (
          <Input
            className="max-w-sm"
            placeholder="Optional reply"
            aria-label="Reply"
            value={reply}
            onChange={(e) => setReply(e.target.value)}
          />
        )}
        <Button
          size="sm"
          onClick={() =>
            (reply.trim() ? api.decideWith(runId, p.id, { reply: reply.trim() }) : api.decide(runId, p.id, true)).catch((e: Error) =>
              toast.error(e.message),
            )
          }
        >
          Continue
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => api.decide(runId, p.id, false, 'declined in the dashboard').catch((e: Error) => toast.error(e.message))}
        >
          Stop here
        </Button>
      </ItemFooter>
    </Item>
  );
}

function RecentRuns({ name }: { name: string }) {
  const runs = useLoad(() => api.workflowRuns(name), [name]);
  if (runs.data?.length === 0) return <EmptyState title="No runs yet" icon={<HistoryIcon />} />;
  return (
    <Card>
      <CardContent>
        <ItemGroup className="gap-1">
          {runs.data?.map((r) => (
            <Item key={r.id} asChild size="xs">
              <a href={`#/activity/${r.id}`}>
                <StatusPill status={r.status} />
                <ItemContent>
                  <ItemTitle>{new Date(r.created_at).toLocaleString()}</ItemTitle>
                </ItemContent>
                <span className="text-sm text-muted-foreground">
                  {r.steps} steps · {money(r.cost)} · {ago(r.created_at)}
                </span>
              </a>
            </Item>
          ))}
        </ItemGroup>
      </CardContent>
    </Card>
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
    <div className="flex flex-col gap-4">
      {showTabs && (
        <TabBar
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
      {!formOk && (
        <Alert>
          <InfoIcon />
          <AlertDescription>This workflow uses steps the form can't show yet, so it opens as YAML.</AlertDescription>
        </Alert>
      )}
      {mode === 'form' && formOk ? (
        <WorkflowForm spec={spec} onChange={fromForm} isNew={isNew} />
      ) : (
        <Textarea
          className="min-h-96 font-mono"
          aria-label="Workflow YAML"
          spellCheck={false}
          value={text}
          onChange={(e) => fromYaml(e.target.value)}
        />
      )}
      {graph.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Preview</CardTitle>
          </CardHeader>
          <CardContent>
            <Dag nodes={graph} />
          </CardContent>
        </Card>
      )}
      <ErrorNote error={error} />
      <div className="flex items-center justify-end gap-2">
        {dev && (
          <Input
            className="max-w-64"
            placeholder="Version note (optional)"
            aria-label="Version note"
            value={note}
            onChange={(e) => setNote(e.target.value)}
          />
        )}
        <Button onClick={save} disabled={busy || (!!error && !error.startsWith('Give'))}>
          {busy && <Spinner data-icon="inline-start" />}
          {isNew ? 'Create workflow' : 'Save changes'}
        </Button>
      </div>
    </div>
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
    <Card>
      <CardContent className="flex flex-col gap-3">
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
      </CardContent>
    </Card>
  );
}

/** The editor, opened on an older version. */
function VersionEditor({ name, version, onSaved }: { name: string; version: number; onSaved: () => void }) {
  const v = useLoad(() => api.workflowVersions(name).then((all) => all.find((x) => x.version === version)), [name, version]);
  if (!v.data) return <ErrorNote error={v.error} />;
  return (
    <div className="flex flex-col gap-4">
      <Alert>
        <HistoryIcon />
        <AlertDescription>Editing from v{version}. Saving makes a new version.</AlertDescription>
      </Alert>
      <Editor initial={stringify(v.data.spec as WorkflowSpec)} isNew={false} onSaved={onSaved} />
    </div>
  );
}
