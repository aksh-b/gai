import { useEffect, useMemo, useState } from 'react';
import { FlaskConicalIcon, GitCompareIcon, PlayIcon, ThumbsDownIcon } from 'lucide-react';
import { toast } from 'sonner';
import YAML from 'yaml';
import { CodeEditor } from '@/components/code-editor';
import { SelectField, TabBar } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, LinkItem, Page, PageHeader, Stat } from '@/components/shared';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Checkbox } from '@/components/ui/checkbox';
import { ItemGroup } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type EvalReport, type EvalResult, type EvalRun } from '../api';
import { IconPlus, IconTrash } from '../icons';
import { modelLabel } from '../labels';
import { modelOptions, useModelGroups } from '../models';
import { OptimizePanel } from '../Optimizer';
import { useDevMode } from '../prefs';
import { navigate } from '../router';
import { ago, money, useLoad } from '../util';

const TEMPLATE = `name: my-tests
description: What this assistant must get right.
agent: assistant
threshold: 0.8          # share of cases that must pass
# judge_model: anthropic/claude-sonnet-5-5
cases:
  - id: refund-window
    input: Can I return shoes I bought five weeks ago?
    expected: 30 days    # the answer must contain this
  - id: polite-no
    input: Give me a refund now or else.
    scorers:
      - judge: Declines calmly, explains the policy and offers help.
      - not_contains: [sorry for the inconvenience]
`;

const pct = (n: number) => `${Math.round(n * 100)}%`;

type TargetInfo = { agent?: string; agent_ref?: string; model?: string };
const target = (r: EvalRun) => (r.target ?? {}) as TargetInfo;

type Score = { scorer: string; score: number; passed: boolean; reason?: string | null };
const scoresOf = (r: EvalResult) => (Array.isArray(r.scores) ? (r.scores as Score[]) : []);

/** Assistants › Tests: eval sets, running them, results and comparisons. */
export function Evals({ parts }: { parts: string[] }) {
  if (parts[0] === 'run' && parts[1]) return <ReportPage id={parts[1]} />;
  if (parts[0] === 'compare' && parts[1] && parts[2]) return <ComparePage a={parts[1]} b={parts[2]} />;
  return <SetsPage selected={parts[0] ? decodeURIComponent(parts[0]) : undefined} />;
}

function SetsPage({ selected }: { selected?: string }) {
  const sets = useLoad(() => api.evalSets(), []);
  const creating = selected === 'new';
  return (
    <Page wide>
      <PageHeader
        title="Tests"
        subtitle="Check that an assistant gives the right answers, compare models, and catch a change that makes things worse."
        actions={
          <Button onClick={() => navigate('/evals/new')}>
            <IconPlus data-icon="inline-start" />
            New test set
          </Button>
        }
      />
      <ErrorNote error={sets.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[19rem_minmax(0,1fr)]">
        <ItemGroup className="gap-2">
          {sets.data?.map((s) => (
            <LinkItem
              key={s.name}
              href={`#/evals/${encodeURIComponent(s.name)}`}
              active={s.name === selected}
              title={s.name}
            >
              {`${s.cases} case${s.cases === 1 ? '' : 's'}${s.description ? ` · ${s.description}` : ''}`}
            </LinkItem>
          ))}
          {sets.data?.length === 0 && !creating && (
            <EmptyState title="No tests yet" icon={<FlaskConicalIcon />}>
              Write a few questions with the answers you expect, then run them whenever you change an assistant or try a new model.
            </EmptyState>
          )}
        </ItemGroup>
        {creating ? (
          <SetEditor key="new" initial={TEMPLATE} onSaved={(n) => (sets.reload(), navigate(`/evals/${encodeURIComponent(n)}`))} />
        ) : selected ? (
          <SetPanel key={selected} name={selected} onChanged={sets.reload} />
        ) : null}
      </div>
    </Page>
  );
}

function SetPanel({ name, onChanged }: { name: string; onChanged: () => void }) {
  const set = useLoad(() => api.evalSet(name), [name]);
  const [tab, setTab] = useState('run');
  if (set.error) return <ErrorNote error={set.error} />;
  if (!set.data) return <Spinner />;
  const s = set.data.set;
  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>{s.name}</CardTitle>
          <CardDescription>
            {s.description ?? 'No description.'} {s.cases.length} cases, {pct(s.threshold ?? 0.8)} must pass.
          </CardDescription>
        </CardHeader>
      </Card>
      <TabBar
        items={[
          { id: 'run', label: 'Run' },
          { id: 'results', label: 'Results' },
          { id: 'improve', label: 'Improve' },
          { id: 'edit', label: 'Edit' },
        ]}
        value={tab}
        onValueChange={setTab}
        variant="underline"
        aria-label="Test set"
      />
      {tab === 'run' && <RunForm name={name} defaultAgent={s.agent ?? undefined} onStarted={() => setTab('results')} />}
      {tab === 'results' && <ResultsList name={name} />}
      {tab === 'improve' && <OptimizePanel set={name} defaultAgent={s.agent ?? undefined} />}
      {tab === 'edit' && (
        <>
          <SetEditor
            key={set.data.version}
            initial={YAML.stringify(s)}
            onSaved={() => {
              set.reload();
              onChanged();
            }}
          />
          <ConfirmAction
            title={`Delete ${name}?`}
            description="The test set and every version of it are deleted. Results already run stay."
            action="Delete"
            destructive
            onConfirm={async () => {
              await api.deleteEvalSet(name);
              onChanged();
              navigate('/evals');
            }}
          >
            <Button variant="outline" className="self-start">
              <IconTrash data-icon="inline-start" />
              Delete test set
            </Button>
          </ConfirmAction>
        </>
      )}
    </div>
  );
}

function SetEditor({ initial, onSaved }: { initial: string; onSaved: (name: string) => void }) {
  const [text, setText] = useState(initial);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  async function save() {
    setBusy(true);
    try {
      const r = await api.saveEvalSet(text);
      toast.success(`Saved ${r.name} v${r.version}`);
      setError(undefined);
      onSaved(r.name);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  /** Bad-rated replies of the set's assistant become cases. */
  async function fromFeedback() {
    try {
      const doc = YAML.parse(text) ?? {};
      const agent = doc.agent as string | undefined;
      const bad = await api.feedback({ agent, rating: -1, limit: 20 });
      if (!bad.length) {
        toast.info(agent ? `No thumbs-down replies for ${agent} yet.` : 'No thumbs-down replies yet.');
        return;
      }
      const have = new Set(((doc.cases ?? []) as { id?: string }[]).map((c) => c.id));
      const added: unknown[] = [];
      for (const f of bad) {
        const id = `feedback-${f.run_id.slice(-6)}`;
        if (have.has(id)) continue;
        const run = await api.run(f.run_id);
        const input = typeof run.input === 'string' ? run.input : JSON.stringify(run.input);
        added.push({
          id,
          input,
          scorers: [{ judge: f.comment?.trim() || 'A correct, complete and helpful answer.' }],
          tags: ['from-feedback', ...f.tags],
        });
      }
      doc.cases = [...(doc.cases ?? []), ...added];
      setText(YAML.stringify(doc));
      toast.success(`Added ${added.length} case${added.length === 1 ? '' : 's'}; edit the rubrics, then save.`);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <CodeEditor language="yaml" schema="EvalSet" minHeight="22rem" aria-label="Test set YAML" value={text} onChange={setText} />
      <ErrorNote error={error} />
      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button variant="outline" onClick={fromFeedback}>
          <ThumbsDownIcon data-icon="inline-start" />
          Add thumbs-down replies
        </Button>
        <Button onClick={save} disabled={busy}>
          {busy && <Spinner data-icon="inline-start" />}
          Save
        </Button>
      </div>
    </div>
  );
}

function RunForm({ name, defaultAgent, onStarted }: { name: string; defaultAgent?: string; onStarted: () => void }) {
  const [dev] = useDevMode();
  const agents = useLoad(() => api.agents(), []);
  const groups = useModelGroups();
  const [agent, setAgent] = useState(defaultAgent ?? '');
  const [models, setModels] = useState<string[]>(['']);
  const [busy, setBusy] = useState(false);
  const options = modelOptions(groups, dev);

  async function start() {
    setBusy(true);
    try {
      const chosen = models.filter(Boolean);
      const r = await api.startEval(name, { agent: agent || undefined, models: chosen.length > 1 ? chosen : [], model: chosen.length === 1 ? chosen[0] : undefined });
      toast.success(r.eval_run_ids.length > 1 ? `Running on ${r.eval_run_ids.length} models` : 'Running');
      onStarted();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardContent className="flex flex-col gap-4">
        <SelectField
          label="Assistant"
          value={agent}
          onValueChange={(v) => setAgent(String(v))}
          options={(agents.data ?? []).map((a) => ({ value: a.summary.name, label: a.summary.name }))}
          placeholder="Choose an assistant"
        />
        {models.map((m, i) => (
          <SelectField
            key={i}
            label={i === 0 ? 'Model' : `Compare with`}
            description={i === 0 ? 'Leave empty to use the assistant’s own model.' : undefined}
            value={m}
            onValueChange={(v) => setModels((ms) => ms.map((x, j) => (j === i ? String(v) : x)))}
            options={[{ value: '', label: i === 0 ? 'The assistant’s model' : 'None' }, ...options]}
          />
        ))}
        <div className="flex flex-wrap justify-end gap-2">
          {models.length < 3 && (
            <Button variant="outline" onClick={() => setModels((ms) => [...ms, ''])}>
              <GitCompareIcon data-icon="inline-start" />
              Compare another model
            </Button>
          )}
          <Button onClick={start} disabled={busy || !agent}>
            {busy ? <Spinner data-icon="inline-start" /> : <PlayIcon data-icon="inline-start" />}
            Run tests
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function ResultsList({ name }: { name: string }) {
  const runs = useLoad(() => api.evalRuns(name), [name]);
  const [picked, setPicked] = useState<string[]>([]);
  const running = runs.data?.some((r) => r.status === 'running');
  useEffect(() => {
    if (!running) return;
    const t = setInterval(runs.reload, 1500);
    return () => clearInterval(t);
  }, [running, runs.reload]);
  if (runs.error) return <ErrorNote error={runs.error} />;
  if (!runs.data) return <Spinner />;
  if (!runs.data.length) return <EmptyState title="Not run yet">Run the tests to see how the assistant does.</EmptyState>;
  const toggle = (id: string) => setPicked((p) => (p.includes(id) ? p.filter((x) => x !== id) : [...p, id].slice(-2)));
  return (
    <div className="flex flex-col gap-3">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-8" />
            <TableHead>When</TableHead>
            <TableHead>Assistant</TableHead>
            <TableHead>Model</TableHead>
            <TableHead className="text-right">Passed</TableHead>
            <TableHead className="text-right">Score</TableHead>
            <TableHead className="text-right">Cost</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {runs.data.map((r) => (
            <TableRow key={r.id} className="cursor-pointer" onClick={() => navigate(`/evals/run/${r.id}`)}>
              <TableCell onClick={(e) => e.stopPropagation()}>
                <Checkbox checked={picked.includes(r.id)} onCheckedChange={() => toggle(r.id)} aria-label="Pick to compare" />
              </TableCell>
              <TableCell>{ago(r.created_at)}</TableCell>
              <TableCell>{target(r).agent_ref ?? target(r).agent ?? ''}</TableCell>
              <TableCell>{target(r).model ? modelLabel(target(r).model!) : ''}</TableCell>
              <RunCells run={r} />
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <div className="flex justify-end">
        <Button variant="outline" disabled={picked.length !== 2} onClick={() => navigate(`/evals/compare/${picked[0]}/${picked[1]}`)}>
          <GitCompareIcon data-icon="inline-start" />
          Compare the two picked
        </Button>
      </div>
    </div>
  );
}

function RunCells({ run }: { run: EvalRun }) {
  const s = run.summary as { passed?: number; total?: number; score?: number; cost?: number; ok?: boolean };
  if (run.status === 'running')
    return (
      <TableCell colSpan={3} className="text-right">
        <Spinner className="inline" /> running
      </TableCell>
    );
  return (
    <>
      <TableCell className="text-right">
        <Badge variant={s.ok ? 'secondary' : 'destructive'}>
          {s.passed}/{s.total}
        </Badge>
      </TableCell>
      <TableCell className="text-right tabular-nums">{(s.score ?? 0).toFixed(2)}</TableCell>
      <TableCell className="text-right tabular-nums">{money(s.cost ?? 0)}</TableCell>
    </>
  );
}

function ReportPage({ id }: { id: string }) {
  const rep = useLoad(() => api.evalReport(id), [id]);
  const running = rep.data?.run.status === 'running';
  useEffect(() => {
    if (!running) return;
    const t = setInterval(rep.reload, 1500);
    return () => clearInterval(t);
  }, [running, rep.reload]);
  if (rep.error) return <ErrorNote error={rep.error} />;
  if (!rep.data) return <Spinner />;
  const r = rep.data;
  return (
    <Page wide>
      <PageHeader
        title={`${r.run.eval_set} on ${target(r.run).agent_ref ?? target(r.run).agent}`}
        subtitle={`${target(r.run).model ? `${modelLabel(target(r.run).model!)} · ` : ''}${ago(r.run.created_at)}${running ? ' · running' : ''}`}
        actions={
          <Button variant="outline" onClick={() => navigate(`/evals/${encodeURIComponent(r.run.eval_set)}`)}>
            Back to the tests
          </Button>
        }
      />
      <SummaryStats report={r} />
      <CaseTable results={r.results} />
    </Page>
  );
}

function SummaryStats({ report }: { report: EvalReport }) {
  const s = report.summary;
  return (
    <>
      {report.run.status !== 'running' && (
        <Alert variant={s.ok ? 'default' : 'destructive'}>
          <AlertDescription>
            {s.ok ? 'Passed' : 'Below the bar'}: {s.passed} of {s.total} cases passed; {pct(s.threshold)} needed.
          </AlertDescription>
        </Alert>
      )}
      <div className="grid gap-3 sm:grid-cols-4">
        <Stat label="Passed" value={`${s.passed}/${s.total}`} />
        <Stat label="Score" value={s.score.toFixed(2)} />
        <Stat label="Cost" value={money(s.cost)} />
        <Stat label="Time per case" value={`${s.mean_latency_ms} ms`} />
      </div>
    </>
  );
}

function CaseTable({ results }: { results: EvalResult[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Case</TableHead>
          <TableHead>Result</TableHead>
          <TableHead>Answer</TableHead>
          <TableHead>Why</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {results.map((x) => {
          const failed = scoresOf(x).filter((s) => !s.passed);
          return (
            <TableRow key={x.case_id} className="align-top">
              <TableCell className="font-medium">
                {x.run_id ? (
                  <a href={`#/activity/${x.run_id}`} title="Open the run">
                    {x.case_id}
                  </a>
                ) : (
                  x.case_id
                )}
              </TableCell>
              <TableCell>
                <Badge variant={x.passed ? 'secondary' : 'destructive'}>{x.passed ? 'pass' : 'fail'}</Badge>{' '}
                <span className="text-muted-foreground tabular-nums">{x.score.toFixed(2)}</span>
              </TableCell>
              <TableCell className="max-w-md whitespace-pre-wrap text-sm">{(x.output ?? '').slice(0, 400)}</TableCell>
              <TableCell className="max-w-sm text-sm text-muted-foreground">
                {x.error ?? failed.map((s) => `${s.scorer}: ${s.reason ?? 'failed'}`).join(' · ')}
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

function ComparePage({ a, b }: { a: string; b: string }) {
  const cmp = useLoad(() => api.compareEvals(a, b), [a, b]);
  const delta = (d: number) => (
    <span className={d > 1e-9 ? 'text-success' : d < -1e-9 ? 'text-destructive' : 'text-muted-foreground'}>
      {d > 0 ? '+' : ''}
      {d.toFixed(2)}
    </span>
  );
  const rows = useMemo(() => cmp.data?.cases ?? [], [cmp.data]);
  if (cmp.error) return <ErrorNote error={cmp.error} />;
  if (!cmp.data) return <Spinner />;
  const c = cmp.data;
  return (
    <Page wide>
      <PageHeader title="Compare tests" subtitle="The first one picked is the baseline." />
      <Alert variant={c.verdict.ok ? 'default' : 'destructive'}>
        <AlertDescription>{c.verdict.ok ? 'No regression.' : c.verdict.reasons.join(' ')}</AlertDescription>
      </Alert>
      <div className="grid gap-3 sm:grid-cols-4">
        <Stat label="Score" value={<>{c.baseline.score.toFixed(2)} → {c.candidate.score.toFixed(2)} {delta(c.score_delta)}</>} />
        <Stat label="Passed" value={`${c.baseline.passed} → ${c.candidate.passed}`} />
        <Stat label="Cost" value={`${money(c.baseline.cost)} → ${money(c.candidate.cost)}`} />
        <Stat label="Newly failing" value={c.regressions.length ? c.regressions.join(', ') : 'none'} />
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Case</TableHead>
            <TableHead className="text-right">Baseline</TableHead>
            <TableHead className="text-right">This one</TableHead>
            <TableHead className="text-right">Change</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.case_id}>
              <TableCell>{r.case_id}</TableCell>
              <TableCell className="text-right">{r.baseline == null ? '—' : `${r.baseline_passed ? 'pass' : 'fail'} ${r.baseline.toFixed(2)}`}</TableCell>
              <TableCell className="text-right">{r.candidate == null ? '—' : `${r.candidate_passed ? 'pass' : 'fail'} ${r.candidate.toFixed(2)}`}</TableCell>
              <TableCell className="text-right">{delta(r.delta)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Page>
  );
}
