import { api } from '../api';
import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from 'recharts';
import { EmptyState, ErrorNote, Page, PageHeader, Stat } from '@/components/shared';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '@/components/ui/chart';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { money, tokens, totalIn, useLoad } from '../util';

const CHART = {
  input: { label: 'Input tokens', color: 'var(--chart-2)' },
  output: { label: 'Output tokens', color: 'var(--chart-4)' },
} satisfies ChartConfig;

export function UsagePage() {
  const { data, error } = useLoad(() => api.usage(), []);
  const rows = [...(data ?? [])].sort((a, b) => b.cost - a.cost || totalIn(b.usage) - totalIn(a.usage));
  const totals = rows.reduce(
    (t, r) => ({
      calls: t.calls + r.calls,
      input: t.input + totalIn(r.usage),
      output: t.output + r.usage.output_tokens,
      cost: t.cost + r.cost,
    }),
    { calls: 0, input: 0, output: 0, cost: 0 },
  );

  const chartData = rows.map((r) => ({ model: r.model, input: totalIn(r.usage), output: r.usage.output_tokens }));

  return (
    <Page>
      <PageHeader title="Spending" subtitle="What your assistants have cost, by model. Local models are free." />
      <ErrorNote error={error} />
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label="Model calls" value={totals.calls.toLocaleString()} />
        <Stat label="Input tokens" value={tokens(totals.input)} />
        <Stat label="Output tokens" value={tokens(totals.output)} />
        <Stat label="Cost" value={money(totals.cost)} />
      </div>
      <Card>
        <CardHeader>
          <CardTitle>By model</CardTitle>
          <CardDescription>Tokens in and out per model.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-6">
          {rows.length === 0 ? (
            <EmptyState title="Nothing spent yet" />
          ) : (
            <>
              <ChartContainer config={CHART} className="w-full" style={{ height: Math.max(120, rows.length * 44) }}>
                <BarChart data={chartData} layout="vertical" margin={{ left: 0, right: 12 }}>
                  <CartesianGrid horizontal={false} />
                  <YAxis dataKey="model" type="category" width={200} tickLine={false} axisLine={false} />
                  <XAxis type="number" tickLine={false} axisLine={false} tickFormatter={(v: number) => tokens(v)} />
                  <ChartTooltip content={<ChartTooltipContent />} />
                  <ChartLegend content={<ChartLegendContent />} />
                  <Bar dataKey="input" stackId="t" fill="var(--color-input)" radius={[4, 0, 0, 4]} />
                  <Bar dataKey="output" stackId="t" fill="var(--color-output)" radius={[0, 4, 4, 0]} />
                </BarChart>
              </ChartContainer>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Model</TableHead>
                    <TableHead className="text-right">Calls</TableHead>
                    <TableHead className="text-right">Tokens in / out</TableHead>
                    <TableHead className="text-right">Cost</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((r) => (
                    <TableRow key={r.model}>
                      <TableCell className="font-mono">{r.model}</TableCell>
                      <TableCell className="text-right tabular-nums">{r.calls}</TableCell>
                      <TableCell className="text-right tabular-nums">
                        {tokens(totalIn(r.usage))} / {tokens(r.usage.output_tokens)}
                      </TableCell>
                      <TableCell className="text-right font-medium tabular-nums">{money(r.cost)}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </>
          )}
        </CardContent>
      </Card>
    </Page>
  );
}
