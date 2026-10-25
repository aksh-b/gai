import { useEffect, useMemo, useState } from 'react';
import { DownloadIcon, NetworkIcon, TableIcon } from 'lucide-react';
import { toast } from 'sonner';
import { TextField } from '@/components/fields';
import { EmptyState, ErrorNote } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type Extracted, type GraphNodeView, type GraphView } from '../api';
import { IconPlus, IconTrash } from '../icons';
import { money, useLoad } from '../util';

type Field = { name: string; description: string };

const STARTER: Field[] = [
  { name: 'party', description: 'the other party' },
  { name: 'start_date', description: 'when it starts' },
  { name: 'end_date', description: 'when it ends' },
  { name: 'amount', description: 'the main fee or price, with its currency' },
];

const fieldKey = (s: string) =>
  s
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_|_$/g, '');

function cell(v: unknown) {
  if (v === null || v === undefined || v === '') return <span className="text-muted-foreground">—</span>;
  return typeof v === 'string' ? v : JSON.stringify(v);
}

/** Read the same fields out of every document into a table. */
export function ExtractPanel({ collection }: { collection: string }) {
  const [fields, setFields] = useState<Field[]>(STARTER);
  const [result, setResult] = useState<Extracted>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  const spec = () =>
    Object.fromEntries(fields.filter((f) => fieldKey(f.name)).map((f) => [fieldKey(f.name), f.description.trim() || f.name]));

  async function run() {
    setBusy(true);
    setError(undefined);
    try {
      setResult(await api.extract(collection, spec()));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function csv() {
    try {
      const blob = await api.extractCsv(collection, spec());
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = `${collection}-fields.csv`;
      a.click();
      URL.revokeObjectURL(a.href);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const columns = result ? Object.keys((result.schema as { properties?: Record<string, unknown> }).properties ?? {}) : [];

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>Turn documents into a table</CardTitle>
          <CardDescription>
            Say what to look for; a model reads every document (scans too, when OCR is on) and fills one row each. Empty means the document
            doesn't say.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          {fields.map((f, i) => (
            <div key={i} className="flex items-end gap-2">
              <TextField
                className="w-48"
                label={i === 0 ? 'Column' : undefined}
                aria-label="Column"
                value={f.name}
                onChange={(e) => setFields((fs) => fs.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))}
              />
              <TextField
                className="flex-1"
                label={i === 0 ? 'What it is' : undefined}
                aria-label="What it is"
                value={f.description}
                onChange={(e) => setFields((fs) => fs.map((x, j) => (j === i ? { ...x, description: e.target.value } : x)))}
              />
              <Button variant="ghost" size="icon-sm" aria-label={`Remove ${f.name}`} onClick={() => setFields((fs) => fs.filter((_, j) => j !== i))}>
                <IconTrash />
              </Button>
            </div>
          ))}
          <Button variant="outline" size="sm" className="w-fit" onClick={() => setFields((fs) => [...fs, { name: '', description: '' }])}>
            <IconPlus data-icon="inline-start" />
            Add a column
          </Button>
        </CardContent>
        <CardFooter className="gap-2">
          <Button onClick={run} disabled={busy || !Object.keys(spec()).length}>
            {busy ? <Spinner data-icon="inline-start" /> : <TableIcon data-icon="inline-start" />}
            {busy ? 'Reading documents…' : 'Make the table'}
          </Button>
          {result && (
            <Button variant="outline" onClick={csv}>
              <DownloadIcon data-icon="inline-start" />
              Download CSV
            </Button>
          )}
          {result && result.cost_usd > 0 && <span className="text-sm text-muted-foreground">Cost {money(result.cost_usd)}</span>}
        </CardFooter>
      </Card>
      <ErrorNote error={error} />
      {result &&
        (result.rows.length === 0 ? (
          <EmptyState title="No documents to read">Add documents first.</EmptyState>
        ) : (
          <Card>
            <CardContent className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Document</TableHead>
                    {columns.map((c) => (
                      <TableHead key={c}>{c}</TableHead>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {result.rows.map((r) => (
                    <TableRow key={r.document_id}>
                      <TableCell className="font-medium">
                        {r.title ?? r.source}
                        {r.error && (
                          <div className="text-xs text-destructive" title={r.error}>
                            could not read it
                          </div>
                        )}
                      </TableCell>
                      {columns.map((c) => (
                        <TableCell key={c}>{cell(r.fields[c])}</TableCell>
                      ))}
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        ))}
    </div>
  );
}

type Pos = { x: number; y: number };

/** A force layout: connected entities pull together, all push apart. */
function layout(view: GraphView): Map<number, Pos> {
  const ids = view.nodes.map((n) => n.id);
  const pos = new Map<number, Pos>();
  ids.forEach((id, i) => {
    const a = (i / Math.max(ids.length, 1)) * Math.PI * 2;
    pos.set(id, { x: Math.cos(a) * 200 + ((i * 37) % 11), y: Math.sin(a) * 200 + ((i * 53) % 13) });
  });
  const k = 60;
  for (let step = 0; step < 300; step++) {
    const temp = 20 * (1 - step / 300) + 0.5;
    const move = new Map<number, Pos>(ids.map((id) => [id, { x: 0, y: 0 }]));
    for (let i = 0; i < ids.length; i++) {
      for (let j = i + 1; j < ids.length; j++) {
        const a = pos.get(ids[i])!;
        const b = pos.get(ids[j])!;
        const dx = a.x - b.x;
        const dy = a.y - b.y;
        const d = Math.max(Math.hypot(dx, dy), 1);
        const f = (k * k) / d;
        move.get(ids[i])!.x += (dx / d) * f;
        move.get(ids[i])!.y += (dy / d) * f;
        move.get(ids[j])!.x -= (dx / d) * f;
        move.get(ids[j])!.y -= (dy / d) * f;
      }
    }
    for (const e of view.edges) {
      const a = pos.get(e.source);
      const b = pos.get(e.target);
      if (!a || !b) continue;
      const dx = a.x - b.x;
      const dy = a.y - b.y;
      const d = Math.max(Math.hypot(dx, dy), 1);
      const f = (d * d) / k;
      move.get(e.source)!.x -= (dx / d) * f;
      move.get(e.source)!.y -= (dy / d) * f;
      move.get(e.target)!.x += (dx / d) * f;
      move.get(e.target)!.y += (dy / d) * f;
    }
    for (const id of ids) {
      const p = pos.get(id)!;
      const m = move.get(id)!;
      // A pull to the middle keeps loose groups on screen.
      m.x -= p.x * 0.05;
      m.y -= p.y * 0.05;
      const len = Math.max(Math.hypot(m.x, m.y), 1);
      p.x += (m.x / len) * Math.min(len, temp);
      p.y += (m.y / len) * Math.min(len, temp);
    }
  }
  return pos;
}

const KIND_COLOR: Record<string, string> = {
  person: 'var(--chart-1)',
  organization: 'var(--chart-2)',
  place: 'var(--chart-3)',
  product: 'var(--chart-4)',
  event: 'var(--chart-5)',
};

/** The collection's entities and how they connect. */
export function GraphPanel({ collection }: { collection: string }) {
  const [q, setQ] = useState('');
  const view = useLoad(() => api.graph(collection, q || undefined), [collection, q]);
  const [selected, setSelected] = useState<number | null>(null);
  const [building, setBuilding] = useState(false);
  const node = useLoad<GraphNodeView | null>(() => (selected == null ? Promise.resolve(null) : api.graphNode(collection, selected)), [selected]);
  const pos = useMemo(() => (view.data ? layout(view.data) : new Map<number, Pos>()), [view.data]);

  useEffect(() => setSelected(null), [collection]);

  async function build() {
    setBuilding(true);
    try {
      const b = await api.buildGraph(collection);
      toast.success(`Read ${b?.chunks ?? 0} passages: ${b?.entities ?? 0} entities, ${b?.relations ?? 0} relations`);
      view.reload();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBuilding(false);
    }
  }

  const stats = view.data?.stats;
  const xs = [...pos.values()];
  const pad = 40;
  const box = xs.length
    ? [
        Math.min(...xs.map((p) => p.x)) - pad,
        Math.min(...xs.map((p) => p.y)) - pad,
        Math.max(...xs.map((p) => p.x)) - Math.min(...xs.map((p) => p.x)) + pad * 2,
        Math.max(...xs.map((p) => p.y)) - Math.min(...xs.map((p) => p.y)) + pad * 2,
      ]
    : [0, 0, 100, 100];
  const maxMentions = Math.max(1, ...(view.data?.nodes.map((n) => n.mentions) ?? [1]));

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>Knowledge graph</CardTitle>
          <CardDescription>
            People, organizations, places and the like that your documents mention, and how they connect. Assistants searching this
            collection use it to find related passages.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap items-center gap-2 text-sm">
          {stats && (
            <>
              <Badge variant="outline">{stats.nodes} entities</Badge>
              <Badge variant="outline">{stats.edges} relations</Badge>
              <Badge variant={stats.chunks_pending ? 'secondary' : 'outline'}>
                {stats.chunks_pending ? `${stats.chunks_pending} passages not read yet` : 'up to date'}
              </Badge>
            </>
          )}
          <Button size="sm" className="ml-auto" onClick={build} disabled={building || (stats != null && stats.chunks_pending === 0)}>
            {building ? <Spinner data-icon="inline-start" /> : <NetworkIcon data-icon="inline-start" />}
            {building ? 'Reading…' : stats?.chunks_done ? 'Read new passages' : 'Build the graph'}
          </Button>
        </CardContent>
      </Card>
      <ErrorNote error={view.error} />
      {view.data && view.data.nodes.length === 0 ? (
        <EmptyState title={q ? 'No entity by that name' : 'No graph yet'}>
          {q ? 'Try part of the name.' : 'Build it: a model reads every passage once (it costs a model call per few passages).'}
        </EmptyState>
      ) : (
        <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_22rem]">
          <Card>
            <CardContent className="flex flex-col gap-2">
              <TextField aria-label="Find an entity" placeholder="Find an entity…" value={q} onChange={(e) => setQ(e.target.value)} />
              <svg viewBox={box.join(' ')} className="h-[32rem] w-full" role="img" aria-label="Knowledge graph">
                {view.data?.edges.map((e, i) => {
                  const a = pos.get(e.source);
                  const b = pos.get(e.target);
                  if (!a || !b) return null;
                  const on = selected === e.source || selected === e.target;
                  return (
                    <g key={i}>
                      <line
                        x1={a.x}
                        y1={a.y}
                        x2={b.x}
                        y2={b.y}
                        stroke="currentColor"
                        className={on ? 'text-primary' : 'text-border'}
                        strokeWidth={on ? 2 : 1}
                      />
                      {on && (
                        <text x={(a.x + b.x) / 2} y={(a.y + b.y) / 2} fontSize={10} textAnchor="middle" className="fill-muted-foreground">
                          {e.relation}
                        </text>
                      )}
                    </g>
                  );
                })}
                {view.data?.nodes.map((n, i) => {
                  const p = pos.get(n.id);
                  if (!p) return null;
                  const r = 5 + (n.mentions / maxMentions) * 10;
                  return (
                    <g key={n.id} onClick={() => setSelected(n.id)} className="cursor-pointer">
                      <circle
                        cx={p.x}
                        cy={p.y}
                        r={r}
                        fill={KIND_COLOR[n.kind] ?? 'var(--muted-foreground)'}
                        stroke={selected === n.id ? 'var(--foreground)' : 'var(--background)'}
                        strokeWidth={2}
                      >
                        <title>
                          {n.name} ({n.kind}, {n.mentions} mentions)
                        </title>
                      </circle>
                      {(i < 25 || selected === n.id) && (
                        <text x={p.x} y={p.y - r - 3} fontSize={11} textAnchor="middle" className="fill-foreground">
                          {n.name}
                        </text>
                      )}
                    </g>
                  );
                })}
              </svg>
            </CardContent>
          </Card>
          <Card>
            <CardHeader>
              <CardTitle>{node.data?.node.name ?? 'Pick an entity'}</CardTitle>
              {node.data && (
                <CardDescription>
                  {node.data.node.kind} · mentioned in {node.data.node.mentions} passages
                  {node.data.node.description && <> · {node.data.node.description}</>}
                </CardDescription>
              )}
            </CardHeader>
            <CardContent className="flex flex-col gap-3 text-sm">
              {!node.data && <span className="text-muted-foreground">Click a dot to see what it is connected to and where it comes up.</span>}
              {node.data?.relations.map((r, i) => (
                <button key={i} type="button" className="text-left hover:underline" onClick={() => setSelected(r.other.id)}>
                  {r.direction === 'out' ? (
                    <>
                      → <em>{r.relation}</em> <strong>{r.other.name}</strong>
                    </>
                  ) : (
                    <>
                      ← <strong>{r.other.name}</strong> <em>{r.relation}</em>
                    </>
                  )}
                </button>
              ))}
              {!!node.data?.passages.length && <div className="pt-2 font-medium">Where it comes up</div>}
              {node.data?.passages.map((h) => (
                <div key={h.chunk_id} className="rounded-md border p-2">
                  <div className="text-xs text-muted-foreground">{h.title ?? h.source}</div>
                  <div className="line-clamp-4">{h.text}</div>
                </div>
              ))}
            </CardContent>
          </Card>
        </div>
      )}
    </div>
  );
}
