import { useEffect, useState } from 'react';
import { Button, toast } from '@flame';
import { api, type PendingApproval } from '../api';
import { Empty, ErrorNote, PageHeader } from '../components';
import { toolSentence } from '../labels';

function safeJson(t: string): unknown {
  try {
    return JSON.parse(t);
  } catch {
    return {};
  }
}
import { IconCheck, IconX } from '../icons';
import { useLoad } from '../util';

/** Every tool call waiting for a human, across all runs. */
export function Approvals() {
  const { data, error, reload } = useLoad(() => api.approvals(), []);

  useEffect(() => {
    const t = setInterval(reload, 2000);
    return () => clearInterval(t);
  }, [reload]);

  return (
    <div className="page">
      <PageHeader
        title="Needs your OK"
        subtitle="Things your assistants want to do but asked about first. They wait here, even if the server restarts."
      />
      <ErrorNote error={error} />
      {data && data.length === 0 && <Empty title="Nothing waiting">When an assistant asks before acting, the question shows up here and in the chat.</Empty>}
      <div className="approval-list">
        {data?.map((p) => (
          <ApprovalCard key={`${p.run_id}/${p.call_id}`} p={p} onDone={reload} />
        ))}
      </div>
    </div>
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
    <section className="card approval-card">
      <div className="row between">
        <div>
          <strong title={p.tool}>{toolSentence(p.tool, safeJson(text))}</strong>
          <div className="muted small">
            <a href={`#/activity/${p.run_id}`}>See what led to this</a>
          </div>
        </div>
        <div className="row gap">
          <Button tone="accent" leading={<IconCheck />} loading={busy === 'approve'} onClick={approve}>
            {edited ? 'Allow with my changes' : 'Allow'}
          </Button>
          <Button variant="outline" leading={<IconX />} loading={busy === 'decline'} onClick={decline}>
            Don't allow
          </Button>
        </div>
      </div>
      <label className="label" htmlFor={`in-${p.call_id}`}>
        Details (you can change them) {edited && <span className="chip accent small">changed</span>}
      </label>
      <textarea
        id={`in-${p.call_id}`}
        className="code-editor small-editor"
        spellCheck={false}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
    </section>
  );
}
