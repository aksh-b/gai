import { useEffect, useState } from 'react';
import { Button, Switch, toast } from '@flame';
import { api, type TriggerView } from '../api';
import { Empty, ErrorNote, PageHeader, StatusPill } from '../components';
import { IconPlus, IconTrash } from '../icons';
import { navigate } from '../router';
import { ago, useLoad } from '../util';
import { SpecEditor } from '../forms';
import { TRIGGER_TEMPLATES, TriggerForm, type TriggerDraft } from '../TriggerForm';

/** Webhook secrets shown right after creating, kept in memory only. */
const freshSecrets = new Map<string, string>();

export function Triggers({ selected }: { selected?: string }) {
  const list = useLoad(() => api.triggers(), []);
  const [creating, setCreating] = useState<string | null>(null);
  const current = list.data?.find((t) => t.id === selected || t.name === selected);

  return (
    <div className="page">
      <PageHeader
        title="Schedules & webhooks"
        subtitle="Run an assistant or workflow by itself: every morning, when another app calls a link, or when files change."
        actions={
          <div className="row gap">
            {Object.keys(TRIGGER_TEMPLATES).map((k) => (
              <Button key={k} variant="outline" size="sm" leading={<IconPlus />} onClick={() => setCreating(k)}>
                {k === 'cron' ? 'Schedule' : k === 'webhook' ? 'Webhook' : 'File watcher'}
              </Button>
            ))}
          </div>
        }
      />
      <ErrorNote error={list.error} />
      <div className="agents-layout">
        <div className="agent-list">
          {list.data?.map((t) => (
            <a
              key={t.id}
              href={`#/schedules/${t.id}`}
              className="agent-card"
              aria-current={current?.id === t.id && !creating ? 'page' : undefined}
              onClick={() => setCreating(null)}
            >
              <div className="row between">
                <strong>{t.name}</strong>
                <span className={`chip small${t.enabled ? ' accent' : ''}`}>{t.enabled ? t.kind : 'off'}</span>
              </div>
              <span className="muted small">
                {t.kind === 'cron' && t.next_fire_at ? `next ${new Date(t.next_fire_at).toLocaleString()}` : (t.schedule_text ?? t.webhook_path ?? '')}
              </span>
              {t.last_fired_at && <span className="muted small">fired {ago(t.last_fired_at)}</span>}
            </a>
          ))}
          {list.data?.length === 0 && !creating && (
            <Empty title="Nothing scheduled yet">Add a schedule, a webhook or a file watcher with the buttons above.</Empty>
          )}
        </div>
        {creating ? (
          <Editor
            key={creating}
            initial={TRIGGER_TEMPLATES[creating]}
            onSaved={(t, secret) => {
              setCreating(null);
              list.reload();
              if (secret) freshSecrets.set(t.id, secret);
              navigate(`/schedules/${t.id}`);
            }}
          />
        ) : current ? (
          <Detail key={current.id} t={current} onChange={list.reload} />
        ) : (
          <section className="card">
            <Empty title="Pick one">Or add one with the buttons above.</Empty>
          </section>
        )}
      </div>
    </div>
  );
}

function Editor({ initial, existing, onSaved }: { initial: TriggerDraft; existing?: TriggerView; onSaved: (t: TriggerView, secret?: string | null) => void }) {
  const [draft, setDraft] = useState<TriggerDraft>(initial);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  async function save() {
    if (!draft.name?.trim()) {
      setError('Give the trigger a name.');
      return;
    }
    setError(undefined);
    setBusy(true);
    try {
      if (existing) {
        let t = await api.updateTrigger(existing.id, draft);
        const enabled = draft.enabled !== false;
        if (enabled !== t.enabled) t = await (enabled ? api.enableTrigger(t.id) : api.disableTrigger(t.id));
        onSaved(t);
      } else {
        const r = await api.createTrigger(draft);
        onSaved(r.trigger, r.secret);
      }
      toast.success('Saved');
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card editor">
      <h3>{existing ? `Edit ${existing.name}` : 'New trigger'}</h3>
      <SpecEditor label="Trigger" value={draft} onChange={setDraft}>
        <TriggerForm draft={draft} onChange={setDraft} isNew={!existing} />
      </SpecEditor>
      <div className="save-bar">
        <ErrorNote error={error} />
        <div className="row gap end">
          <Button tone="accent" loading={busy} onClick={save}>
            {existing ? 'Save' : 'Create trigger'}
          </Button>
        </div>
      </div>
    </section>
  );
}

function Detail({ t, onChange }: { t: TriggerView; onChange: () => void }) {
  const events = useLoad(() => api.triggerEvents(t.id), [t.id, t.last_fired_at]);
  const [editing, setEditing] = useState(false);
  const [secret, setSecret] = useState<string | null>(() => freshSecrets.get(t.id) ?? null);
  const [firing, setFiring] = useState(false);

  // Follow new firings while the page is open.
  useEffect(() => {
    const i = setInterval(() => {
      events.reload();
      onChange();
    }, 5000);
    return () => clearInterval(i);
  }, [t.id]);

  async function fire() {
    setFiring(true);
    try {
      const f = await api.fireTrigger(t.id);
      if (f.status === 'started') toast.success('Started a run');
      else toast.error(`${f.status}${f.error ? `: ${f.error}` : ''}`);
      events.reload();
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setFiring(false);
    }
  }

  if (editing) {
    return (
      <Editor
        initial={{ ...(t.spec as TriggerDraft), ...(t.enabled ? {} : { enabled: false }) }}
        existing={t}
        onSaved={() => {
          setEditing(false);
          onChange();
        }}
      />
    );
  }
  const hookUrl = t.webhook_path ? `${location.origin}${t.webhook_path}` : null;
  return (
    <section className="stack">
      <div className="card stack tight">
        <div className="row between wrap">
          <h2 style={{ margin: 0, whiteSpace: 'nowrap' }}>{t.name}</h2>
          <div className="row gap wrap">
            <Switch
              size="sm"
              label={t.enabled ? 'On' : 'Off'}
              checked={t.enabled}
              onChange={async (e) => {
                await (e.target.checked ? api.enableTrigger(t.id) : api.disableTrigger(t.id)).catch((x: Error) => toast.error(x.message));
                onChange();
              }}
            />
            <Button size="sm" variant="outline" loading={firing} onClick={fire}>
              Fire now
            </Button>
            <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
              Edit
            </Button>
            <Button
              size="sm"
              variant="ghost"
              tone="danger"
              leading={<IconTrash />}
              onClick={async () => {
                if (!confirm(`Delete ${t.name}?`)) return;
                await api.deleteTrigger(t.id).catch((e: Error) => toast.error(e.message));
                navigate('/schedules');
                onChange();
              }}
            >
              Delete
            </Button>
          </div>
        </div>
        <div className="kv">
          <span className="muted">Kind</span>
          <span>{t.kind}</span>
          {t.schedule_text && (
            <>
              <span className="muted">Schedule</span>
              <span>{t.schedule_text}</span>
            </>
          )}
          {t.next_fire_at && t.enabled && (
            <>
              <span className="muted">Next</span>
              <span>{new Date(t.next_fire_at).toLocaleString()}</span>
            </>
          )}
          <span className="muted">Last fired</span>
          <span>{t.last_fired_at ? `${new Date(t.last_fired_at).toLocaleString()} (${ago(t.last_fired_at)})` : 'never'}</span>
        </div>
        {hookUrl && (
          <div className="stack tight">
            <span className="label">Webhook</span>
            <pre className="code">{`curl -X POST ${hookUrl} \\
  -H 'content-type: application/json' \\
  -H 'x-gai-token: ${secret ?? '<secret>'}' \\
  -d '{"hello": "world"}'`}</pre>
            <div className="row gap">
              {!secret && (
                <Button size="sm" variant="outline" onClick={() => api.triggerSecret(t.id).then((r) => setSecret(r.secret)).catch((e: Error) => toast.error(e.message))}>
                  Show secret
                </Button>
              )}
              <span className="muted small">Or sign the body: X-Gai-Signature (or GitHub&apos;s X-Hub-Signature-256): sha256=&lt;hex HMAC-SHA256 of the body&gt;.</span>
            </div>
          </div>
        )}
      </div>
      <section className="card flush">
        <h3 className="pad" style={{ margin: 0 }}>
          Firings
        </h3>
        {events.data?.length === 0 && <p className="muted pad">Nothing yet.</p>}
        {events.data?.map((e) => (
          <a key={e.id} className="run-row" href={e.run_id ? `#/activity/${e.run_id}` : undefined}>
            <span className="pill" data-tone={e.status === 'started' ? 'ok' : e.status === 'skipped' ? 'warn' : 'bad'}>
              {e.status}
            </span>
            <span>
              {e.reason}
              {e.error && <span className="muted small"> · {e.error}</span>}
            </span>
            <span className="muted small mono">{e.run_id ?? ''}</span>
            <span />
            <span className="muted small">{ago(e.fired_at)}</span>
          </a>
        ))}
      </section>
      <RunsOf trigger={t.name} />
    </section>
  );
}

/** Runs this trigger started, with their status. */
function RunsOf({ trigger }: { trigger: string }) {
  const runs = useLoad(() => api.runs({ limit: 100 }).then((rs) => rs.filter((r) => r.metadata.trigger === trigger).slice(0, 10)), [trigger]);
  if (!runs.data?.length) return null;
  return (
    <section className="card flush">
      <h3 className="pad" style={{ margin: 0 }}>
        Recent runs
      </h3>
      {runs.data.map((r) => (
        <a key={r.id} className="run-row" href={`#/activity/${r.id}`}>
          <StatusPill status={r.status} />
          <span>{r.agent}</span>
          <span className="muted small">{r.steps} steps</span>
          <span />
          <span className="muted small">{ago(r.created_at)}</span>
        </a>
      ))}
    </section>
  );
}
