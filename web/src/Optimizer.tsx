// The prompt optimizer for one test set: rewrite an assistant's
// instructions (or the prompt it uses) to score higher, see each attempt,
// and save the best as a new version.

import { useEffect, useState } from 'react';
import { SparklesIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField } from '@/components/fields';
import { ErrorNote, Stat } from '@/components/shared';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type Optimization } from './api';
import { NumberInput } from './forms';
import { modelOptions, useModelGroups } from './models';
import { useDevMode } from './prefs';
import { ago, useLoad } from './util';

const pct = (n: number) => `${Math.round(n * 100)}%`;

export function OptimizePanel({ set, defaultAgent }: { set: string; defaultAgent?: string }) {
  const [dev] = useDevMode();
  const agents = useLoad(() => api.agents(), []);
  const groups = useModelGroups();
  const past = useLoad(() => api.optimizations().then((all) => all.filter((o) => o.eval_set === set)), [set]);
  const [agent, setAgent] = useState(defaultAgent ?? '');
  const [writer, setWriter] = useState('');
  const [rounds, setRounds] = useState<number | undefined>(3);
  const [candidates, setCandidates] = useState<number | undefined>(2);
  const [current, setCurrent] = useState<string>();
  const [busy, setBusy] = useState(false);
  const shown = current ?? past.data?.[0]?.id;

  async function start() {
    setBusy(true);
    try {
      const r = await api.optimize(set, {
        agent: agent || undefined,
        writer_model: writer || undefined,
        rounds: rounds ?? 3,
        candidates: candidates ?? 2,
      } as Parameters<typeof api.optimize>[1]);
      setCurrent(r.id);
      past.reload();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>Improve the instructions</CardTitle>
          <CardDescription>
            Runs these tests, shows a model the cases that failed, and has it rewrite the assistant’s instructions (or the prompt it
            uses). Each rewrite is tested; only one that scores higher is kept. Nothing changes until you save it.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4 sm:grid-cols-2">
          <SelectField
            label="Assistant"
            value={agent}
            onValueChange={setAgent}
            options={(agents.data ?? []).map((a) => ({ value: a.summary.name, label: a.summary.name }))}
            placeholder="Choose an assistant"
          />
          <SelectField
            label="Model that rewrites"
            description="A strong model works best."
            value={writer}
            onValueChange={setWriter}
            options={[{ value: '', label: 'The default model' }, ...modelOptions(groups, dev)]}
          />
          <NumberInput label="Rounds" min={1} value={rounds} onChange={setRounds} />
          <NumberInput label="Rewrites per round" min={1} value={candidates} onChange={setCandidates} />
        </CardContent>
        <CardFooter className="justify-end">
          <Button onClick={start} disabled={busy || !agent}>
            {busy ? <Spinner data-icon="inline-start" /> : <SparklesIcon data-icon="inline-start" />}
            Start
          </Button>
        </CardFooter>
      </Card>
      {(past.data?.length ?? 0) > 1 && (
        <SelectField
          label="Earlier runs"
          value={shown ?? ''}
          onValueChange={setCurrent}
          options={(past.data ?? []).map((o) => ({
            value: o.id,
            label: `${o.agent ?? '?'} · ${ago(o.created_at)}`,
            description: o.status,
          }))}
        />
      )}
      {shown && <Progress key={shown} id={shown} onChange={past.reload} />}
    </div>
  );
}

function Progress({ id, onChange }: { id: string; onChange: () => void }) {
  const [o, setO] = useState<Optimization>();
  const [error, setError] = useState<string>();
  const [note, setNote] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let stop = false;
    let t: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await api.optimization(id);
        if (stop) return;
        setO(next);
        if (next.status === 'running') t = setTimeout(poll, 2000);
        else onChange();
      } catch (e) {
        setError((e as Error).message);
      }
    };
    poll();
    return () => {
      stop = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  async function apply() {
    setSaving(true);
    try {
      const r = await api.applyOptimization(id, note);
      toast.success(`Saved ${r.name} v${r.version}`);
      setO(await api.optimization(id));
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setSaving(false);
    }
  }

  if (error) return <ErrorNote error={error} />;
  if (!o) return <Spinner />;
  const r = o.report;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          {o.status === 'running' && <Spinner />}
          {o.status === 'running' ? 'Working on it…' : o.status === 'failed' ? 'Stopped' : r?.improved ? 'Found better instructions' : 'No improvement found'}
        </CardTitle>
        <CardDescription>
          {o.agent}
          {r?.prompt ? ` · rewriting the prompt ${r.prompt}` : ''} · started {ago(o.created_at)}
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <ErrorNote error={o.error ?? undefined} />
        {r && (
          <>
            <div className="flex flex-wrap gap-6">
              <Stat label="As it is" value={`${pct(r.baseline.score)} (${r.baseline.passed}/${r.baseline.total})`} />
              <Stat label="Best" value={`${pct(r.best.score)} (${r.best.passed}/${r.best.total})`} />
              <Stat label="Rewrites tried" value={r.attempts.length} />
            </div>
            {r.attempts.length > 0 && (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Round</TableHead>
                    <TableHead>Score</TableHead>
                    <TableHead>What changed</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {r.attempts.map((a, i) => (
                    <TableRow key={i}>
                      <TableCell>{a.round}</TableCell>
                      <TableCell>
                        {a.error ? '—' : `${pct(a.score)} (${a.passed}/${a.total})`}
                        {a.kept && (
                          <Badge variant="secondary" className="ml-2">
                            kept
                          </Badge>
                        )}
                      </TableCell>
                      <TableCell className="whitespace-normal text-muted-foreground">{a.error ?? a.rationale}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
            {r.improved && (
              <div className="flex flex-col gap-2">
                <span className="text-sm font-medium">The best instructions</span>
                <pre className="max-h-80 overflow-auto rounded-md bg-muted p-3 text-sm whitespace-pre-wrap">{r.best.instructions}</pre>
              </div>
            )}
          </>
        )}
        {o.applied && (
          <Alert>
            <AlertDescription>
              Saved as {o.applied.name} v{o.applied.version}.
            </AlertDescription>
          </Alert>
        )}
      </CardContent>
      {r?.done && r.improved && !o.applied && (
        <CardFooter className="flex-wrap justify-end gap-2">
          <Input className="max-w-72" placeholder="Version note (optional)" aria-label="Version note" value={note} onChange={(e) => setNote(e.target.value)} />
          <Button onClick={apply} disabled={saving}>
            {saving && <Spinner data-icon="inline-start" />}
            Save as a new version
          </Button>
        </CardFooter>
      )}
    </Card>
  );
}
