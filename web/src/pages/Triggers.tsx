import { useEffect, useState } from 'react';
import { CalendarClockIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SwitchField } from '@/components/fields';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Item, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { api, type TriggerView } from '../api';
import { ConfirmAction, Details, EmptyState, ErrorNote, LinkItem, Page, PageHeader, Snippet, StatusPill } from '@/components/shared';
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
    <Page wide>
      <PageHeader
        title="Schedules & webhooks"
        subtitle="Run an assistant or workflow by itself: every morning, when another app calls a link, or when files change."
        actions={Object.keys(TRIGGER_TEMPLATES).map((k) => (
          <Button variant="outline" size="sm" key={k} onClick={() => setCreating(k)}>
            <IconPlus data-icon="inline-start" />
            {k === 'cron' ? 'Schedule' : k === 'webhook' ? 'Webhook' : 'File watcher'}
          </Button>
        ))}
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[18rem_minmax(0,1fr)]">
        <ItemGroup className="gap-2">
          {list.data?.map((t) => (
            <LinkItem
              key={t.id}
              href={`#/schedules/${t.id}`}
              active={current?.id === t.id && !creating}
              onClick={() => setCreating(null)}
              title={
                <>
                  <span className="truncate">{t.name}</span>
                  <Badge variant={t.enabled ? 'secondary' : 'outline'} className="ml-auto">
                    {t.enabled ? t.kind : 'Off'}
                  </Badge>
                </>
              }
            >
              <ItemDescription>
                {t.kind === 'cron' && t.next_fire_at
                  ? `Next ${new Date(t.next_fire_at).toLocaleString()}`
                  : (t.schedule_text ?? t.webhook_path ?? '')}
                {t.last_fired_at && ` · fired ${ago(t.last_fired_at)}`}
              </ItemDescription>
            </LinkItem>
          ))}
          {list.data?.length === 0 && !creating && (
            <EmptyState title="Nothing scheduled yet" icon={<CalendarClockIcon />}>
              Add a schedule, a webhook or a file watcher with the buttons above.
            </EmptyState>
          )}
        </ItemGroup>
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
          <Card>
            <CardContent>
              <EmptyState title="Pick one">Or add one with the buttons above.</EmptyState>
            </CardContent>
          </Card>
        )}
      </div>
    </Page>
  );
}

function Editor({
  initial,
  existing,
  onSaved,
}: {
  initial: TriggerDraft;
  existing?: TriggerView;
  onSaved: (t: TriggerView, secret?: string | null) => void;
}) {
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
    <Card>
      <CardHeader>
        <CardTitle>{existing ? `Edit ${existing.name}` : 'New trigger'}</CardTitle>
      </CardHeader>
      <CardContent>
        <SpecEditor label="Trigger" value={draft} onChange={setDraft}>
          <TriggerForm draft={draft} onChange={setDraft} isNew={!existing} />
        </SpecEditor>
      </CardContent>
      <CardFooter className="sticky bottom-0 flex-col items-stretch gap-3 border-t bg-card">
        <ErrorNote error={error} />
        <Button className="self-end" onClick={save} disabled={busy}>
          {busy && <Spinner data-icon="inline-start" />}
          {existing ? 'Save' : 'Create trigger'}
        </Button>
      </CardFooter>
    </Card>
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
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">{t.name}</CardTitle>
          <CardAction className="flex flex-wrap items-center gap-2">
            <SwitchField
              size="sm"
              className="w-auto"
              label={t.enabled ? 'On' : 'Off'}
              checked={t.enabled}
              onCheckedChange={async (checked) => {
                await (checked ? api.enableTrigger(t.id) : api.disableTrigger(t.id)).catch((x: Error) => toast.error(x.message));
                onChange();
              }}
            />
            <Button variant="outline" size="sm" onClick={fire} disabled={firing}>
              {firing && <Spinner data-icon="inline-start" />}
              Fire now
            </Button>
            <Button variant="outline" size="sm" onClick={() => setEditing(true)}>
              Edit
            </Button>
            <ConfirmAction
              title={`Delete ${t.name}?`}
              description="It stops firing. Runs it already started are kept."
              action="Delete"
              destructive
              onConfirm={async () => {
                await api.deleteTrigger(t.id).catch((e: Error) => toast.error(e.message));
                navigate('/schedules');
                onChange();
              }}
            >
              <Button variant="destructive" size="sm">
                <IconTrash data-icon="inline-start" />
                Delete
              </Button>
            </ConfirmAction>
          </CardAction>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <Details
            items={[
              ['Kind', t.kind],
              ...(t.schedule_text ? [['Schedule', t.schedule_text] as [string, string]] : []),
              ...(t.next_fire_at && t.enabled ? [['Next', new Date(t.next_fire_at).toLocaleString()] as [string, string]] : []),
              ['Last fired', t.last_fired_at ? `${new Date(t.last_fired_at).toLocaleString()} (${ago(t.last_fired_at)})` : 'Never'],
            ]}
          />
          {hookUrl && (
            <div className="flex flex-col gap-2">
              <Snippet
                title="Webhook"
                code={`curl -X POST ${hookUrl} \\
  -H 'content-type: application/json' \\
  -H 'x-gai-token: ${secret ?? '<secret>'}' \\
  -d '{"hello": "world"}'`}
              />
              <div className="flex flex-wrap items-center gap-2">
                {!secret && (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api
                        .triggerSecret(t.id)
                        .then((r) => setSecret(r.secret))
                        .catch((e: Error) => toast.error(e.message))
                    }
                  >
                    Show secret
                  </Button>
                )}
                <span className="text-sm text-muted-foreground">
                  Or sign the body: X-Gai-Signature (or GitHub&apos;s X-Hub-Signature-256): sha256=&lt;hex HMAC-SHA256 of the body&gt;.
                </span>
              </div>
            </div>
          )}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Firings</CardTitle>
        </CardHeader>
        <CardContent>
          {events.data?.length === 0 && <p className="text-sm text-muted-foreground">Nothing yet.</p>}
          <ItemGroup className="gap-1">
            {events.data?.map((e) => {
              const row = (
                <>
                  <ItemContent>
                    <ItemTitle>
                      <Badge variant={e.status === 'started' ? 'secondary' : e.status === 'skipped' ? 'outline' : 'destructive'}>
                        {e.status}
                      </Badge>
                      {e.reason}
                    </ItemTitle>
                    {(e.error || e.run_id) && <ItemDescription className="font-mono">{e.error ?? e.run_id}</ItemDescription>}
                  </ItemContent>
                  <span className="text-sm text-muted-foreground">{ago(e.fired_at)}</span>
                </>
              );
              return e.run_id ? (
                <Item key={e.id} asChild size="xs">
                  <a href={`#/activity/${e.run_id}`}>{row}</a>
                </Item>
              ) : (
                <Item key={e.id} size="xs">
                  {row}
                </Item>
              );
            })}
          </ItemGroup>
        </CardContent>
      </Card>
      <RunsOf trigger={t.name} />
    </div>
  );
}

/** Runs this trigger started, with their status. */
function RunsOf({ trigger }: { trigger: string }) {
  const runs = useLoad(
    () => api.runs({ limit: 100 }).then((rs) => rs.filter((r) => r.metadata.trigger === trigger).slice(0, 10)),
    [trigger],
  );
  if (!runs.data?.length) return null;
  return (
    <Card>
      <CardHeader>
        <CardTitle>Recent runs</CardTitle>
      </CardHeader>
      <CardContent>
        <ItemGroup className="gap-1">
          {runs.data.map((r) => (
            <Item key={r.id} asChild size="xs">
              <a href={`#/activity/${r.id}`}>
                <StatusPill status={r.status} />
                <ItemContent>
                  <ItemTitle>{r.agent}</ItemTitle>
                </ItemContent>
                <span className="text-sm text-muted-foreground">
                  {r.steps} steps · {ago(r.created_at)}
                </span>
              </a>
            </Item>
          ))}
        </ItemGroup>
      </CardContent>
    </Card>
  );
}
