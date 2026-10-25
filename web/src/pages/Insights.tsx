import { useEffect, useState } from 'react';
import { SearchIcon } from 'lucide-react';
import { SelectField } from '@/components/fields';
import { EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { Card, CardContent } from '@/components/ui/card';
import { InputGroup, InputGroupAddon, InputGroupInput } from '@/components/ui/input-group';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type MessageHit } from '../api';
import { ago, money, useLoad } from '../util';

const PERIODS = [
  { value: '7', label: 'Last 7 days' },
  { value: '30', label: 'Last 30 days' },
  { value: '90', label: 'Last 90 days' },
  { value: '', label: 'All time' },
];

const since = (days: string) => (days ? new Date(Date.now() - Number(days) * 86_400_000).toISOString() : undefined);

const ms = (n: number) => (n >= 10_000 ? `${(n / 1000).toFixed(0)} s` : n >= 1000 ? `${(n / 1000).toFixed(1)} s` : `${n} ms`);

/** How each assistant (and each of its versions) does. */
export function AgentAnalytics() {
  const [days, setDays] = useState('30');
  const { data, error, loading } = useLoad(() => api.agentStats(since(days)), [days]);
  return (
    <Page>
      <PageHeader
        title="By assistant"
        subtitle="How each version does: how often it finishes, how long and how many steps it takes, what it costs, and how people rate it. Tests aren't counted."
        actions={<SelectField aria-label="Period" options={PERIODS} value={days} onValueChange={(v) => setDays(String(v ?? ''))} />}
      />
      <ErrorNote error={error} />
      {loading && !data ? (
        <Spinner />
      ) : !data?.length ? (
        <EmptyState title="No runs in this period">Chat with an assistant and it shows up here.</EmptyState>
      ) : (
        <Card>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Assistant</TableHead>
                  <TableHead className="text-right">Runs</TableHead>
                  <TableHead className="text-right">Finished well</TableHead>
                  <TableHead className="text-right">Steps</TableHead>
                  <TableHead className="text-right">Time (median · slowest 5%)</TableHead>
                  <TableHead className="text-right">Cost</TableHead>
                  <TableHead className="text-right">Ratings</TableHead>
                  <TableHead className="text-right">Last run</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.map((s) => (
                  <TableRow key={`${s.agent}@${s.agent_version ?? ''}`}>
                    <TableCell className="font-medium">
                      <a href={`#/assistants/${encodeURIComponent(s.agent)}`}>{s.agent}</a>
                      {s.agent_version != null && <span className="text-muted-foreground"> v{s.agent_version}</span>}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">{s.runs}</TableCell>
                    <TableCell className="text-right tabular-nums" title={`${s.completed} done, ${s.failed} failed, ${s.stopped} stopped`}>
                      {s.completed + s.failed + s.stopped ? `${Math.round(s.success_rate * 100)}%` : '—'}
                      {s.failed > 0 && <span className="text-destructive"> · {s.failed} failed</span>}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">{s.avg_steps.toFixed(1)}</TableCell>
                    <TableCell className="text-right tabular-nums">
                      {ms(s.p50_latency_ms)} · {ms(s.p95_latency_ms)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">{money(s.cost_usd)}</TableCell>
                    <TableCell className="text-right tabular-nums">
                      {s.good + s.bad ? `👍 ${s.good} · 👎 ${s.bad}` : '—'}
                    </TableCell>
                    <TableCell className="text-right text-muted-foreground">{ago(s.last_run_at)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      )}
    </Page>
  );
}

/** `[word]` marks in a snippet, shown highlighted. */
function Snippet({ text }: { text: string }) {
  const parts = text.split(/(\[[^\]]*\])/);
  return (
    <>
      {parts.map((p, i) =>
        p.startsWith('[') && p.endsWith(']') ? (
          <mark key={i} className="rounded-sm bg-primary/15 px-0.5 text-foreground">
            {p.slice(1, -1)}
          </mark>
        ) : (
          <span key={i}>{p}</span>
        ),
      )}
    </>
  );
}

/** Find words across every conversation. */
export function ConversationSearch() {
  const [q, setQ] = useState('');
  const [agent, setAgent] = useState('');
  const [hits, setHits] = useState<MessageHit[]>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const agents = useLoad(() => api.agents(), []);

  useEffect(() => {
    const words = q.trim();
    if (!words) {
      setHits(undefined);
      return;
    }
    setBusy(true);
    const t = setTimeout(() => {
      api
        .searchSessions(words, agent || undefined)
        .then((h) => {
          setHits(h);
          setError(undefined);
        })
        .catch((e: Error) => setError(e.message))
        .finally(() => setBusy(false));
    }, 250);
    return () => clearTimeout(t);
  }, [q, agent]);

  return (
    <Page>
      <PageHeader title="Search chats" subtitle="Every conversation in this organization, by the words in it." />
      <div className="flex flex-wrap gap-3">
        <InputGroup className="max-w-xl flex-1">
          <InputGroupAddon>{busy ? <Spinner /> : <SearchIcon />}</InputGroupAddon>
          <InputGroupInput
            autoFocus
            placeholder="e.g. quarterly report"
            aria-label="Search chats"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </InputGroup>
        <SelectField
          className="w-56"
          aria-label="Assistant"
          options={[{ value: '', label: 'Every assistant' }, ...(agents.data ?? []).map((a) => ({ value: a.summary.name, label: a.summary.name }))]}
          value={agent}
          onValueChange={(v) => setAgent(String(v ?? ''))}
        />
      </div>
      <ErrorNote error={error} />
      {hits && hits.length === 0 && <EmptyState title="Nothing found">Try fewer or different words.</EmptyState>}
      <div className="flex flex-col gap-2">
        {hits?.map((h) => (
          <a
            key={`${h.session_id}-${h.seq}`}
            href={`#/chat?session=${h.session_id}${h.agent ? `&agent=${encodeURIComponent(h.agent)}` : ''}`}
            className="flex flex-col gap-1 rounded-lg border p-3 hover:bg-muted/50"
          >
            <span className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
              <span className="font-medium text-foreground">{h.session_title || 'Untitled chat'}</span>
              {h.agent && <span>· {h.agent}</span>}
              <span>· {h.role === 'user' ? 'asked' : 'answered'}</span>
              <span className="ml-auto">{ago(h.created_at)}</span>
            </span>
            <span className="text-sm">
              <Snippet text={h.snippet} />
            </span>
          </a>
        ))}
      </div>
    </Page>
  );
}
