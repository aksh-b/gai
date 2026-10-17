import { DataTable, type Column } from '@flame';
import { api, type CatalogEntry, type KeyState } from '../api';
import { ErrorNote, PageHeader } from '../components';
import { useLoad } from '../util';

function KeyBadge({ k }: { k: KeyState }) {
  if (k.status === 'missing')
    return (
      <span className="pill" data-tone="warn">
        needs {k.env}
      </span>
    );
  return (
    <span className="pill" data-tone="ok">
      {k.status === 'present' ? 'key set' : 'no key needed'}
    </span>
  );
}

const ctx = (n?: number) => (n ? (n >= 1_000_000 ? `${n / 1_000_000}M` : `${Math.round(n / 1000)}k`) : '—');

export function Models() {
  const providers = useLoad(() => api.providers(), []);
  const models = useLoad(() => api.models(), []);

  const columns: Column<CatalogEntry>[] = [
    { key: 'id', header: 'Model', value: (m) => m.id, cell: (m) => <span className="mono">{m.id}</span>, minWidth: 240 },
    { key: 'ctx', header: 'Context', value: (m) => m.capabilities.context_window ?? 0, cell: (m) => ctx(m.capabilities.context_window), align: 'end', width: 90 },
    { key: 'out', header: 'Max out', value: (m) => m.capabilities.max_output_tokens ?? 0, cell: (m) => ctx(m.capabilities.max_output_tokens), align: 'end', width: 90 },
    { key: 'in$', header: '$/M in', value: (m) => m.pricing?.input ?? 0, cell: (m) => (m.pricing ? `$${m.pricing.input}` : '—'), align: 'end', width: 80 },
    { key: 'out$', header: '$/M out', value: (m) => m.pricing?.output ?? 0, cell: (m) => (m.pricing ? `$${m.pricing.output}` : '—'), align: 'end', width: 80 },
    {
      key: 'caps',
      header: 'Capabilities',
      cell: (m) => (
        <span className="row gap">
          {m.capabilities.tools && <span className="chip small">tools</span>}
          {m.capabilities.vision && <span className="chip small">vision</span>}
          {m.capabilities.reasoning && <span className="chip small">reasoning</span>}
          {m.capabilities.prompt_caching && <span className="chip small">caching</span>}
        </span>
      ),
      minWidth: 240,
    },
    { key: 'key', header: 'Status', value: (m) => m.key.status, cell: (m) => <KeyBadge k={m.key} />, width: 190 },
  ];

  return (
    <div className="page">
      <PageHeader
        title="Models"
        subtitle={
          <>
            Any model a provider serves works as <span className="mono">provider/model</span>, listed here or not. Add providers in{' '}
            <span className="mono">gai.toml</span>.
          </>
        }
      />
      <ErrorNote error={providers.error ?? models.error} />
      <section className="card">
        <h3>Providers</h3>
        <div className="provider-grid">
          {providers.data?.map((p) => (
            <div key={p.name} className="provider">
              <div className="row between">
                <strong>{p.name}</strong>
                <KeyBadge k={p.key} />
              </div>
              <span className="muted small mono">{p.dialect} · {p.base_url ?? 'built in'}</span>
            </div>
          ))}
        </div>
      </section>
      <section className="card flush">
        <DataTable rows={models.data ?? []} columns={columns} getRowId={(m) => m.id} height={520} aria-label="Model catalog" />
      </section>
    </div>
  );
}
