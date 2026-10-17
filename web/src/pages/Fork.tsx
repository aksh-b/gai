import { useEffect, useRef, useState } from 'react';
import { Button, Select, toast } from '@flame';
import {
  api,
  type AgentVersion,
  type CatalogEntry,
  type ContentBlock,
  type ForkInfo,
  type ReplayOverrides,
  type Run,
  type Step,
} from '../api';
import { ErrorNote } from '../components';
import { navigate } from '../router';
import { blocksText } from '../util';

/** One model step of a run with the tool calls it made and their results. */
export interface Turn {
  step: number;
  model: string | null;
  content: ContentBlock[];
  /** Taken from the run this one was forked from. */
  replayed: boolean;
  failed: boolean;
  tools: { id: string; name: string; input: unknown; output?: string; error?: boolean }[];
}

/** Group a run's journal into model turns. */
export function turns(steps: Step[]): Turn[] {
  const out: Turn[] = [];
  const results = new Map<string, { text: string; error: boolean }>();
  for (const s of steps) {
    if (s.kind === 'tool') {
      const input = s.input as { call_id?: string } | null;
      const output = s.output as { content?: ContentBlock[]; is_error?: boolean } | null;
      if (input?.call_id && output) results.set(input.call_id, { text: blocksText(output.content), error: !!output.is_error });
    }
  }
  for (const s of steps) {
    if (s.kind !== 'model') continue;
    const input = s.input as { step?: number; replayed_from?: string } | null;
    const output = s.output as { content?: ContentBlock[] } | string | null;
    const content = typeof output === 'object' && output?.content ? output.content : [];
    const step = input?.step ?? out.length + 1;
    const turn: Turn = {
      step,
      model: s.name ?? null,
      content,
      replayed: !!input?.replayed_from,
      failed: s.status === 'failed',
      tools: content.flatMap((b) =>
        b.type === 'tool_use'
          ? [{ id: b.id, name: b.name, input: b.input, output: results.get(b.id)?.text, error: results.get(b.id)?.error }]
          : [],
      ),
    };
    // A retried step keeps its number; the later attempt replaces it.
    const i = out.findIndex((t) => t.step === step);
    if (i >= 0) out[i] = turn;
    else out.push(turn);
  }
  return out;
}

export function forkOf(run: Run): ForkInfo | undefined {
  return run.metadata.fork as ForkInfo | undefined;
}

/**
 * Fork a run: steps before `from` replay from the recording without calling
 * the model; the rest run live with the chosen changes.
 */
export function ForkDialog({ run, steps, from, onClose }: { run: Run; steps: Step[]; from: number | null; onClose: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  const ts = turns(steps);
  const max = (ts.at(-1)?.step ?? 0) + 1;
  const spec = run.metadata.spec;
  const [fromStep, setFromStep] = useState(1);
  const [model, setModel] = useState('');
  const [version, setVersion] = useState('');
  const [tools, setTools] = useState('');
  const [system, setSystem] = useState('');
  const [editStep, setEditStep] = useState('');
  const [editText, setEditText] = useState('');
  const [models, setModels] = useState<CatalogEntry[]>([]);
  const [versions, setVersions] = useState<AgentVersion[]>([]);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (from === null) {
      ref.current?.close();
      return;
    }
    setFromStep(Math.min(Math.max(1, from), max));
    setModel('');
    setVersion('');
    setTools((spec?.tools ?? []).join(', '));
    setSystem(spec?.system ?? '');
    setEditStep('');
    setEditText('');
    setError(undefined);
    api.models().then(setModels).catch(() => {});
    api.agentVersions(run.agent).then(setVersions).catch(() => setVersions([]));
    ref.current?.showModal();
  }, [from]);

  const replayedTurns = ts.filter((t) => t.step < fromStep);
  useEffect(() => {
    const t = ts.find((x) => String(x.step) === editStep);
    setEditText(t ? blocksText(t.content) : '');
  }, [editStep]);

  async function submit() {
    const o: ReplayOverrides = {};
    if (model) o.model = model;
    if (version) o.agent_version = Number(version);
    const toolList = tools
      .split(',')
      .map((t) => t.trim())
      .filter(Boolean);
    const original = spec?.tools ?? [];
    if (toolList.join(',') !== original.join(',')) o.tools = toolList.includes('none') ? [] : toolList;
    if (system !== (spec?.system ?? '')) o.system_prompt = system;
    const edited = ts.find((x) => String(x.step) === editStep);
    if (edited && edited.step < fromStep && editText !== blocksText(edited.content)) o.step_edits = [{ step: edited.step, text: editText }];
    setBusy(true);
    try {
      const r = await api.replay(run.id, fromStep, o);
      toast.success(`Forked into ${r.run_id}`);
      onClose();
      navigate(`/runs/${run.id}/compare/${r.run_id}`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  const stepOptions = Array.from({ length: max }, (_, i) => i + 1).map((n) => ({
    value: String(n),
    label:
      n === 1
        ? '1 · re-run everything live'
        : n === max
          ? `${n} · replay ${n === 2 ? 'step 1' : `all ${n - 1} steps`}, then continue`
          : `${n} · replay ${n === 2 ? 'step 1' : `steps 1–${n - 1}`}`,
  }));
  const modelOptions = [
    { value: '', label: `Same model (${run.metadata.model ?? 'original'})` },
    ...models.map((m) => ({ value: m.id, label: m.id })),
  ];
  const versionOptions = [
    { value: '', label: `As recorded${run.agent_version ? ` (v${run.agent_version})` : ''}` },
    ...versions.map((v) => ({ value: String(v.version), label: `v${v.version}${v.note ? ` · ${v.note}` : ''}` })),
  ];

  return (
    <dialog ref={ref} className="key-dialog fork-dialog" aria-labelledby="fork-title" onClose={onClose}>
      <form
        method="dialog"
        className="stack"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <h2 id="fork-title">Fork this run</h2>
        <p className="muted">
          Steps before the one you pick are copied from this run without calling the model or any tool. From there on the fork runs live,
          with your changes, in a copy of the conversation.
        </p>
        <Select label="Run live from step" options={stepOptions} value={String(fromStep)} onValueChange={(v) => setFromStep(Number(v))} />
        <Select label="Model" options={modelOptions} value={model} onValueChange={setModel} />
        {versions.length > 0 && <Select label="Agent version" options={versionOptions} value={version} onValueChange={setVersion} />}
        <label className="stack tight">
          <span className="label">Tools (comma-separated; none for no tools)</span>
          <input className="plain-input" value={tools} onChange={(e) => setTools(e.target.value)} />
        </label>
        <label className="stack tight">
          <span className="label">System prompt</span>
          <textarea className="code-editor short" spellCheck={false} value={system} onChange={(e) => setSystem(e.target.value)} />
        </label>
        {replayedTurns.length > 0 && (
          <div className="stack tight">
            <Select
              label="Change what the model said at a replayed step (optional)"
              options={[{ value: '', label: 'No change' }, ...replayedTurns.map((t) => ({ value: String(t.step), label: `Step ${t.step}` }))]}
              value={editStep}
              onValueChange={setEditStep}
            />
            {editStep && (
              <textarea
                className="code-editor short"
                aria-label="Reply text"
                value={editText}
                onChange={(e) => setEditText(e.target.value)}
                placeholder="The reply's text (tool calls are kept)"
              />
            )}
          </div>
        )}
        <ErrorNote error={error} />
        <div className="row gap end">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" tone="accent" loading={busy}>
            Fork and run
          </Button>
        </div>
      </form>
    </dialog>
  );
}
