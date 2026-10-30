import { useEffect, useState } from 'react';
import { BellIcon, BellOffIcon, SendIcon } from 'lucide-react';
import { toast } from 'sonner';
import { CheckboxField, SwitchField, TextField } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldDescription, FieldLabel } from '@/components/ui/field';
import { api, type NotificationRule, type NotifyEvent } from '../api';
import { IconPlus, IconTrash } from '../icons';
import { useLoad } from '../util';

const EVENTS: { id: NotifyEvent; label: string; hint: string }[] = [
  { id: 'approval', label: 'Something needs an OK', hint: 'An assistant or automation waits for approval.' },
  { id: 'run_failed', label: 'A run failed', hint: 'A chat, scheduled run or automation ended with an error.' },
  { id: 'run_finished', label: 'A run finished', hint: 'Every finished run. Narrow it to some assistants below.' },
  { id: 'alert', label: 'A dashboard alert', hint: 'A watched number crossed its limit.' },
  { id: 'suggestion', label: 'A new suggestion', hint: 'An assistant watching your data suggests something.' },
];

const BLANK: NotificationRule = { name: '', on: ['approval'], push: true, enabled: true };

const list = (s: string) =>
  s
    .split(/[,\s]+/)
    .map((x) => x.trim())
    .filter(Boolean);

function urlBase64ToBytes(s: string): Uint8Array<ArrayBuffer> {
  const pad = '='.repeat((4 - (s.length % 4)) % 4);
  const raw = atob((s + pad).replace(/-/g, '+').replace(/_/g, '/'));
  const out = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

const pushSupported = () => 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;

/** This browser's push subscription, if it has one. */
async function currentSubscription(): Promise<PushSubscription | null> {
  if (!pushSupported()) return null;
  const reg = await navigator.serviceWorker.getRegistration('/');
  return (await reg?.pushManager.getSubscription()) ?? null;
}

/** Turn browser notifications on or off for this browser. */
function ThisBrowser({ onChange }: { onChange: () => void }) {
  const [sub, setSub] = useState<PushSubscription | null | undefined>();
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    currentSubscription()
      .then(setSub)
      .catch(() => setSub(null));
  }, []);

  async function turnOn() {
    setBusy(true);
    try {
      if ((await Notification.requestPermission()) !== 'granted') {
        toast.error('The browser did not allow notifications. Allow them in its site settings and try again.');
        return;
      }
      const reg = await navigator.serviceWorker.register('/sw.js', { scope: '/' });
      await navigator.serviceWorker.ready;
      const { public_key } = await api.pushInfo();
      const s = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: urlBase64ToBytes(public_key) });
      await api.subscribePush(s.toJSON());
      setSub(s);
      toast.success('This browser will get notifications');
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function turnOff() {
    if (!sub) return;
    setBusy(true);
    try {
      await api.unsubscribePush(sub.endpoint);
      await sub.unsubscribe();
      setSub(null);
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>This browser</CardTitle>
        <CardDescription>
          {!pushSupported()
            ? 'This browser cannot show notifications from websites.'
            : sub
              ? 'Notifications from rules with "Browser" turned on show up here, even when gai is closed.'
              : 'Turn on to get notifications from rules with "Browser" turned on.'}
        </CardDescription>
        <CardAction>
          {pushSupported() &&
            sub !== undefined &&
            (sub ? (
              <Button variant="outline" disabled={busy} onClick={turnOff}>
                <BellOffIcon data-icon="inline-start" />
                Turn off
              </Button>
            ) : (
              <Button disabled={busy} onClick={turnOn}>
                <BellIcon data-icon="inline-start" />
                Turn on
              </Button>
            ))}
        </CardAction>
      </CardHeader>
      {!window.isSecureContext && (
        <CardContent>
          <FieldDescription>Browser notifications need https (or localhost).</FieldDescription>
        </CardContent>
      )}
    </Card>
  );
}

function RuleCard({ rule, onSaved, onDelete }: { rule: NotificationRule; onSaved: () => void; onDelete?: () => void }) {
  const [r, setR] = useState(rule);
  const [busy, setBusy] = useState(false);
  const isNew = !rule.name;
  const set = (patch: Partial<NotificationRule>) => setR((x) => ({ ...x, ...patch }));

  async function save() {
    setBusy(true);
    try {
      const clean: NotificationRule = { ...r, email: r.email?.length ? r.email : undefined, agents: r.agents?.length ? r.agents : undefined };
      for (const k of ['slack', 'slack_env', 'webhook', 'webhook_secret_env', 'description'] as const) {
        if (!clean[k]) delete clean[k];
      }
      await api.saveNotificationRule(clean);
      toast.success(`Saved ${r.name}`);
      onSaved();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function test() {
    try {
      const sent = await api.testNotificationRule(r.name);
      if (sent.error) toast.error(sent.error);
      else if (sent.to.length === 0) toast.info('Nothing to send to yet: no browser has turned notifications on.');
      else toast.success(`Sent: ${sent.to.join(', ')}`);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const toggle = (id: NotifyEvent, on: boolean) => set({ on: on ? [...r.on, id] : r.on.filter((e) => e !== id) });

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          {isNew ? 'New rule' : r.name}
          {!isNew && r.enabled === false && <Badge variant="secondary">Off</Badge>}
        </CardTitle>
        {r.description && <CardDescription>{r.description}</CardDescription>}
        {!isNew && (
          <CardAction>
            <SwitchField aria-label="On" checked={r.enabled !== false} onCheckedChange={(c) => set({ enabled: c })} />
          </CardAction>
        )}
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {isNew && <TextField label="Name" placeholder="on-call" value={r.name} onChange={(e) => set({ name: e.target.value })} />}
        <div className="flex flex-col gap-2">
          <FieldLabel>When</FieldLabel>
          <div className="grid gap-2 sm:grid-cols-2">
            {EVENTS.map((e) => (
              <CheckboxField key={e.id} label={e.label} description={e.hint} checked={r.on.includes(e.id)} onCheckedChange={(c) => toggle(e.id, c)} />
            ))}
          </div>
        </div>
        <TextField
          label="Only for"
          description="Assistant, automation or dashboard names. Leave empty for all."
          placeholder="triage, weekly-report"
          value={(r.agents ?? []).join(', ')}
          onChange={(e) => set({ agents: list(e.target.value) })}
        />
        <div className="grid gap-4 sm:grid-cols-2">
          <TextField
            label="Email"
            placeholder="ops@example.com"
            description="Needs [notify] smtp_url in gai.toml."
            value={(r.email ?? []).join(', ')}
            onChange={(e) => set({ email: list(e.target.value) })}
          />
          <TextField
            label="Slack"
            placeholder="https://hooks.slack.com/services/…"
            description="An incoming webhook URL."
            value={r.slack ?? ''}
            onChange={(e) => set({ slack: e.target.value })}
          />
          <TextField
            label="Webhook"
            placeholder="https://example.com/hooks/gai"
            description="Gets the notification as JSON."
            value={r.webhook ?? ''}
            onChange={(e) => set({ webhook: e.target.value })}
          />
          <SwitchField
            label="Browser"
            description="Every browser that turned notifications on."
            checked={!!r.push}
            onCheckedChange={(c) => set({ push: c })}
          />
        </div>
      </CardContent>
      <CardFooter className="gap-2">
        <Button disabled={busy || !r.name || r.on.length === 0} onClick={save}>
          Save
        </Button>
        {!isNew && (
          <Button variant="outline" onClick={test}>
            <SendIcon data-icon="inline-start" />
            Send a test
          </Button>
        )}
        {onDelete && (
          <ConfirmAction title={`Delete ${r.name}?`} description="Nobody will hear about these events through it." action="Delete" destructive onConfirm={onDelete}>
            <Button variant="ghost" className="ml-auto">
              <IconTrash data-icon="inline-start" />
              Delete
            </Button>
          </ConfirmAction>
        )}
      </CardFooter>
    </Card>
  );
}

/** Settings → Notifications: who hears about approvals, failures and alerts. */
export function NotificationsPage() {
  const rules = useLoad(() => api.notificationRules(), []);
  const [adding, setAdding] = useState(false);

  async function remove(name: string) {
    try {
      await api.deleteNotificationRule(name);
      rules.reload();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <Page className="max-w-4xl">
      <PageHeader
        title="Notifications"
        subtitle="Hear about approvals, failed runs and alerts by email, Slack or in this browser."
        actions={
          <Button onClick={() => setAdding(true)} disabled={adding}>
            <IconPlus data-icon="inline-start" />
            New rule
          </Button>
        }
      />
      <ErrorNote error={rules.error} />
      <ThisBrowser onChange={rules.reload} />
      {adding && (
        <RuleCard
          rule={BLANK}
          onSaved={() => {
            setAdding(false);
            rules.reload();
          }}
        />
      )}
      {rules.data?.length === 0 && !adding && (
        <EmptyState title="No rules yet" icon={<BellIcon />}>
          Add a rule to hear when something needs your OK, a run fails or a dashboard alert fires.
        </EmptyState>
      )}
      {rules.data?.map((r) => (
        <RuleCard key={r.name} rule={r} onSaved={rules.reload} onDelete={() => remove(r.name)} />
      ))}
    </Page>
  );
}
