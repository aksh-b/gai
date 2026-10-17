import { useEffect, useState } from 'react';
import { Button, toast } from '@flame';
import { api, followRun, type EventEnvelope, type GraphNode, type Step } from '../api';
import { Dag, type NodeStatus } from '../Dag';
import { Disclosure, ErrorNote, Json, Markdown, Stat, StatusPill } from '../components';
import { IconBack, IconChat, IconStop } from '../icons';
import { navigate } from '../router';
import { blocksText, compactJson, duration, money, tokens, totalIn, useLoad } from '../util';
import { ForkDialog, forkOf } from './Fork';

export function RunDetail({ id }: { id: string }) {
  const run = useLoad(() => api.run(id), [id]);
  const steps = useLoad(() => api.steps(id), [id]);
  const [events, setEvents] = useState<EventEnvelope[]>([]);
  const [forkFrom, setForkFrom] = useState<number | null>(null);

  // Stored events, then live ones while the run is going.
  useEffect(() => {
    setEvents([]);
    const abort = new AbortController();
    followRun(
      id,
      (e) => {
        if (e.type === 'text_delta' || e.type === 'reasoning_delta') return;
        setEvents((prev) => [...prev, e]);
        if (e.type !== 'step_started') {
          steps.reload();
        }
        if (e.type === 'run_finished' || e.type === 'approval_required' || e.type === 'approval_resolved') run.reload();
      },
      abort.signal,
    ).catch(() => {});
    return () => abort.abort();
  }, [id]);

  const r = run.data;
  const isWorkflow = r?.metadata.kind === 'workflow';
  const fork = r ? forkOf(r) : undefined;
  const canFork = !!r && !isWorkflow && !r.active && !!r.metadata.spec;
  return (
    <div className="page">
      <div className="row gap">
        <Button variant="ghost" size="sm" leading={<IconBack />} onClick={() => navigate('/runs')}>
          Runs
        </Button>
      </div>
      <ErrorNote error={run.error} />
      {r && (
        <>
          <header className="page-head">
            <div>
              <h1 className="row gap">
                {r.agent}
                {r.agent_version && <span className="muted small">v{r.agent_version}</span>}
                <StatusPill status={r.status} />
              </h1>
              <p className="muted mono small">{r.id}</p>
            </div>
            <div className="row gap">
              <Button variant="outline" leading={<IconChat />} onClick={() => navigate(`/playground?session=${r.session_id}`)}>
                Open conversation
              </Button>
              {canFork && (
                <Button variant="outline" onClick={() => setForkFrom(Math.max(1, r.steps))}>
                  Fork…
                </Button>
              )}
              {!r.active && ['running', 'queued', 'awaiting_approval'].includes(r.status) && (
                <Button
                  tone="accent"
                  variant="soft"
                  onClick={() =>
                    api
                      .resume(r.id)
                      .then(() => location.reload())
                      .catch((e: Error) => toast.error(e.message))
                  }
                >
                  Resume
                </Button>
              )}
              {r.active && (
                <Button
                  tone="danger"
                  variant="soft"
                  leading={<IconStop />}
                  onClick={() => api.cancel(r.id).then(run.reload).catch((e: Error) => toast.error(e.message))}
                >
                  Cancel
                </Button>
              )}
            </div>
          </header>

          {fork && (
            <p className="note fork-note">
              Forked from{' '}
              <a href={`#/runs/${fork.run_id}`} className="mono">
                {fork.run_id}
              </a>{' '}
              at step {fork.from_step}: earlier steps were replayed from it.{' '}
              <a href={`#/runs/${fork.run_id}/compare/${r.id}`}>Compare side by side</a>
            </p>
          )}

          <div className="stats">
            <Stat label={isWorkflow ? 'Kind' : 'Model'} value={<span className="mono">{isWorkflow ? 'workflow' : r.metadata.model}</span>} />
            <Stat label="Steps" value={r.steps} />
            <Stat label="Input tokens" value={tokens(totalIn(r.usage))} />
            <Stat label="Output tokens" value={tokens(r.usage.output_tokens)} />
            <Stat label="Cost" value={money(r.cost)} />
            <Stat label="Duration" value={duration(r.created_at, r.finished_at)} />
          </div>

          <div className="grid-2">
            <section className="card">
              <h3>Input</h3>
              {Array.isArray(r.input) ? <Markdown text={blocksText(r.input)} /> : <Json value={r.input} />}
              <h3>Output</h3>
              {r.error && <div className="error-note">{r.error}</div>}
              {typeof r.output === 'string' ? <Markdown text={r.output} /> : r.output ? <Json value={r.output} /> : <p className="muted">—</p>}
            </section>
            <section className="card">
              <h3>Steps</h3>
              <Waterfall
                steps={steps.data ?? []}
                start={r.created_at}
                end={r.finished_at}
                onFork={canFork ? (n) => setForkFrom(n) : undefined}
              />
            </section>
          </div>

          {isWorkflow && <WorkflowGraph spec={r.metadata.workflow} steps={steps.data ?? []} events={events} />}
          <ChildRuns id={r.id} refresh={events.length} />
          {!isWorkflow && <Forks id={r.id} refresh={r.status} />}
          {r && <ForkDialog run={r} steps={steps.data ?? []} from={forkFrom} onClose={() => setForkFrom(null)} />}

          <section className="card">
            <h3>Events</h3>
            <ol className="events">
              {events.map((e) => (
                <EventRow key={e.seq} e={e} />
              ))}
            </ol>
          </section>
        </>
      )}
    </div>
  );
}

function Waterfall({
  steps,
  start,
  end,
  onFork,
}: {
  steps: Step[];
  start: string;
  end: string | null;
  onFork?: (step: number) => void;
}) {
  if (!steps.length) return <p className="muted">No steps yet.</p>;
  const t0 = new Date(start).getTime();
  const t1 = Math.max(
    end ? new Date(end).getTime() : Date.now(),
    ...steps.map((s) => (s.finished_at ? new Date(s.finished_at).getTime() : Date.now())),
  );
  const span = Math.max(1, t1 - t0);
  return (
    <div className="waterfall">
      {steps.map((s) => {
        const a = new Date(s.started_at).getTime() - t0;
        const b = (s.finished_at ? new Date(s.finished_at).getTime() : Date.now()) - t0;
        return (
          <Disclosure
            key={s.seq}
            summary={
              <span className="wf-row">
                <span className="wf-label">
                  <span className="wf-kind" data-kind={s.kind}>
                    {s.kind}
                  </span>
                  {modelStep(s) !== undefined && <span className="muted small">#{modelStep(s)}</span>}
                  {isReplayed(s) && <span className="chip small">replayed</span>}
                  <span className="mono small">{s.name}</span>
                </span>
                <span className="wf-track">
                  <span
                    className="wf-bar"
                    data-kind={s.kind}
                    data-status={s.status}
                    style={{ left: `${(a / span) * 100}%`, width: `max(3px, ${((b - a) / span) * 100}%)` }}
                  />
                </span>
                <span className="muted small wf-time">{duration(s.started_at, s.finished_at)}</span>
              </span>
            }
          >
            {onFork && modelStep(s) !== undefined && (
              <div className="row gap">
                <Button size="sm" variant="soft" onClick={() => onFork(modelStep(s)!)}>
                  Fork from step {modelStep(s)}
                </Button>
                <span className="muted small">Replays the steps before it and runs this one live again.</span>
              </div>
            )}
            <div className="tool-io">
              {s.input !== null && (
                <div>
                  <div className="label">Input</div>
                  <Json value={s.input} />
                </div>
              )}
              {s.output !== null && (
                <div>
                  <div className="label">Output{s.usage ? ` · ${tokens(totalIn(s.usage))} in / ${tokens(s.usage.output_tokens)} out` : ''}</div>
                  <Json value={s.output} />
                </div>
              )}
            </div>
          </Disclosure>
        );
      })}
    </div>
  );
}

/** The model step number of a model step. */
function modelStep(s: Step): number | undefined {
  return s.kind === 'model' ? ((s.input as { step?: number } | null)?.step ?? undefined) : undefined;
}

function isReplayed(s: Step): boolean {
  return !!(s.input as { replayed_from?: string } | null)?.replayed_from;
}

/** Runs forked from this one. */
function Forks({ id, refresh }: { id: string; refresh: string }) {
  const forks = useLoad(() => api.runs({ fork_of: id, limit: 50 }), [id, refresh]);
  const list = forks.data ?? [];
  if (!list.length) return null;
  return (
    <section className="card flush">
      <h3 className="pad" style={{ margin: 0 }}>
        Forks
      </h3>
      {list.map((f) => {
        const info = forkOf(f);
        return (
          <a
            key={f.id}
            className="run-row"
            href={`#/runs/${id}/compare/${f.id}`}
            onClick={(e) => {
              e.preventDefault();
              navigate(`/runs/${id}/compare/${f.id}`);
            }}
          >
            <StatusPill status={f.status} />
            <span>
              from step {info?.from_step}
              {info?.overrides.model && <span className="muted small mono"> · {info.overrides.model}</span>}
              {info?.overrides.agent_version && <span className="muted small"> · v{info.overrides.agent_version}</span>}
            </span>
            <span className="muted small">{f.steps} steps</span>
            <span className="muted small num">{money(f.cost)}</span>
            <span className="muted small">compare →</span>
          </a>
        );
      })}
    </section>
  );
}

function EventRow({ e }: { e: EventEnvelope }) {
  let detail = '';
  switch (e.type) {
    case 'run_started':
      detail = `${e.agent} on ${e.model}`;
      break;
    case 'tool_call':
      detail = `${e.name} ${compactJson(e.input, 100)}`;
      break;
    case 'tool_result':
      detail = `${e.name} → ${compactJson(blocksText(e.output.content), 100)} (${e.duration_ms} ms)`;
      break;
    case 'approval_required':
      detail = `${e.name} is waiting for approval`;
      break;
    case 'approval_resolved':
      detail = e.approved ? 'approved' : `declined${e.reason ? `: ${e.reason}` : ''}`;
      break;
    case 'usage':
      detail = `${tokens(totalIn(e.usage))} in / ${tokens(e.usage.output_tokens)} out${e.cost ? ` · ${money(e.cost)}` : ''}`;
      break;
    case 'assistant_message':
      detail = compactJson(blocksText(e.message.content) || `${e.message.content.length} blocks`, 120);
      break;
    case 'warning':
      detail = e.message;
      break;
    case 'run_finished':
      detail = `${e.status}${e.error ? `: ${e.error}` : ''}`;
      break;
    case 'step_started':
      detail = `step ${e.step}`;
      break;
    case 'step_replayed':
      detail = `step ${e.step} from ${e.from_run_id}`;
      break;
    case 'model_routed':
      detail = `step ${e.step} → ${e.model}: ${e.reason}`;
      break;
    case 'workflow_started':
      detail = `${e.workflow}${e.version ? ` v${e.version}` : ''} · ${compactJson(e.inputs, 100)}`;
      break;
    case 'node_started':
      detail = `${e.node} (${e.kind})`;
      break;
    case 'node_finished':
      detail = `${e.node} ${e.status}${e.error ? `: ${e.error}` : e.output !== undefined ? ` → ${compactJson(e.output, 90)}` : ''} (${e.duration_ms} ms)`;
      break;
  }
  return (
    <li className="event" data-type={e.type}>
      <span className="mono small muted">{new Date(e.ts).toLocaleTimeString()}</span>
      <span className="event-type">{e.type.replaceAll('_', ' ')}</span>
      <span className="event-detail">{detail}</span>
    </li>
  );
}

/** A workflow run's graph, coloured by what its nodes did. */
function WorkflowGraph({ spec, steps, events }: { spec: unknown; steps: Step[]; events: EventEnvelope[] }) {
  const [graph, setGraph] = useState<GraphNode[]>([]);
  useEffect(() => {
    api
      .validateWorkflow(spec)
      .then((v) => setGraph(v.graph))
      .catch(() => {});
  }, [JSON.stringify(spec)]);
  let status: Record<string, NodeStatus> = {};
  for (const s of steps) {
    if (s.kind !== 'node' || !s.name) continue;
    const out = s.output as { status?: NodeStatus } | null;
    status[s.name] = s.status === 'started' ? 'running' : (out?.status ?? 'completed');
  }
  // Waiting for a person shows only in events: a running node with an
  // approval asked for and not yet answered.
  const open = new Set<string>();
  for (const e of events) {
    if (e.type === 'approval_required') open.add(e.id);
    if (e.type === 'approval_resolved') open.delete(e.id);
  }
  for (const id of open) {
    const node = id.split(/[[~]/)[0];
    if (status[node] === 'running') status = { ...status, [node]: 'waiting' };
  }
  return (
    <section className="card">
      <h3>Graph</h3>
      <Dag nodes={graph} status={status} />
    </section>
  );
}

/** Runs this run started: delegated agents, or a workflow's agent nodes. */
function ChildRuns({ id, refresh }: { id: string; refresh: number }) {
  const trace = useLoad(() => api.trace(id), [id, Math.floor(refresh / 5)]);
  const kids = trace.data?.children ?? [];
  if (!kids.length) return null;
  return (
    <section className="card flush">
      <h3 className="pad" style={{ margin: 0 }}>
        Child runs
      </h3>
      {kids.map((c) => (
        <a
          key={c.run.id}
          className="run-row"
          href={`#/runs/${c.run.id}`}
          onClick={(e) => {
            e.preventDefault();
            navigate(`/runs/${c.run.id}`);
          }}
        >
          <StatusPill status={c.run.status} />
          <span>
            <strong>{c.run.agent}</strong>{' '}
            <span className="muted small mono">{String((c.run.metadata as { child_key?: string } | null)?.child_key ?? '')}</span>
          </span>
          <span className="muted small">{c.run.steps} steps</span>
          <span className="muted small num">{money(c.run.cost)}</span>
          <span className="muted small">{duration(c.run.created_at, c.run.finished_at)}</span>
        </a>
      ))}
    </section>
  );
}
