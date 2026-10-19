import { useMemo, useState } from 'react';
import { BrainIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SwitchField, TabBar, TextField } from '@/components/fields';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { FieldGroup } from '@/components/ui/field';
import { Item, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Progress } from '@/components/ui/progress';
import { Spinner } from '@/components/ui/spinner';
import { Textarea } from '@/components/ui/textarea';
import { cn } from '@/lib/utils';
import { api, type MemoryBlock } from '../api';
import { Disclosure, EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { IconPlus, IconTrash } from '../icons';
import { ago, useLoad } from '../util';

const KIND_LABEL: Record<string, string> = { agent: 'Agents', user: 'User profiles', shared: 'Shared' };

export function Memory() {
  const blocks = useLoad(() => api.blocks(), []);
  const [kind, setKind] = useState('all');

  const groups = useMemo(() => {
    const m = new Map<string, MemoryBlock[]>();
    for (const b of blocks.data ?? []) {
      if (kind !== 'all' && !b.scope.startsWith(`${kind}:`)) continue;
      m.set(b.scope, [...(m.get(b.scope) ?? []), b]);
    }
    return [...m.entries()];
  }, [blocks.data, kind]);

  const counts = (k: string) => (blocks.data ?? []).filter((b) => b.scope.startsWith(`${k}:`)).length;
  const count = (n: number | undefined) => <Badge variant="secondary">{n ?? 0}</Badge>;

  return (
    <Page>
      <PageHeader
        title="Memory"
        subtitle={
          <>
            Notes your assistants keep and update as they learn, like what they know about you. You can read and correct them here. Turn
            memory on for an assistant in its settings (Advanced).
          </>
        }
      />
      <ErrorNote error={blocks.error} />
      <TabBar
        items={[
          { id: 'all', label: 'All', badge: count(blocks.data?.length) },
          ...['agent', 'user', 'shared'].map((k) => ({ id: k, label: KIND_LABEL[k], badge: count(counts(k)) })),
        ]}
        value={kind}
        onValueChange={setKind}
        variant="pill"
        aria-label="Filter blocks"
      />
      {groups.length === 0 && blocks.data && (
        <EmptyState title="No memories yet" icon={<BrainIcon />}>
          Assistants with memory turned on (like the “Remembers you” template) keep their notes here. You can also add one below.
        </EmptyState>
      )}
      {groups.map(([scope, list]) => (
        <section key={scope} className="flex flex-col gap-3">
          <h2 className="font-mono text-sm font-medium text-muted-foreground">{scope}</h2>
          <div className="grid gap-3 lg:grid-cols-2">
            {list.map((b) => (
              <BlockCard key={`${b.id}:${b.version}`} b={b} onChange={blocks.reload} />
            ))}
          </div>
        </section>
      ))}
      <Card>
        <CardContent>
          <Disclosure summary="New block">
            <NewBlock onCreated={blocks.reload} />
          </Disclosure>
        </CardContent>
      </Card>
    </Page>
  );
}

function BlockCard({ b, onChange }: { b: MemoryBlock; onChange: () => void }) {
  const [value, setValue] = useState(b.value);
  const [saving, setSaving] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const history = useLoad(() => (showHistory ? api.blockHistory(b.id) : Promise.resolve([])), [showHistory, b.id, b.version]);
  const used = [...value].length;
  const dirty = value !== b.value;
  const over = used > b.limit;

  async function save(body: Parameters<typeof api.editBlock>[1]) {
    setSaving(true);
    try {
      await api.editBlock(b.id, body);
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    await api.deleteBlock(b.id).catch((e: Error) => toast.error(e.message));
    onChange();
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <span className="font-mono">{b.label}</span>
          <Badge variant="outline">v{b.version}</Badge>
        </CardTitle>
        <CardDescription>
          {b.description ? `${b.description} · ` : ''}
          {ago(b.updated_at)}
        </CardDescription>
        <CardAction>
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button variant="ghost" size="icon-sm" aria-label={`Delete ${b.label}`}>
                <IconTrash />
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete the {b.label} block?</AlertDialogTitle>
                <AlertDialogDescription>
                  From {b.scope}. Assistants that declare it start again from its starting value.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction variant="destructive" onClick={remove}>
                  Delete
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <Textarea
          className="max-h-64 min-h-24 font-mono"
          aria-label={`${b.label} value`}
          aria-invalid={over || undefined}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="(empty)"
        />
        <Progress value={Math.min(100, (used / b.limit) * 100)} aria-label={`${used} of ${b.limit} characters`} />
        <span className={cn('text-xs tabular-nums', over ? 'text-destructive' : 'text-muted-foreground')}>
          {used} / {b.limit} characters
        </span>
      </CardContent>
      <CardFooter className="flex-wrap gap-2">
        <SwitchField
          className="w-auto"
          label="Read-only"
          size="sm"
          checked={b.read_only}
          onCheckedChange={(checked) => save({ read_only: checked })}
        />
        <div className="ml-auto flex gap-2">
          {dirty && (
            <Button variant="ghost" size="sm" onClick={() => setValue(b.value)}>
              Reset
            </Button>
          )}
          <Button size="sm" onClick={() => save({ value })} disabled={saving || !dirty || over}>
            {saving && <Spinner data-icon="inline-start" />}
            Save
          </Button>
        </div>
        <Collapsible open={showHistory} onOpenChange={setShowHistory} className="flex w-full flex-col gap-2">
          <CollapsibleTrigger asChild>
            <Button variant="link" size="sm" className="w-fit px-0">
              {showHistory ? 'Hide history' : 'History'}
            </Button>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <ItemGroup className="max-h-72 gap-1 overflow-auto">
              {history.data?.map((r) => (
                <Item key={r.version} size="xs" variant="muted">
                  <ItemContent>
                    <ItemTitle>
                      <Badge variant="outline">v{r.version}</Badge>
                      {r.op}
                      <span className="font-mono font-normal text-muted-foreground">{r.actor ?? ''}</span>
                      <span className="font-normal text-muted-foreground">{ago(r.created_at)}</span>
                    </ItemTitle>
                    <ItemDescription className="line-clamp-none whitespace-pre-wrap">{r.value || '(empty)'}</ItemDescription>
                  </ItemContent>
                </Item>
              ))}
            </ItemGroup>
          </CollapsibleContent>
        </Collapsible>
      </CardFooter>
    </Card>
  );
}

function NewBlock({ onCreated }: { onCreated: () => void }) {
  const [scope, setScope] = useState('user:default');
  const [label, setLabel] = useState('');
  const [limit, setLimit] = useState('2000');
  const [description, setDescription] = useState('');
  const [busy, setBusy] = useState(false);
  async function create() {
    setBusy(true);
    try {
      await api.createBlock({
        scope: scope.trim(),
        label: label.trim(),
        value: '',
        limit: Number(limit) || 2000,
        description: description.trim() || undefined,
      });
      setLabel('');
      setDescription('');
      onCreated();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <FieldGroup className="grid gap-4 pt-2 sm:grid-cols-2">
      <TextField
        label="Scope"
        description="agent:<name>, user:<id> or shared:<name>"
        value={scope}
        onChange={(e) => setScope(e.target.value)}
      />
      <TextField label="Label" placeholder="preferences" value={label} onChange={(e) => setLabel(e.target.value)} />
      <TextField
        label="Limit (characters)"
        inputMode="numeric"
        value={limit}
        onChange={(e) => setLimit(e.target.value.replace(/\D/g, ''))}
      />
      <TextField
        label="Description"
        placeholder="What the agent should keep here"
        value={description}
        onChange={(e) => setDescription(e.target.value)}
      />
      <Button className="w-fit" onClick={create} disabled={busy || !label.trim() || !scope.includes(':')}>
        {busy ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
        Create block
      </Button>
    </FieldGroup>
  );
}
