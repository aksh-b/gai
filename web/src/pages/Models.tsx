import { DataTable, type Column } from '@/components/data-table';
import { api, type CatalogEntry, type KeyState } from '../api';
import { CircleCheckIcon, KeyRoundIcon } from 'lucide-react';
import { ErrorNote, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { useLoad } from '../util';

function KeyBadge({ k }: { k: KeyState }) {
  if (k.status === 'missing')
    return (
      <Badge variant="outline">
        <KeyRoundIcon data-icon="inline-start" />
        Needs {k.env}
      </Badge>
    );
  return (
    <Badge variant="secondary">
      <CircleCheckIcon data-icon="inline-start" />
      {k.status === 'present' ? 'Key set' : 'No key needed'}
    </Badge>
  );
}

const ctx = (n?: number) => (n ? (n >= 1_000_000 ? `${n / 1_000_000}M` : `${Math.round(n / 1000)}k`) : '—');

export function Models({ embedded = false }: { embedded?: boolean } = {}) {
  const providers = useLoad(() => api.providers(), []);
  const models = useLoad(() => api.models(), []);

  const columns: Column<CatalogEntry>[] = [
    { key: 'id', header: 'Model', value: (m) => m.id, cell: (m) => <span className="font-mono">{m.id}</span>, minWidth: 240 },
    {
      key: 'ctx',
      header: 'Context',
      value: (m) => m.capabilities.context_window ?? 0,
      cell: (m) => ctx(m.capabilities.context_window),
      align: 'end',
      width: 90,
    },
    {
      key: 'out',
      header: 'Max out',
      value: (m) => m.capabilities.max_output_tokens ?? 0,
      cell: (m) => ctx(m.capabilities.max_output_tokens),
      align: 'end',
      width: 90,
    },
    {
      key: 'in$',
      header: '$/M in',
      value: (m) => m.pricing?.input ?? 0,
      cell: (m) => (m.pricing ? `$${m.pricing.input}` : '—'),
      align: 'end',
      width: 80,
    },
    {
      key: 'out$',
      header: '$/M out',
      value: (m) => m.pricing?.output ?? 0,
      cell: (m) => (m.pricing ? `$${m.pricing.output}` : '—'),
      align: 'end',
      width: 80,
    },
    {
      key: 'caps',
      header: 'Capabilities',
      cell: (m) => (
        <span className="flex gap-1">
          {m.capabilities.tools && <Badge variant="outline">Tools</Badge>}
          {m.capabilities.vision && <Badge variant="outline">Vision</Badge>}
          {m.capabilities.reasoning && <Badge variant="outline">Reasoning</Badge>}
          {m.capabilities.prompt_caching && <Badge variant="outline">Caching</Badge>}
        </span>
      ),
      minWidth: 240,
    },
    { key: 'key', header: 'Status', value: (m) => m.key.status, cell: (m) => <KeyBadge k={m.key} />, width: 190 },
  ];

  const body = (
    <>
      <p className="text-sm text-muted-foreground">
        Any model a provider serves works as <code>provider/model</code>, listed here or not. More providers can be added in{' '}
        <code>gai.toml</code>.
      </p>
      <ErrorNote error={providers.error ?? models.error} />
      <Card>
        <CardHeader>
          <CardTitle>Providers</CardTitle>
        </CardHeader>
        <CardContent>
          <ItemGroup className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {providers.data?.map((p) => (
              <Item key={p.name} variant="outline" size="sm">
                <ItemContent className="min-w-0">
                  <ItemTitle>{p.name}</ItemTitle>
                  <ItemDescription className="truncate font-mono">
                    {p.dialect} · {p.base_url ?? 'built in'}
                  </ItemDescription>
                </ItemContent>
                <ItemActions>
                  <KeyBadge k={p.key} />
                </ItemActions>
              </Item>
            ))}
          </ItemGroup>
        </CardContent>
      </Card>
      <DataTable rows={models.data ?? []} columns={columns} getRowId={(m) => m.id} height={520} aria-label="Model catalog" />
    </>
  );
  if (embedded) return <div className="flex flex-col gap-4">{body}</div>;
  return (
    <Page wide>
      <PageHeader title="Models" />
      {body}
    </Page>
  );
}
