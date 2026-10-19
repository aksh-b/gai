import { useEffect, useState } from 'react';
import { BrainIcon, GitForkIcon, HandIcon, LayersIcon, WorkflowIcon, WrenchIcon } from 'lucide-react';
import { Bar, BarChart, XAxis, YAxis } from 'recharts';
import { toast } from 'sonner';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { ChartContainer, ChartTooltip, type ChartConfig } from '@/components/ui/chart';
import { Item, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from '@/components/ui/trace';
import { api, followRun, type EventEnvelope, type GraphNode, type Step } from '../api';
import { Dag, type NodeStatus } from '../Dag';
import { ErrorNote, Json, Markdown, Page, PageHeader, Stat, StatusPill } from '@/components/shared';
import { IconBack, IconChat, IconStop } from '../icons';
import { navigate } from '../router';
import { blocksText, compactJson, duration, money, tokens, totalIn, useLoad } from '../util';
import { modelLabel } from '../labels';
import { useDevMode } from '../prefs';
import { ForkDialog, forkOf } from './Fork';

export function RunDetail({ id }: { id: string }) {
  const [dev] = useDevMode();
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
    <Page wide>
      <Button variant="ghost" size="sm" className="w-fit" onClick={() => navigate('/activity')}>
        <IconBack data-icon="inline-start" />
        History
      </Button>
      <ErrorNote error={run.error} />
      {r && (
        <>
          <PageHeader
            title={
              <>
                {r.agent}
                {dev && r.agent_version && <Badge variant="outline">v{r.agent_version}</Badge>}
                <StatusPill status={r.status} />
              </>
            }
            subtitle={dev ? <span className="font-mono text-xs">{r.id}</span> : undefined}
            actions={
              <>
                <Button variant="outline" onClick={() => navigate(`/chat?session=${r.session_id}`)}>
                  <IconChat data-icon="inline-start" />
                  Open conversation
                </Button>
                {canFork && dev && (
                  <Button variant="outline" onClick={() => setForkFrom(Math.max(1, r.steps))}>
                    <GitForkIcon data-icon="inline-start" />
                    Fork…
                  </Button>
                )}
                {!r.active && ['running', 'queued', 'awaiting_approval'].includes(r.status) && (
                  <Button
                    variant="secondary"
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
                    variant="destructive"
                    onClick={() =>
                      api
                        .cancel(r.id)
                        .then(run.reload)
                        .catch((e: Error) => toast.error(e.message))
                    }
                  >
                    <IconStop data-icon="inline-start" />
                    Stop
                  </Button>
                )}
              </>
            }
          />

          {fork && (
            <Alert>
              <GitForkIcon />
              <AlertDescription>
                <p>
                  Forked from{' '}
                  <a href={`#/activity/${fork.run_id}`} className="font-mono">
                    {fork.run_id}
                  </a>{' '}
                  at step {fork.from_step}: earlier steps were replayed from it.{' '}
                  <a href={`#/activity/${fork.run_id}/compare/${r.id}`}>Compare side by side</a>
                </p>
              </AlertDescription>
            </Alert>
          )}

          <div className="grid gap-3 sm:grid-cols-3 lg:grid-cols-6">
            <Stat
              label={isWorkflow ? 'Kind' : 'Model'}
              value={isWorkflow ? 'workflow' : dev ? <span className="font-mono">{r.metadata.model}</span> : modelLabel(r.metadata.model)}
            />
            <Stat label="Steps" value={r.steps} />
            {dev && <Stat label="Input tokens" value={tokens(totalIn(r.usage))} />}
            {dev && <Stat label="Output tokens" value={tokens(r.usage.output_tokens)} />}
            <Stat label="Cost" value={money(r.cost)} />
            <Stat label="Duration" value={duration(r.created_at, r.finished_at)} />
          </div>

          <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)]">
            <Card>
              <CardHeader>
                <CardTitle>Asked</CardTitle>
              </CardHeader>
              <CardContent>{Array.isArray(r.input) ? <Markdown text={blocksText(r.input)} /> : <Json value={r.input} />}</CardContent>
              <CardHeader>
                <CardTitle>Answer</CardTitle>
              </CardHeader>
              <CardContent className="flex flex-col gap-3">
                <ErrorNote error={r.error ?? undefined} />
                {typeof r.output === 'string' ? (
                  <Markdown text={r.output} />
                ) : r.output ? (
                  <Json value={r.output} />
                ) : (
                  <p className="text-muted-foreground">—</p>
                )}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle>What happened</CardTitle>
              </CardHeader>
              <CardContent>
                <Waterfall
                  steps={steps.data ?? []}
                  start={r.created_at}
                  end={r.finished_at}
                  onFork={canFork ? (n) => setForkFrom(n) : undefined}
                />
              </CardContent>
            </Card>
          </div>

          {isWorkflow && <WorkflowGraph spec={r.metadata.workflow} steps={steps.data ?? []} events={events} />}
          <ChildRuns id={r.id} refresh={events.length} />
          {!isWorkflow && <Forks id={r.id} refresh={r.status} />}
          {r && <ForkDialog run={r} steps={steps.data ?? []} from={forkFrom} onClose={() => setForkFrom(null)} />}

          {dev && (
            <Card>
              <CardHeader>
                <CardTitle>Events</CardTitle>
              </CardHeader>
              <CardContent>
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className="w-24">Time</TableHead>
                      <TableHead className="w-44">Event</TableHead>
                      <TableHead>Detail</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {events.map((e) => (
                      <EventRow key={e.seq} e={e} />
                    ))}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          )}
        </>
      )}
    </Page>
  );
}

const KIND_ICON: Record<string, typeof BrainIcon> = {
  model: BrainIcon,
  tool: WrenchIcon,
  approval: HandIcon,
  node: WorkflowIcon,
  compaction: LayersIcon,
};

const TIMELINE = { time: { label: 'Time', color: 'var(--chart-2)' } } satisfies ChartConfig;

/** Each step as a trace line, with a timeline of when each ran. */
function Waterfall({ steps, start, end, onFork }: { steps: Step[]; start: string; end: string | null; onFork?: (step: number) => void }) {
  if (!steps.length) return <p className="text-muted-foreground">No steps yet.</p>;
  const t0 = new Date(start).getTime();
  const at = (t?: string | null) => (t ? new Date(t).getTime() : Date.now());
  const timeline = steps.map((s) => ({
    label: `${s.kind} ${s.name ?? ''}`.trim(),
    // [start, end] in seconds from the start of the run: a floating bar.
    time: [(at(s.started_at) - t0) / 1000, (at(s.finished_at) - t0) / 1000],
  }));
  const span = Math.max(0.001, (Math.max(at(end), ...steps.map((s) => at(s.finished_at))) - t0) / 1000);
  return (
    <div className="flex flex-col gap-4">
      <ChartContainer config={TIMELINE} className="w-full" style={{ height: Math.max(80, steps.length * 22 + 30) }}>
        <BarChart data={timeline} layout="vertical" margin={{ left: 0, right: 8 }} barCategoryGap={3}>
          <XAxis
            type="number"
            domain={[0, span]}
            tickLine={false}
            axisLine={false}
            tickFormatter={(v: number) => `${v.toFixed(v < 10 ? 1 : 0)}s`}
          />
          <YAxis type="category" dataKey="label" hide />
          <ChartTooltip
            cursor={false}
            content={({ active, payload }) => {
              const p = payload?.[0]?.payload as { label: string; time: [number, number] } | undefined;
              if (!active || !p) return null;
              return (
                <div className="rounded-lg border bg-background px-2.5 py-1.5 text-xs shadow-xl">
                  {p.label}: {(p.time[1] - p.time[0]).toFixed(2)}s
                </div>
              );
            }}
          />
          <Bar dataKey="time" fill="var(--color-time)" radius={3} minPointSize={3} />
        </BarChart>
      </ChartContainer>
      <Trace>
        {steps.map((s) => {
          const Icon = KIND_ICON[s.kind] ?? BrainIcon;
          const n = modelStep(s);
          return (
            <TraceCollapsible key={s.seq}>
              <TraceCollapsibleTrigger>
                <TraceLine status={s.status === 'started' ? 'running' : s.status === 'failed' ? 'error' : undefined}>
                  <TraceIcon>
                    <Icon />
                  </TraceIcon>
                  <TraceTitle className="font-mono">
                    {s.kind}
                    {n !== undefined ? ` #${n}` : ''} {s.name}
                  </TraceTitle>
                  {isReplayed(s) && <TraceDescription>replayed</TraceDescription>}
                  <TraceDescription>{duration(s.started_at, s.finished_at)}</TraceDescription>
                </TraceLine>
              </TraceCollapsibleTrigger>
              <TraceCollapsibleContent className="flex flex-col gap-3">
                {onFork && n !== undefined && (
                  <div className="flex flex-wrap items-center gap-2">
                    <Button variant="secondary" size="sm" onClick={() => onFork(n)}>
                      <GitForkIcon data-icon="inline-start" />
                      Fork from step {n}
                    </Button>
                    <span className="text-sm text-muted-foreground">Replays the steps before it and runs this one live again.</span>
                  </div>
                )}
                {s.input !== null && (
                  <div className="flex flex-col gap-1">
                    <span className="text-xs font-medium text-muted-foreground">Input</span>
                    <Json value={s.input} />
                  </div>
                )}
                {s.output !== null && (
                  <div className="flex flex-col gap-1">
                    <span className="text-xs font-medium text-muted-foreground">
                      Output{s.usage ? ` · ${tokens(totalIn(s.usage))} in / ${tokens(s.usage.output_tokens)} out` : ''}
                    </span>
                    <Json value={s.output} />
                  </div>
                )}
              </TraceCollapsibleContent>
            </TraceCollapsible>
          );
        })}
      </Trace>
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
    <Card>
      <CardHeader>
        <CardTitle>Forks</CardTitle>
      </CardHeader>
      <CardContent>
        <ItemGroup className="gap-1">
          {list.map((f) => {
            const info = forkOf(f);
            return (
              <Item key={f.id} asChild size="xs">
                <a href={`#/activity/${id}/compare/${f.id}`}>
                  <StatusPill status={f.status} />
                  <ItemContent>
                    <ItemTitle>From step {info?.from_step}</ItemTitle>
                    {(info?.overrides.model || info?.overrides.agent_version) && (
                      <ItemDescription className="font-mono">
                        {[info?.overrides.model, info?.overrides.agent_version && `v${info.overrides.agent_version}`]
                          .filter(Boolean)
                          .join(' · ')}
                      </ItemDescription>
                    )}
                  </ItemContent>
                  <span className="text-sm text-muted-foreground">
                    {f.steps} steps · {money(f.cost)} · compare →
                  </span>
                </a>
              </Item>
            );
          })}
        </ItemGroup>
      </CardContent>
    </Card>
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
    <TableRow>
      <TableCell className="font-mono text-xs text-muted-foreground">{new Date(e.ts).toLocaleTimeString()}</TableCell>
      <TableCell className="font-medium capitalize">{e.type.replaceAll('_', ' ')}</TableCell>
      <TableCell className="font-mono text-xs break-all whitespace-normal text-muted-foreground">{detail}</TableCell>
    </TableRow>
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
    <Card>
      <CardHeader>
        <CardTitle>Graph</CardTitle>
      </CardHeader>
      <CardContent>
        <Dag nodes={graph} status={status} />
      </CardContent>
    </Card>
  );
}

/** Runs this run started: delegated agents, or a workflow's agent nodes. */
function ChildRuns({ id, refresh }: { id: string; refresh: number }) {
  const trace = useLoad(() => api.trace(id), [id, Math.floor(refresh / 5)]);
  const kids = trace.data?.children ?? [];
  if (!kids.length) return null;
  return (
    <Card>
      <CardHeader>
        <CardTitle>Child runs</CardTitle>
      </CardHeader>
      <CardContent>
        <ItemGroup className="gap-1">
          {kids.map((c) => (
            <Item key={c.run.id} asChild size="xs">
              <a href={`#/activity/${c.run.id}`}>
                <StatusPill status={c.run.status} />
                <ItemContent>
                  <ItemTitle>{c.run.agent}</ItemTitle>
                  <ItemDescription className="font-mono">
                    {String((c.run.metadata as { child_key?: string } | null)?.child_key ?? '')}
                  </ItemDescription>
                </ItemContent>
                <span className="text-sm text-muted-foreground">
                  {c.run.steps} steps · {money(c.run.cost)} · {duration(c.run.created_at, c.run.finished_at)}
                </span>
              </a>
            </Item>
          ))}
        </ItemGroup>
      </CardContent>
    </Card>
  );
}
