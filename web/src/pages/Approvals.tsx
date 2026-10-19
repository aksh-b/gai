import { useEffect, useState } from 'react';
import { CircleCheckIcon } from 'lucide-react';
import { toast } from 'sonner';
import { EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Field, FieldLabel } from '@/components/ui/field';
import { Spinner } from '@/components/ui/spinner';
import { Textarea } from '@/components/ui/textarea';
import { api, type PendingApproval } from '../api';
import { IconCheck, IconX } from '../icons';
import { toolSentence } from '../labels';
import { useLoad } from '../util';

function safeJson(t: string): unknown {
  try {
    return JSON.parse(t);
  } catch {
    return {};
  }
}

/** Every tool call waiting for a human, across all runs. */
export function Approvals() {
  const { data, error, reload } = useLoad(() => api.approvals(), []);

  useEffect(() => {
    const t = setInterval(reload, 2000);
    return () => clearInterval(t);
  }, [reload]);

  return (
    <Page>
      <PageHeader
        title="Needs your OK"
        subtitle="Things your assistants want to do but asked about first. They wait here, even if the server restarts."
      />
      <ErrorNote error={error} />
      {data && data.length === 0 && (
        <EmptyState title="Nothing waiting" icon={<CircleCheckIcon />}>
          When an assistant asks before acting, the question shows up here and in the chat.
        </EmptyState>
      )}
      <div className="flex flex-col gap-3">
        {data?.map((p) => (
          <ApprovalCard key={`${p.run_id}/${p.call_id}`} p={p} onDone={reload} />
        ))}
      </div>
    </Page>
  );
}

function ApprovalCard({ p, onDone }: { p: PendingApproval; onDone: () => void }) {
  const [text, setText] = useState(() => JSON.stringify(p.input, null, 2));
  const [busy, setBusy] = useState<'approve' | 'decline' | null>(null);
  const edited = text !== JSON.stringify(p.input, null, 2);

  async function approve() {
    let input: unknown;
    try {
      input = JSON.parse(text);
    } catch {
      toast.error('The input is not valid JSON.');
      return;
    }
    setBusy('approve');
    try {
      if (edited) await api.decideWith(p.run_id, p.call_id, input);
      else await api.decide(p.run_id, p.call_id, true);
      toast.success('Allowed');
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function decline() {
    setBusy('decline');
    try {
      await api.decide(p.run_id, p.call_id, false, 'declined in the dashboard');
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle title={p.tool}>{toolSentence(p.tool, safeJson(text))}</CardTitle>
        <CardDescription>
          <a href={`#/activity/${p.run_id}`} className="underline underline-offset-4">
            See what led to this
          </a>
        </CardDescription>
        <CardAction className="flex gap-2">
          <Button onClick={approve} disabled={busy === 'approve'}>
            {busy === 'approve' ? <Spinner data-icon="inline-start" /> : <IconCheck data-icon="inline-start" />}
            {edited ? 'Allow with my changes' : 'Allow'}
          </Button>
          <Button variant="outline" onClick={decline} disabled={busy === 'decline'}>
            {busy === 'decline' ? <Spinner data-icon="inline-start" /> : <IconX data-icon="inline-start" />}
            Don't allow
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent>
        <Field>
          <FieldLabel htmlFor={`in-${p.call_id}`}>
            Details (you can change them)
            {edited && <Badge variant="secondary">Changed</Badge>}
          </FieldLabel>
          <Textarea
            id={`in-${p.call_id}`}
            className="max-h-64 min-h-24 font-mono"
            spellCheck={false}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
        </Field>
      </CardContent>
    </Card>
  );
}
