import { useState } from 'react';
import { BellIcon, RefreshCwIcon } from 'lucide-react';
import { Area, AreaChart, Bar, BarChart, CartesianGrid, Cell, Line, LineChart, Pie, PieChart, XAxis, YAxis } from 'recharts';
import { toast } from 'sonner';
import { CodeEditor } from '@/components/code-editor';
import { ConfirmAction, EmptyState, ErrorNote, LinkItem, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { ChartContainer, ChartLegend, ChartLegendContent, ChartTooltip, ChartTooltipContent, type ChartConfig } from '@/components/ui/chart';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type DashboardPanel, type DashboardView, type PanelResult } from '../api';
import { IconPlus, IconTrash } from '../icons';
import { navigate } from '../router';
import { ago, useLoad } from '../util';

const EXAMPLE = `name: growth
title: Growth
refresh_minutes: 60
# notify: { email: [you@example.com] }
panels:
  - title: Signups per day
    connection: app
    sql: SELECT day, COUNT(*) AS signups FROM signups GROUP BY day ORDER BY day
    chart: line          # line | bar | area | pie | number | table
    x: day
    y: [signups]
    # alert: { below: 50 }
`;

const COLORS = ['var(--chart-1)', 'var(--chart-2)', 'var(--chart-3)', 'var(--chart-4)', 'var(--chart-5)'];

/** Dashboards: charts with their queries, refreshed on a schedule. */
export function DashboardsPage({ selected }: { selected?: string }) {
  const list = useLoad(() => api.dashboards(), []);
  const [adding, setAdding] = useState(false);
  const current = adding ? undefined : (list.data?.find((d) => d.spec.name === selected) ?? list.data?.[0]);

  return (
    <Page wide>
      <PageHeader
        title="Dashboards"
        subtitle="Charts over your databases. Ask an assistant to build one, or write it here. Alerts tell you when a number crosses a limit."
        actions={
          <Button onClick={() => setAdding(true)}>
            <IconPlus data-icon="inline-start" />
            New dashboard
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[14rem_minmax(0,1fr)]">
        <div className="flex flex-col gap-1">
          {list.data?.length === 0 && !adding && (
            <EmptyState title="No dashboards yet">Ask the Database analyst to "make a dashboard of daily signups".</EmptyState>
          )}
          {list.data?.map((d) => (
            <LinkItem
              key={d.spec.name}
              href={`#/dashboards/${encodeURIComponent(d.spec.name)}`}
              active={current?.spec.name === d.spec.name}
              title={d.spec.title}
              onClick={() => setAdding(false)}
            >
              <span className="text-xs text-muted-foreground">{d.spec.panels.length} panels</span>
            </LinkItem>
          ))}
        </div>
        {adding ? (
          <NewDashboard
            onSaved={(name) => {
              setAdding(false);
              list.reload();
              navigate(`/dashboards/${encodeURIComponent(name)}`);
            }}
            onCancel={() => setAdding(false)}
          />
        ) : current ? (
          <DashboardView key={current.spec.name} view={current} onChange={list.reload} />
        ) : null}
      </div>
    </Page>
  );
}

function NewDashboard({ onSaved, onCancel }: { onSaved: (name: string) => void; onCancel: () => void }) {
  const [yaml, setYaml] = useState(EXAMPLE);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  return (
    <Card>
      <CardHeader>
        <CardTitle>New dashboard</CardTitle>
        <CardDescription>Each panel is one SELECT on a connection and how to draw it.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <CodeEditor language="yaml" schema="DashboardSpec" value={yaml} onChange={setYaml} aria-label="Dashboard" minHeight="16rem" />
        <ErrorNote error={error} />
      </CardContent>
      <CardFooter className="gap-2">
        <Button
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError(undefined);
            try {
              const d = await api.saveDashboard(yaml);
              onSaved(d.spec.name);
            } catch (e) {
              setError((e as Error).message);
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy && <Spinner data-icon="inline-start" />}
          Save and refresh
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </CardFooter>
    </Card>
  );
}

function DashboardView({ view, onChange }: { view: DashboardView; onChange: () => void }) {
  const [results, setResults] = useState(view.results);
  const [busy, setBusy] = useState(false);
  async function refresh() {
    setBusy(true);
    try {
      setResults(await api.refreshDashboard(view.spec.name));
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
          <CardTitle>{view.spec.title}</CardTitle>
          <CardDescription>
            {view.spec.description ? `${view.spec.description} · ` : ''}
            {results?.refreshed_at ? `Refreshed ${ago(results.refreshed_at)}` : 'Not refreshed yet'}
            {view.spec.refresh_minutes ? ` · every ${view.spec.refresh_minutes} min` : ''}
          </CardDescription>
          <CardAction className="flex gap-2">
            <Button variant="outline" size="sm" onClick={refresh} disabled={busy}>
              {busy ? <Spinner data-icon="inline-start" /> : <RefreshCwIcon data-icon="inline-start" />}
              Refresh
            </Button>
            <ConfirmAction
              title={`Delete ${view.spec.title}?`}
              description="Its charts and alerts go; the databases aren't touched."
              action="Delete"
              destructive
              onConfirm={() => api.deleteDashboard(view.spec.name).then(onChange, (e: Error) => toast.error(e.message))}
            >
              <Button variant="ghost" size="sm">
                <IconTrash data-icon="inline-start" />
                Delete
              </Button>
            </ConfirmAction>
          </CardAction>
        </CardHeader>
      </Card>
      <div className="grid gap-4 xl:grid-cols-2">
        {view.spec.panels.map((p, i) => (
          <PanelCard key={i} panel={p} result={results?.panels[i]} />
        ))}
      </div>
    </div>
  );
}

function PanelCard({ panel, result }: { panel: DashboardPanel; result?: PanelResult }) {
  return (
    <Card className={panel.chart === 'number' ? '' : 'xl:col-span-1'}>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          {panel.title}
          {panel.alert && (
            <Badge variant={result?.alerting ? 'destructive' : 'outline'} className="gap-1">
              <BellIcon className="size-3" />
              {result?.alerting ? 'alerting' : panel.alert.below != null ? `below ${panel.alert.below}` : `above ${panel.alert.above}`}
            </Badge>
          )}
        </CardTitle>
        <CardDescription className="font-mono text-xs">{panel.connection}</CardDescription>
      </CardHeader>
      <CardContent>
        {!result ? (
          <span className="text-sm text-muted-foreground">No results yet: refresh.</span>
        ) : result.error ? (
          <ErrorNote error={result.error} />
        ) : (
          <PanelChart panel={panel} result={result} />
        )}
      </CardContent>
    </Card>
  );
}

function PanelChart({ panel, result }: { panel: DashboardPanel; result: PanelResult }) {
  const cols = result.columns.map((c) => c.name);
  const rows = result.rows.map((r) => Object.fromEntries(cols.map((c, i) => [c, r[i]])));
  const x = panel.x ?? cols[0];
  const numeric = (c: string) => rows.some((r) => typeof r[c] === 'number' || (typeof r[c] === 'string' && r[c] !== '' && !isNaN(Number(r[c]))));
  const ys = panel.y?.length ? panel.y : cols.filter((c) => c !== x && numeric(c));
  const data = rows.map((r) => ({ ...r, ...Object.fromEntries(ys.map((y) => [y, Number(r[y])])) }));
  const config: ChartConfig = Object.fromEntries(ys.map((y, i) => [y, { label: y, color: COLORS[i % COLORS.length] }]));

  if (panel.chart === 'number') {
    const last = result.rows[result.rows.length - 1];
    const v = last?.[cols.indexOf(ys[0] ?? cols[0])] ?? last?.[0];
    return <div className="text-4xl font-semibold tabular-nums">{typeof v === 'number' ? v.toLocaleString() : String(v ?? '—')}</div>;
  }
  if (panel.chart === 'table' || !ys.length) {
    return (
      <div className="max-h-80 overflow-auto">
        <Table>
          <TableHeader>
            <TableRow>
              {cols.map((c) => (
                <TableHead key={c}>{c}</TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {result.rows.map((r, i) => (
              <TableRow key={i}>
                {r.map((v, j) => (
                  <TableCell key={j} className="tabular-nums">
                    {v === null ? '' : String(v)}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    );
  }
  if (panel.chart === 'pie') {
    const y = ys[0];
    return (
      <ChartContainer config={config} className="h-64 w-full">
        <PieChart>
          <ChartTooltip content={<ChartTooltipContent nameKey={x} />} />
          <Pie data={data} dataKey={y} nameKey={x} innerRadius={50}>
            {data.map((_, i) => (
              <Cell key={i} fill={COLORS[i % COLORS.length]} />
            ))}
          </Pie>
        </PieChart>
      </ChartContainer>
    );
  }
  const axes = (
    <>
      <CartesianGrid vertical={false} />
      <XAxis dataKey={x} tickLine={false} axisLine={false} minTickGap={24} />
      <YAxis tickLine={false} axisLine={false} width={48} />
      <ChartTooltip content={<ChartTooltipContent />} />
      {ys.length > 1 && <ChartLegend content={<ChartLegendContent />} />}
    </>
  );
  return (
    <ChartContainer config={config} className="h-64 w-full">
      {panel.chart === 'bar' ? (
        <BarChart data={data}>
          {axes}
          {ys.map((y) => (
            <Bar key={y} dataKey={y} fill={`var(--color-${y})`} radius={3} />
          ))}
        </BarChart>
      ) : panel.chart === 'area' ? (
        <AreaChart data={data}>
          {axes}
          {ys.map((y) => (
            <Area key={y} dataKey={y} stroke={`var(--color-${y})`} fill={`var(--color-${y})`} fillOpacity={0.2} />
          ))}
        </AreaChart>
      ) : (
        <LineChart data={data}>
          {axes}
          {ys.map((y) => (
            <Line key={y} dataKey={y} stroke={`var(--color-${y})`} dot={false} strokeWidth={2} />
          ))}
        </LineChart>
      )}
    </ChartContainer>
  );
}
