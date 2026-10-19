import { useEffect } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { CodeBlock, CodeBlockContent, CodeBlockLine } from '@/components/ui/code-block';
import { Item, ItemContent, ItemDescription, ItemTitle } from '@/components/ui/item';
import { Separator } from '@/components/ui/separator';
import { api, type Run } from '../api';
import { Details, EmptyState, ErrorNote, Json, Page, PageHeader, StatusPill } from '@/components/shared';
import { IconBack } from '../icons';
import { navigate } from '../router';
import { blocksText, compactJson, duration, money, tokens, totalIn, useLoad } from '../util';
import { lineDiff } from '../Versions';
import { cn } from '@/lib/utils';
import { forkOf, turns, type Turn } from './Fork';

const terminal = ['completed', 'failed', 'cancelled', 'budget_exceeded'];

/** Two runs side by side, step by step: an original and its fork. */
export function Compare({ a, b }: { a: string; b: string }) {
  const ra = useLoad(() => api.run(a), [a]);
  const rb = useLoad(() => api.run(b), [b]);
  const sa = useLoad(() => api.steps(a), [a]);
  const sb = useLoad(() => api.steps(b), [b]);

  // Follow runs that are still going.
  const going = [ra.data, rb.data].some((r) => r && !terminal.includes(r.status));
  useEffect(() => {
    if (!going) return;
    const t = setInterval(() => {
      ra.reload();
      rb.reload();
      sa.reload();
      sb.reload();
    }, 1500);
    return () => clearInterval(t);
  }, [going]);

  const left = ra.data;
  const right = rb.data;
  const ta = turns(sa.data ?? []);
  const tb = turns(sb.data ?? []);
  const fork = right ? forkOf(right) : undefined;
  const n = Math.max(ta.at(-1)?.step ?? 0, tb.at(-1)?.step ?? 0);
  const outA = outputText(left);
  const outB = outputText(right);

  return (
    <Page wide>
      <div className="flex gap-2">
        <Button variant="ghost" size="sm" onClick={() => navigate(`/activity/${a}`)}>
          <IconBack data-icon="inline-start" />
          Original run
        </Button>
        <Button variant="ghost" size="sm" onClick={() => navigate(`/activity/${b}`)}>
          Fork
        </Button>
      </div>
      <ErrorNote error={ra.error ?? rb.error} />
      <PageHeader
        title="Compare runs"
        subtitle={
          fork && fork.run_id === a
            ? `Forked at step ${fork.from_step}: steps before it were replayed from the original, later steps ran live.`
            : undefined
        }
      />
      {fork && Object.keys(fork.overrides ?? {}).length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Changes in the fork</CardTitle>
          </CardHeader>
          <CardContent>
            <ul className="flex list-disc flex-col gap-1 pl-5">
              {fork.overrides.model && (
                <li>
                  Model <code>{fork.overrides.model}</code>
                </li>
              )}
              {fork.overrides.agent_version && <li>Agent version v{fork.overrides.agent_version}</li>}
              {fork.overrides.tools && <li>Tools: {fork.overrides.tools.length ? fork.overrides.tools.join(', ') : 'none'}</li>}
              {fork.overrides.system_prompt !== undefined && <li>New system prompt</li>}
              {(fork.overrides.step_edits ?? []).map((e) => (
                <li key={e.step}>Edited the reply at step {e.step}</li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}

      <div className="grid gap-4 md:grid-cols-2">
        <RunSummary run={left} title="Original" />
        <RunSummary run={right} title="Fork" />
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Steps</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {n === 0 && <EmptyState title="No model steps yet" />}
          {Array.from({ length: n }, (_, i) => i + 1).map((step) => {
            const x = ta.find((t) => t.step === step);
            const y = tb.find((t) => t.step === step);
            const same = x && y && turnKey(x) === turnKey(y);
            return (
              <div key={step} className="flex flex-col gap-3">
                {step > 1 && <Separator />}
                <div className="flex items-center gap-2">
                  <span className="font-mono text-sm">Step {step}</span>
                  {y?.replayed ? (
                    <Badge variant="outline">Replayed</Badge>
                  ) : same ? (
                    <Badge variant="outline">Same</Badge>
                  ) : x && y ? (
                    <Badge>Different</Badge>
                  ) : null}
                </div>
                <div className="grid gap-4 md:grid-cols-2">
                  <TurnView turn={x} />
                  <TurnView turn={y} />
                </div>
              </div>
            );
          })}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Final output</CardTitle>
        </CardHeader>
        <CardContent>
          {outA === outB ? (
            <p className="text-muted-foreground">{outA ? 'The same in both runs.' : 'Neither run has an output yet.'}</p>
          ) : (
            <CodeBlock size="sm">
              <CodeBlockContent wrap className="max-h-[32rem]">
                {lineDiff(outA, outB).map((l, i) => (
                  <CodeBlockLine key={i} variant={l.op === 'add' ? 'added' : l.op === 'del' ? 'removed' : 'default'}>
                    {l.text || ' '}
                  </CodeBlockLine>
                ))}
              </CodeBlockContent>
            </CodeBlock>
          )}
        </CardContent>
      </Card>
    </Page>
  );
}

function outputText(r: Run | undefined): string {
  if (!r) return '';
  if (r.error && r.output == null) return `error: ${r.error}`;
  if (r.output == null) return '';
  return typeof r.output === 'string' ? r.output : JSON.stringify(r.output, null, 2);
}

/** What a turn said and did, to tell whether two turns match. */
function turnKey(t: Turn): string {
  return JSON.stringify([blocksText(t.content), t.tools.map((c) => [c.name, c.input, c.output])]);
}

function RunSummary({ run, title }: { run: Run | undefined; title: string }) {
  if (!run) return <Card />;
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>
          <a
            className="font-mono underline-offset-4 hover:underline"
            href={`#/activity/${run.id}`}
            onClick={(e) => {
              e.preventDefault();
              navigate(`/activity/${run.id}`);
            }}
          >
            {run.id}
          </a>
        </CardDescription>
        <CardAction>
          <StatusPill status={run.status} />
        </CardAction>
      </CardHeader>
      <CardContent>
        <Details
          items={[
            ['Agent', `${run.agent}${run.agent_version ? ` v${run.agent_version}` : ''}`],
            ['Model', <span className="font-mono">{run.metadata.model}</span>],
            ['Steps', run.steps],
            ['Tokens', `${tokens(totalIn(run.usage))} in / ${tokens(run.usage.output_tokens)} out`],
            ['Cost', money(run.cost)],
            ['Duration', duration(run.created_at, run.finished_at)],
          ]}
        />
      </CardContent>
    </Card>
  );
}

function TurnView({ turn }: { turn: Turn | undefined }) {
  if (!turn) return <div className="text-sm text-muted-foreground">—</div>;
  const text = blocksText(turn.content);
  return (
    <div className={cn('flex min-w-0 flex-col gap-2', turn.failed && 'text-muted-foreground')}>
      {turn.model && <div className="font-mono text-xs text-muted-foreground">{turn.model}</div>}
      {text && <p className="break-words whitespace-pre-wrap">{text}</p>}
      {turn.tools.map((c) => (
        <Item key={c.id} variant="muted" size="xs">
          <ItemContent className="min-w-0">
            <ItemTitle className="font-mono break-all">
              {c.name} {compactJson(c.input, 80)}
            </ItemTitle>
            {c.output !== undefined && (
              <ItemDescription className={cn('break-all', c.error && 'text-destructive')}>→ {compactJson(c.output, 160)}</ItemDescription>
            )}
          </ItemContent>
        </Item>
      ))}
      {!text && !turn.tools.length && (turn.failed ? <span className="text-sm">Failed</span> : <Json value={turn.content} />)}
    </div>
  );
}
