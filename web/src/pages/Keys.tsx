import { useState } from 'react';
import { Button, Checkbox, TextField, toast } from '@flame';
import { api, getApiKey, setApiKey, type Scope } from '../api';
import { Empty, ErrorNote, PageHeader } from '../components';
import { IconPlus, IconTrash } from '../icons';
import { ago, money, useLoad } from '../util';

const SCOPES: { id: Scope; label: string; description: string }[] = [
  { id: 'read', label: 'Read', description: 'View runs, sessions, agents and usage' },
  { id: 'run', label: 'Run', description: 'Start runs, chat and answer approvals' },
  { id: 'admin', label: 'Admin', description: 'Everything, including agents and keys' },
];

export function Keys() {
  const list = useLoad(() => api.keys(), []);
  const [name, setName] = useState('');
  const [scopes, setScopes] = useState<Scope[]>(['run']);
  const [budget, setBudget] = useState('');
  const [creating, setCreating] = useState(false);
  const [secret, setSecret] = useState<string | null>(null);

  async function create() {
    if (!name.trim() || scopes.length === 0) return;
    setCreating(true);
    try {
      const r = await api.createKey(name.trim(), scopes, budget ? Number(budget) : undefined);
      // The first key switches authentication on. Keep this browser signed
      // in by adopting the new key rather than locking it out.
      if (!getApiKey()) {
        setApiKey(r.secret);
        toast.info('This browser now signs in with the new key.');
      }
      setSecret(r.secret);
      setName('');
      setBudget('');
      list.reload();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setCreating(false);
    }
  }

  async function revoke(id: string, keyName: string) {
    if (!confirm(`Revoke ${keyName}? Anything using it stops working.`)) return;
    await api.revokeKey(id).catch((e: Error) => toast.error(e.message));
    list.reload();
  }

  const active = list.data?.filter((k) => !k.revoked_at) ?? [];
  const revoked = list.data?.filter((k) => k.revoked_at) ?? [];

  return (
    <div className="page">
      <PageHeader
        title="API keys"
        subtitle={
          <>
            Keys authenticate the API, this dashboard and OpenAI-compatible clients (<span className="mono">/openai/v1</span>). While no key
            exists, a local server stays open; the first key turns authentication on.
          </>
        }
      />
      <ErrorNote error={list.error} />

      {secret && (
        <section className="card secret-card">
          <h3>New key</h3>
          <p>Copy it now. It will not be shown again.</p>
          <div className="row gap">
            <code className="secret mono">{secret}</code>
            <Button
              size="sm"
              onClick={() => navigator.clipboard.writeText(secret).then(() => toast.success('Copied'), () => toast.error('Copy failed'))}
            >
              Copy
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setSecret(null)}>
              Done
            </Button>
          </div>
        </section>
      )}

      <div className="keys-layout">
        <section className="card">
          <h3>Create a key</h3>
          <div className="stack">
            <TextField label="Name" placeholder="e.g. ci, laptop, slack-bot" value={name} onChange={(e) => setName(e.target.value)} />
            <div>
              <div className="label">Scopes</div>
              <div className="stack tight">
                {SCOPES.map((s) => (
                  <Checkbox
                    key={s.id}
                    label={s.label}
                    description={s.description}
                    checked={scopes.includes(s.id)}
                    onChange={(e) => setScopes((cur) => (e.target.checked ? [...cur, s.id] : cur.filter((x) => x !== s.id)))}
                  />
                ))}
              </div>
            </div>
            <TextField
              label="Budget (USD, optional)"
              description="New runs are refused once the key has spent this much."
              inputMode="decimal"
              value={budget}
              onChange={(e) => setBudget(e.target.value.replace(/[^0-9.]/g, ''))}
            />
            <Button tone="accent" leading={<IconPlus />} loading={creating} disabled={!name.trim() || !scopes.length} onClick={create}>
              Create key
            </Button>
          </div>
        </section>

        <section className="card">
          <h3>Active</h3>
          {active.length === 0 && <Empty title="No keys yet">Authentication is open on a local server until you create one.</Empty>}
          <div className="key-list">
            {active.map((k) => (
              <div key={k.id} className="key-row">
                <div>
                  <strong>{k.name}</strong>
                  <div className="muted small mono">{k.prefix}…</div>
                </div>
                <div className="row gap wrap">
                  {k.scopes.map((s) => (
                    <span key={s} className="chip small">
                      {s}
                    </span>
                  ))}
                </div>
                <div className="small num">
                  {money(k.spent_usd)}
                  {k.budget_usd != null && <span className="muted"> / {money(k.budget_usd)}</span>}
                </div>
                <div className="muted small">{k.last_used_at ? `used ${ago(k.last_used_at)}` : 'never used'}</div>
                <button type="button" className="icon-btn" aria-label={`Revoke ${k.name}`} onClick={() => revoke(k.id, k.name)}>
                  <IconTrash />
                </button>
              </div>
            ))}
          </div>
          {revoked.length > 0 && (
            <p className="muted small">
              {revoked.length} revoked key{revoked.length === 1 ? '' : 's'} not shown.
            </p>
          )}
        </section>
      </div>
    </div>
  );
}
