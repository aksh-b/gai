import { useEffect, useState } from 'react';
import { DatabaseIcon, PlayIcon, RefreshCwIcon } from 'lucide-react';
import { toast } from 'sonner';
import { CodeEditor } from '@/components/code-editor';
import { ConfirmAction, EmptyState, ErrorNote, LinkItem, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type DataSchema, type QueryAnswer } from '../api';
import { IconPlus, IconTrash } from '../icons';
import { navigate } from '../router';
import { ago, useLoad } from '../util';

const EXAMPLE = `name: app
kind: postgres            # postgres | mysql | sqlite | bigquery | snowflake
url: postgres://reader@db.example.com:5432/app
password_env: APP_DB_PASSWORD
description: Users, signups and orders
# read_only: true         # the default: only SELECTs run
# max_rows: 1000
`;

/** Databases assistants can query, their tables, and a place to try SQL. */
export function DataPage({ selected }: { selected?: string }) {
  const list = useLoad(() => api.connections(), []);
  const [adding, setAdding] = useState(false);
  const current = list.data?.find((c) => c.name === selected) ?? (adding ? undefined : list.data?.[0]);

  return (
    <Page wide>
      <PageHeader
        title="Databases"
        subtitle="Connections your assistants can query with SQL. They are read-only unless you say otherwise, and passwords stay in environment variables or secrets."
        actions={
          <Button onClick={() => setAdding(true)}>
            <IconPlus data-icon="inline-start" />
            Add a database
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[16rem_minmax(0,1fr)]">
        <div className="flex flex-col gap-1">
          {list.data?.length === 0 && !adding && <EmptyState title="No databases yet">Add one to let assistants answer from it.</EmptyState>}
          {list.data?.map((c) => (
            <LinkItem
              key={c.name}
              href={`#/data/${encodeURIComponent(c.name)}`}
              active={!adding && current?.name === c.name}
              title={c.name}
              onClick={() => setAdding(false)}
            >
              <span className="truncate text-xs text-muted-foreground">
                {c.kind}
                {(c.spec as { description?: string }).description ? ` · ${(c.spec as { description?: string }).description}` : ''}
              </span>
            </LinkItem>
          ))}
        </div>
        {adding ? (
          <AddConnection
            onSaved={(name) => {
              setAdding(false);
              list.reload();
              navigate(`/data/${encodeURIComponent(name)}`);
            }}
            onCancel={() => setAdding(false)}
          />
        ) : current ? (
          <ConnectionView
            key={current.name}
            name={current.name}
            kind={current.kind}
            readOnly={(current.spec as { read_only?: boolean }).read_only !== false}
            schemaAt={current.schema_at ?? null}
            onDeleted={() => {
              list.reload();
              navigate('/data');
            }}
          />
        ) : null}
      </div>
    </Page>
  );
}

function AddConnection({ onSaved, onCancel }: { onSaved: (name: string) => void; onCancel: () => void }) {
  const [yaml, setYaml] = useState(EXAMPLE);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  async function save() {
    setBusy(true);
    setError(undefined);
    try {
      const c = await api.saveConnection(yaml);
      toast.success(`Connected to ${c.name}`);
      onSaved(c.name);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Card>
      <CardHeader>
        <CardTitle>Add a database</CardTitle>
        <CardDescription>
          Give the password's environment variable (or, for an organization, a secret's name), never the password itself. BigQuery
          takes <code>project</code> and <code>credentials_env</code>; Snowflake <code>account</code> and <code>token_env</code>.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <CodeEditor language="yaml" schema="ConnectionSpec" value={yaml} onChange={setYaml} aria-label="Connection" minHeight="14rem" />
        <ErrorNote error={error} />
      </CardContent>
      <CardFooter className="gap-2">
        <Button onClick={save} disabled={busy}>
          {busy ? <Spinner data-icon="inline-start" /> : <DatabaseIcon data-icon="inline-start" />}
          Connect
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </CardFooter>
    </Card>
  );
}

function ConnectionView({
  name,
  kind,
  readOnly,
  schemaAt,
  onDeleted,
}: {
  name: string;
  kind: string;
  readOnly: boolean;
  schemaAt: string | null;
  onDeleted: () => void;
}) {
  const [refresh, setRefresh] = useState(0);
  const schema = useLoad(() => api.connectionSchema(name, refresh > 0), [name, refresh]);
  const [sql, setSql] = useState('');
  const [answer, setAnswer] = useState<QueryAnswer>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    const t = schema.data?.tables[0];
    if (t && !sql) setSql(`SELECT * FROM ${t.schema && kind !== 'sqlite' ? `${t.schema}.` : ''}${t.name} LIMIT 20`);
  }, [schema.data, kind, sql]);

  async function run(dry: boolean) {
    setBusy(true);
    setError(undefined);
    try {
      setAnswer(await api.query(name, sql, dry));
    } catch (e) {
      setError((e as Error).message);
      setAnswer(undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            {name} <Badge variant="outline">{kind}</Badge>
            {readOnly ? <Badge variant="secondary">read-only</Badge> : <Badge variant="destructive">can change data</Badge>}
          </CardTitle>
          <CardDescription>{schemaAt ? `Tables read ${ago(schemaAt)}` : 'Tables not read yet'}</CardDescription>
          <CardAction className="flex gap-2">
            <Button variant="outline" size="sm" onClick={() => setRefresh((n) => n + 1)} disabled={schema.loading}>
              <RefreshCwIcon data-icon="inline-start" />
              Read tables again
            </Button>
            <ConfirmAction
              title={`Delete ${name}?`}
              description="Assistants can't query it any more. The database itself isn't touched."
              action="Delete"
              destructive
              onConfirm={() => api.deleteConnection(name).then(onDeleted, (e: Error) => toast.error(e.message))}
            >
              <Button variant="ghost" size="sm">
                <IconTrash data-icon="inline-start" />
                Delete
              </Button>
            </ConfirmAction>
          </CardAction>
        </CardHeader>
        <CardContent>
          <ErrorNote error={schema.error} />
          {schema.loading && !schema.data ? <Spinner /> : <SchemaList schema={schema.data} onPick={(t) => setSql(`SELECT * FROM ${t} LIMIT 20`)} />}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Try a query</CardTitle>
          <CardDescription>One SELECT; it runs read-only, with the connection's row limit.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <CodeEditor language="text" value={sql} onChange={setSql} aria-label="SQL" minHeight="6rem" />
          <ErrorNote error={error} />
          {answer?.plan && <pre className="overflow-x-auto rounded-md bg-muted p-3 text-xs">{answer.plan}</pre>}
          {answer && !answer.plan && <Results answer={answer} />}
        </CardContent>
        <CardFooter className="gap-2">
          <Button onClick={() => run(false)} disabled={busy || !sql.trim()}>
            {busy ? <Spinner data-icon="inline-start" /> : <PlayIcon data-icon="inline-start" />}
            Run
          </Button>
          <Button variant="outline" onClick={() => run(true)} disabled={busy || !sql.trim()}>
            Check only
          </Button>
        </CardFooter>
      </Card>
    </div>
  );
}

function SchemaList({ schema, onPick }: { schema?: DataSchema; onPick: (table: string) => void }) {
  if (!schema) return null;
  if (!schema.tables.length) return <span className="text-sm text-muted-foreground">No tables.</span>;
  return (
    <div className="grid gap-2 md:grid-cols-2 xl:grid-cols-3">
      {schema.tables.map((t) => {
        const full = t.schema ? `${t.schema}.${t.name}` : t.name;
        return (
          <button key={full} type="button" onClick={() => onPick(full)} className="rounded-md border p-2 text-left text-sm hover:bg-muted/50">
            <div className="flex items-center gap-2 font-medium">
              {full}
              {t.kind === 'view' && <Badge variant="outline">view</Badge>}
              {t.rows != null && <span className="ml-auto text-xs text-muted-foreground">~{t.rows.toLocaleString()} rows</span>}
            </div>
            <div className="mt-1 line-clamp-3 text-xs text-muted-foreground">
              {t.columns.map((c) => `${c.name} ${c.data_type.toLowerCase()}`).join(', ')}
            </div>
          </button>
        );
      })}
    </div>
  );
}

function Results({ answer }: { answer: QueryAnswer }) {
  const columns = answer.columns ?? [];
  const rows = answer.rows ?? [];
  return (
    <div className="flex flex-col gap-1">
      <span className="text-sm text-muted-foreground">
        {rows.length} rows in {answer.elapsed_ms} ms{answer.truncated ? ' (more were left out)' : ''}
      </span>
      <div className="max-h-[28rem] overflow-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              {columns.map((c) => (
                <TableHead key={c.name} title={c.data_type}>
                  {c.name}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((r, i) => (
              <TableRow key={i}>
                {r.map((v, j) => (
                  <TableCell key={j} className="font-mono text-xs">
                    {v === null ? <span className="text-muted-foreground">null</span> : typeof v === 'object' ? JSON.stringify(v) : String(v)}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
