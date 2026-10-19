import { useEffect, useState } from 'react';
import { TabBar } from '@/components/fields';
import { DataTable, type Column } from '@/components/data-table';
import { api, type Run } from '../api';
import { ErrorNote, PageHeader, StatusPill } from '../components';
import { navigate } from '../router';
import { ago, blocksText, money, tokens, totalIn, useLoad } from '../util';
import { modelLabel } from '../labels';
import { useDevMode } from '../prefs';

const FILTERS = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Working' },
  { id: 'awaiting_approval', label: 'Needs your OK' },
  { id: 'completed', label: 'Done' },
  { id: 'failed', label: 'Failed' },
];

export function Runs() {
  const [dev] = useDevMode();
  const [filter, setFilter] = useState('all');
  const { data, error, reload } = useLoad(
    () => api.runs({ status: filter === 'all' ? undefined : filter, limit: 500 }),
    [filter],
  );

  // Refresh while anything is still in flight.
  useEffect(() => {
    if (!data?.some((r) => ['running', 'queued', 'awaiting_approval'].includes(r.status))) return;
    const t = setInterval(reload, 2000);
    return () => clearInterval(t);
  }, [data, reload]);

  const columns: Column<Run>[] = [
    { key: 'status', header: 'Status', value: (r) => r.status, cell: (r) => <StatusPill status={r.status} />, width: 150 },
    { key: 'agent', header: 'Who', value: (r) => r.agent.replace(/^workflow:/, ''), width: 140 },
    { key: 'input', header: 'Asked', value: (r) => blocksText(r.input), minWidth: 260 },
    {
      key: 'model',
      header: 'Model',
      value: (r) => r.metadata.model ?? '',
      cell: (r) => (dev ? <span className="mono small">{r.metadata.model}</span> : <span className="small">{r.metadata.model ? modelLabel(r.metadata.model) : ''}</span>),
      width: 220,
    },
    ...(dev
      ? ([
          { key: 'steps', header: 'Steps', value: (r) => r.steps, align: 'end', width: 70 },
          { key: 'tokens', header: 'Tokens', value: (r) => totalIn(r.usage) + r.usage.output_tokens, cell: (r) => tokens(totalIn(r.usage) + r.usage.output_tokens), align: 'end', width: 90 },
        ] as Column<Run>[])
      : []),
    { key: 'cost', header: 'Cost', value: (r) => r.cost, cell: (r) => money(r.cost), align: 'end', width: 80 },
    { key: 'created', header: 'Started', value: (r) => new Date(r.created_at), cell: (r) => ago(r.created_at), align: 'end', width: 100 },
  ];

  return (
    <div className="page">
      <PageHeader title="History" subtitle="Everything your assistants and workflows did: open one to see each step." />
      <div className="row between">
        <TabBar items={FILTERS} value={filter} onValueChange={setFilter} variant="pill" aria-label="Filter runs" />
      </div>
      <ErrorNote error={error} />
      <div className="card flush">
        <DataTable
          rows={data ?? []}
          columns={columns}
          getRowId={(r) => r.id}
          height={Math.max(320, window.innerHeight - 260)}
          onRowAction={(r) => navigate(`/activity/${r.id}`)}
          defaultSort={[{ key: 'created', dir: 'desc' }]}
          empty="No runs yet. Start one from the Playground."
          aria-label="Runs"
        />
      </div>
      <p className="muted small">Click a row (or press Enter) to open it.</p>
    </div>
  );
}
