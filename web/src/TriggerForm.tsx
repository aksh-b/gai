import { useEffect, useMemo, useRef, useState } from 'react';
import { SelectField, SwitchField, TextField } from '@/components/fields';
import { api } from './api';
import { NumberInput, Section, TemplateVars, TextArea, insertAt, setIn } from './forms';
import { useLoad } from './util';

export type TriggerDraft = {
  name?: string;
  description?: string;
  kind?: 'cron' | 'webhook' | 'file';
  schedule?: string;
  timezone?: string;
  catch_up?: 'once' | 'skip';
  path?: string;
  pattern?: string;
  debounce_ms?: number;
  target?: { agent?: string; workflow?: string };
  input?: string;
  inputs?: Record<string, unknown>;
  auto_approve?: boolean;
  skip_if_running?: boolean;
  enabled?: boolean;
  [k: string]: unknown;
};

const PRESETS = [
  { value: '*/15 * * * *', label: 'Every 15 minutes' },
  { value: '0 * * * *', label: 'Every hour' },
  { value: '0 9 * * *', label: 'Every day at 09:00' },
  { value: '0 9 * * 1-5', label: 'Weekdays at 09:00' },
  { value: '0 9 * * 1', label: 'Mondays at 09:00' },
  { value: '0 9 1 * *', label: 'The 1st of each month at 09:00' },
];

const VARS: Record<string, { name: string; hint: string }[]> = {
  cron: [
    { name: 'date', hint: 'Today (YYYY-MM-DD)' },
    { name: 'now', hint: 'The time it fired' },
    { name: 'trigger', hint: "The trigger's name" },
  ],
  webhook: [
    { name: 'payload', hint: 'The request body (JSON); use payload.field for a field' },
    { name: 'payload | tojson', hint: 'The whole body as JSON text' },
    { name: 'now', hint: 'The time it arrived' },
  ],
  file: [
    { name: 'path', hint: 'The file that changed' },
    { name: 'event', hint: 'created, modified or removed' },
    { name: 'content', hint: 'The text of small text files' },
    { name: 'now', hint: 'The time it changed' },
  ],
};

export const TRIGGER_TEMPLATES: Record<string, TriggerDraft> = {
  cron: { name: '', kind: 'cron', schedule: '0 9 * * 1-5', timezone: browserZone(), input: 'It is {{ date }}. ' },
  webhook: { name: '', kind: 'webhook', input: 'Something happened:\n\n{{ payload | tojson }}' },
  file: { name: '', kind: 'file', path: '/data/inbox', pattern: '*.txt', input: 'The file {{ path }} was {{ event }}.' },
};

function browserZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

function zones(): string[] {
  try {
    return (Intl as unknown as { supportedValuesOf: (k: string) => string[] }).supportedValuesOf('timeZone');
  } catch {
    return ['UTC', browserZone()];
  }
}

/** A trigger as a form, with a live check of the schedule and target. */
export function TriggerForm({ draft, onChange, isNew }: { draft: TriggerDraft; onChange: (d: TriggerDraft) => void; isNew: boolean }) {
  const agents = useLoad(() => api.agents(), []);
  const workflows = useLoad(() => api.workflows(), []);
  const set = (path: (string | number)[], v: unknown) => onChange(setIn(draft, path, v));
  const kind = draft.kind ?? 'cron';
  const targetKind = draft.target?.workflow !== undefined ? 'workflow' : 'agent';
  const targetName = draft.target?.workflow ?? draft.target?.agent ?? '';
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const tzList = useMemo(zones, []);

  // The chosen workflow's inputs, to show a field for each.
  const wf = useLoad(
    () => (targetKind === 'workflow' && targetName ? api.workflow(targetName) : Promise.resolve(null)),
    [targetKind, targetName],
  );

  // Ask the server what this would do, a moment after typing stops.
  const [preview, setPreview] = useState<{ ok: boolean; error?: string | null; schedule_text?: string | null; next: string[] } | null>(null);
  useEffect(() => {
    if (!targetName) {
      setPreview({ ok: false, error: `Choose the ${targetKind} this trigger runs.`, next: [] });
      return;
    }
    const t = setTimeout(() => {
      api
        .previewTrigger({ ...draft, name: draft.name || 'preview' })
        .then(setPreview)
        .catch(() => setPreview(null));
    }, 400);
    return () => clearTimeout(t);
  }, [JSON.stringify(draft)]);

  const preset = PRESETS.find((p) => p.value === draft.schedule)?.value ?? 'custom';
  const switchKind = (k: 'cron' | 'webhook' | 'file') => {
    // Keep what the kinds share; take the rest from that kind's starter.
    const base = TRIGGER_TEMPLATES[k];
    const keep: TriggerDraft = { name: draft.name, description: draft.description, target: draft.target, auto_approve: draft.auto_approve, skip_if_running: draft.skip_if_running, enabled: draft.enabled };
    onChange(Object.fromEntries(Object.entries({ ...base, ...keep, kind: k, input: draft.input ?? base.input }).filter(([, v]) => v !== undefined)) as TriggerDraft);
  };

  return (
    <>
      <Section title="Basics">
        <TextField
          label="Name"
          value={draft.name ?? ''}
          description={isNew ? 'Letters, digits, - and _.' : undefined}
          onChange={(e) => set(['name'], e.target.value)}
        />
        <TextField label="Description" value={draft.description ?? ''} onChange={(e) => set(['description'], e.target.value)} />
      </Section>

      <Section title="When">
        <div className="span-all">
          <div className="segmented" role="group" aria-label="Trigger kind">
            {(
              [
                ['cron', 'On a schedule'],
                ['webhook', 'From a webhook'],
                ['file', 'When files change'],
              ] as const
            ).map(([k, label]) => (
              <button key={k} type="button" aria-pressed={kind === k} disabled={!isNew && kind !== k && k === 'webhook'} onClick={() => switchKind(k)}>
                {label}
              </button>
            ))}
          </div>
        </div>
        {kind === 'cron' && (
          <>
            <SelectField
              label="Repeat"
              options={[...PRESETS, { value: 'custom', label: 'Custom (cron)' }]}
              value={preset}
              onValueChange={(v) => v !== 'custom' && set(['schedule'], v)}
            />
            <TextField
              label="Cron schedule"
              description="minute hour day month weekday; a sixth field in front adds seconds"
              value={draft.schedule ?? ''}
              className="mono"
              onChange={(e) => set(['schedule'], e.target.value)}
            />
            <SelectField
              label="Time zone"
              options={[...new Set(['UTC', browserZone(), ...tzList])].map((z) => ({ value: z, label: z }))}
              value={draft.timezone ?? 'UTC'}
              onValueChange={(v) => set(['timezone'], v === 'UTC' ? undefined : v)}
            />
            <div className="span-all">
              <SwitchField
                size="sm"
                label="Catch up once after downtime"
                description="If gai was not running at a scheduled time, run once when it starts again."
                checked={(draft.catch_up ?? 'once') === 'once'}
                onCheckedChange={(checked) => set(['catch_up'], checked ? undefined : 'skip')}
              />
            </div>
          </>
        )}
        {kind === 'webhook' && (
          <p className="muted span-all">
            {isNew
              ? 'After saving you get a URL and a secret. Anything that can POST to a URL can start this trigger.'
              : 'The URL and secret are below.'}{' '}
            The request body becomes <span className="mono">payload</span>.
          </p>
        )}
        {kind === 'file' && (
          <>
            <TextField label="Folder or file" description="A path gai can see (inside its container on this machine)" value={draft.path ?? ''} onChange={(e) => set(['path'], e.target.value)} />
            <TextField label="Only names like" placeholder="*.pdf" value={draft.pattern ?? ''} onChange={(e) => set(['pattern'], e.target.value)} />
            <NumberInput label="Wait for quiet (ms)" description="Fire once changes settle" min={50} value={draft.debounce_ms} onChange={(v) => set(['debounce_ms'], v)} />
          </>
        )}
      </Section>

      <Section title="What runs">
        <div className="span-all">
          <div className="segmented" role="group" aria-label="Target kind">
            {(['agent', 'workflow'] as const).map((k) => (
              <button key={k} type="button" aria-pressed={targetKind === k} onClick={() => set(['target'], { [k]: '' })}>
                {k === 'agent' ? 'An agent' : 'A workflow'}
              </button>
            ))}
          </div>
        </div>
        <SelectField
          label={targetKind === 'agent' ? 'Agent' : 'Workflow'}
          placeholder="Choose…"
          options={(targetKind === 'agent'
            ? (agents.data ?? []).map((a) => ({ value: a.summary.name, label: a.summary.name, description: a.spec?.description ?? '' }))
            : (workflows.data ?? []).map((w) => ({ value: w.name, label: w.name, description: w.description ?? '' }))
          ).concat(targetName && ![...(agents.data ?? []).map((a) => a.summary.name), ...(workflows.data ?? []).map((w) => w.name)].includes(targetName) ? [{ value: targetName, label: targetName, description: 'not found' }] : [])}
          value={targetName || null}
          onValueChange={(v) => set(['target'], { [targetKind]: v })}
        />
        {targetKind === 'agent' ? (
          <>
            <TextArea
              label="Task for the agent"
              rows={5}
              value={draft.input ?? ''}
              placeholder="What the agent should do each time"
              onChange={(v) => set(['input'], v)}
              textareaRef={inputRef}
            />
            <TemplateVars vars={VARS[kind]} onInsert={(t) => set(['input'], insertAt(inputRef.current, draft.input ?? '', t))} />
          </>
        ) : (
          <div className="span-all stack tight">
            {wf.data && Object.keys(wf.data.spec.inputs ?? {}).length === 0 && <p className="muted small">This workflow takes no inputs.</p>}
            {Object.entries(wf.data?.spec.inputs ?? {}).map(([k, i]) => (
              <TextField
                key={k}
                label={k}
                description={`${(i as { description?: string }).description ?? ''} Templates work, e.g. {{ ${VARS[kind][0].name} }}. ${(i as { default?: unknown }).default !== undefined ? 'Leave empty for the default.' : 'Required.'}`}
                value={typeof draft.inputs?.[k] === 'string' ? (draft.inputs[k] as string) : draft.inputs?.[k] === undefined ? '' : JSON.stringify(draft.inputs[k])}
                onChange={(e) => set(['inputs', k], e.target.value)}
              />
            ))}
          </div>
        )}
      </Section>

      <Section title="Options">
        <SwitchField size="sm" label="Turned on" checked={draft.enabled !== false} onCheckedChange={(checked) => set(['enabled'], checked ? undefined : false)} />
        <SwitchField
          size="sm"
          label="Skip while the last run is going"
          checked={draft.skip_if_running !== false}
          onCheckedChange={(checked) => set(['skip_if_running'], checked ? undefined : false)}
        />
        <SwitchField
          size="sm"
          label="Approve tool calls automatically"
          description="Otherwise calls that need approval wait in the Approvals inbox."
          checked={!!draft.auto_approve}
          onCheckedChange={(checked) => set(['auto_approve'], checked || undefined)}
        />
      </Section>

      {preview && (
        <div className="preview-box" data-ok={preview.ok || undefined}>
          {preview.ok ? (
            <>
              <strong className="small">{preview.schedule_text ?? (kind === 'webhook' ? 'Fires when the webhook is called.' : kind === 'file' ? 'Fires when matching files change.' : 'Looks good.')}</strong>
              {preview.next.length > 0 && (
                <ul className="small muted">
                  {preview.next.slice(0, 3).map((t) => (
                    <li key={t}>{new Date(t).toLocaleString()}</li>
                  ))}
                </ul>
              )}
            </>
          ) : (
            <span className="small error-text">{preview.error}</span>
          )}
        </div>
      )}
    </>
  );
}
