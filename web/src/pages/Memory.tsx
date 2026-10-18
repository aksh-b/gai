import { useMemo, useState } from 'react';
import { Button, Switch, Tabs, TextField, toast } from '@flame';
import { api, type MemoryBlock } from '../api';
import { Disclosure, Empty, ErrorNote, PageHeader } from '../components';
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

  return (
    <div className="page">
      <PageHeader
        title="Memory"
        subtitle={
          <>
            Notes your assistants keep and update as they learn, like what they know about you. You can read and correct them
            here. Turn memory on for an assistant in its settings (Advanced).
          </>
        }
      />
      <ErrorNote error={blocks.error} />
      <div className="row between">
        <Tabs
          items={[
            { id: 'all', label: 'All', badge: blocks.data?.length },
            ...['agent', 'user', 'shared'].map((k) => ({ id: k, label: KIND_LABEL[k], badge: counts(k) })),
          ]}
          value={kind}
          onValueChange={setKind}
          variant="pill"
          aria-label="Filter blocks"
        />
      </div>
      {groups.length === 0 && blocks.data && (
        <Empty title="No memories yet">
          Assistants with memory turned on (like the “Remembers you” template) keep their notes here. You can also add one below.
        </Empty>
      )}
      {groups.map(([scope, list]) => (
        <section key={scope} className="stack tight">
          <h3 className="mono" style={{ margin: '8px 0 0' }}>
            {scope}
          </h3>
          <div className="block-grid">
            {list.map((b) => (
              <BlockCard key={`${b.id}:${b.version}`} b={b} onChange={blocks.reload} />
            ))}
          </div>
        </section>
      ))}
      <section className="card">
        <Disclosure summary={<strong>New block</strong>}>
          <NewBlock onCreated={blocks.reload} />
        </Disclosure>
      </section>
    </div>
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
    if (!confirm(`Delete the ${b.label} block in ${b.scope}? Agents that declare it start again from its starting value.`)) return;
    await api.deleteBlock(b.id).catch((e: Error) => toast.error(e.message));
    onChange();
  }

  return (
    <div className="card stack tight block-card">
      <div className="row between">
        <div className="row gap">
          <strong className="mono">{b.label}</strong>
          <span className="chip small">v{b.version}</span>
          <span className="muted small">{ago(b.updated_at)}</span>
        </div>
        <button type="button" className="icon-btn" aria-label={`Delete ${b.label}`} onClick={remove}>
          <IconTrash />
        </button>
      </div>
      {b.description && <span className="muted small">{b.description}</span>}
      <textarea
        className="code-editor small-editor"
        aria-label={`${b.label} value`}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="(empty)"
      />
      <div className="meter" aria-label={`${used} of ${b.limit} characters`}>
        <div className="meter-fill" data-over={over || undefined} style={{ width: `${Math.min(100, (used / b.limit) * 100)}%` }} />
      </div>
      <div className="row between">
        <span className={`small num ${over ? 'danger' : 'muted'}`}>
          {used} / {b.limit} characters
        </span>
        <div className="row gap">
          <Switch label="Read-only" size="sm" checked={b.read_only} onChange={(e) => save({ read_only: e.target.checked })} />
          {dirty && (
            <Button size="sm" variant="ghost" onClick={() => setValue(b.value)}>
              Reset
            </Button>
          )}
          <Button size="sm" tone="accent" loading={saving} disabled={!dirty || over} onClick={() => save({ value })}>
            Save
          </Button>
        </div>
      </div>
      <button type="button" className="link small" style={{ alignSelf: 'flex-start' }} onClick={() => setShowHistory(!showHistory)}>
        {showHistory ? 'Hide history' : 'History'}
      </button>
      {showHistory && (
        <ol className="revisions">
          {history.data?.map((r) => (
            <li key={r.version}>
              <div className="row gap small">
                <span className="chip small">v{r.version}</span>
                <strong>{r.op}</strong>
                <span className="muted mono">{r.actor ?? ''}</span>
                <span className="muted">{ago(r.created_at)}</span>
              </div>
              <div className="pre small muted">{r.value || '(empty)'}</div>
            </li>
          ))}
        </ol>
      )}
    </div>
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
    <div className="mcp-form">
      <TextField label="Scope" description="agent:<name>, user:<id> or shared:<name>" value={scope} onChange={(e) => setScope(e.target.value)} />
      <TextField label="Label" placeholder="preferences" value={label} onChange={(e) => setLabel(e.target.value)} />
      <TextField label="Limit (characters)" inputMode="numeric" value={limit} onChange={(e) => setLimit(e.target.value.replace(/\D/g, ''))} />
      <TextField label="Description" placeholder="What the agent should keep here" value={description} onChange={(e) => setDescription(e.target.value)} />
      <div>
        <Button tone="accent" leading={<IconPlus />} loading={busy} disabled={!label.trim() || !scope.includes(':')} onClick={create}>
          Create block
        </Button>
      </div>
    </div>
  );
}
