// A workflow as a list of steps, for people who'd rather not write YAML.
// Covers the common shapes (assistant, tool, a person's OK, text); anything
// else is kept as written and edited in YAML.

import { useRef } from 'react';
import { Button } from '@/components/ui/button';
import { SelectField, TextField } from '@/components/fields';
import { api } from './api';
import { Disclosure } from './components';
import { IconPlus, IconTrash } from './icons';
import { NumberInput, Section, TextArea, insertAt } from './forms';
import { toolLabel } from './labels';
import { useDevMode } from './prefs';
import { useLoad } from './util';

/* eslint-disable @typescript-eslint/no-explicit-any */
type Node = Record<string, any>;
type Spec = Record<string, any> & { nodes?: Node[]; inputs?: Record<string, { description?: string; default?: unknown }> };

const KINDS = [
  { value: 'agent', label: 'Ask an assistant', description: 'An assistant does a task and answers' },
  { value: 'tool', label: 'Use a tool', description: 'Read a page, calculate, write a file…' },
  { value: 'human', label: 'Ask a person', description: 'Wait for someone to OK it (and maybe reply)' },
  { value: 'template', label: 'Put text together', description: 'Combine earlier results into text' },
];

const INLINE = '__inline';

export function nodeKind(n: Node): string | null {
  const kinds = ['agent', 'tool', 'human', 'template', 'value'].filter((k) => n[k] !== undefined);
  return kinds.length === 1 ? kinds[0] : null;
}

/** Whether every step is one the form can show. */
export function formCanEdit(spec: Spec): boolean {
  return (spec.nodes ?? []).every((n) => {
    const k = nodeKind(n);
    return k !== null && k !== 'value' && (k !== 'tool' || n.args === undefined || (typeof n.args === 'object' && !Array.isArray(n.args)));
  });
}

const freshId = (nodes: Node[]) => {
  let i = nodes.length + 1;
  while (nodes.some((n) => n.id === `step${i}`)) i++;
  return `step${i}`;
};

export function WorkflowForm({ spec, onChange, isNew }: { spec: Spec; onChange: (s: Spec) => void; isNew: boolean }) {
  const [dev] = useDevMode();
  const agents = useLoad(() => api.agents(), []);
  const tools = useLoad(() => api.tools(), []);
  const nodes = spec.nodes ?? [];
  const inputs = Object.entries(spec.inputs ?? {});

  const set = (patch: Partial<Spec>) => onChange({ ...spec, ...patch });
  const setNode = (i: number, n: Node | null) => {
    const next = [...nodes];
    if (n) next[i] = n;
    else next.splice(i, 1);
    set({ nodes: next });
  };
  const move = (i: number, by: number) => {
    const j = i + by;
    if (j < 0 || j >= nodes.length) return;
    const next = [...nodes];
    [next[i], next[j]] = [next[j], next[i]];
    set({ nodes: next });
  };
  const addStep = (kind: string) => {
    const id = freshId(nodes);
    const prev = nodes[nodes.length - 1];
    const ref = prev ? `{{ nodes.${prev.id}.output }}` : inputs[0] ? `{{ inputs.${inputs[0][0]} }}` : '';
    const n: Node =
      kind === 'agent'
        ? { id, agent: agents.data?.[0]?.summary.name ?? { name: id }, input: ref }
        : kind === 'tool'
          ? { id, tool: 'http_fetch', args: { url: '' } }
          : kind === 'human'
            ? { id, human: `Does this look right?\n\n${ref}` }
            : { id, template: ref };
    set({ nodes: [...nodes, n] });
  };

  const renameInput = (from: string, to: string) => {
    const clean = to.replace(/[^A-Za-z0-9_]/g, '_');
    const next: Spec['inputs'] = {};
    for (const [k, v] of inputs) next[k === from ? clean : k] = v;
    set({ inputs: next });
  };
  const setInput = (name: string, v: { description?: string; default?: unknown } | null) => {
    const next = { ...(spec.inputs ?? {}) };
    if (v) next[name] = v;
    else delete next[name];
    set({ inputs: next });
  };

  /** Chips that insert a reference to an input or an earlier step's result. */
  const refs = (upTo: number) => [
    ...inputs.map(([k]) => ({ label: k, text: `{{ inputs.${k} }}`, hint: 'What you type when you run it' })),
    ...nodes.slice(0, upTo).map((n) => ({ label: `${n.id}'s result`, text: `{{ nodes.${n.id}.output }}`, hint: `The output of step “${n.id}”` })),
  ];

  const outputStep = typeof spec.output === 'string' ? /^\{\{\s*nodes\.(\w+)\.output\s*\}\}$/.exec(spec.output)?.[1] : undefined;

  return (
    <>
      <Section title="Basics">
        <TextField
          label="Name"
          value={spec.name ?? ''}
          disabled={!isNew}
          description={isNew ? 'Letters, digits, - and _.' : 'Changes are kept as versions.'}
          onChange={(e) => set({ name: e.target.value })}
        />
        <TextField label="Description" value={spec.description ?? ''} onChange={(e) => set({ description: e.target.value || undefined })} />
      </Section>

      <Section
        title="What you fill in when you run it"
        hint="Optional. Steps can use these, e.g. a topic or a web address."
        aside={
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              let n = inputs.length ? `input${inputs.length + 1}` : 'topic';
              while (spec.inputs?.[n]) n += '_';
              setInput(n, { description: '' });
            }}
          >
            <IconPlus data-icon="inline-start" />
            Add
          </Button>
        }
      >
        {inputs.map(([k, v]) => (
          <div key={k} className="span-all row gap wrap">
            <input className="plain-input" aria-label="Input name" value={k} onChange={(e) => renameInput(k, e.target.value)} />
            <input
              className="plain-input"
              style={{ flex: 2 }}
              aria-label="What it is"
              placeholder="What it is (shown when running)"
              value={v?.description ?? ''}
              onChange={(e) => setInput(k, { ...v, description: e.target.value || undefined })}
            />
            <input
              className="plain-input"
              aria-label="Default value"
              placeholder="Default (optional)"
              value={v?.default === undefined ? '' : typeof v.default === 'string' ? v.default : JSON.stringify(v.default)}
              onChange={(e) => setInput(k, { ...v, default: e.target.value === '' ? undefined : e.target.value })}
            />
            <button type="button" className="icon-btn" aria-label={`Remove ${k}`} onClick={() => setInput(k, null)}>
              <IconTrash />
            </button>
          </div>
        ))}
        {inputs.length === 0 && <p className="muted small span-all">None.</p>}
      </Section>

      <fieldset className="form-section">
        <legend>Steps</legend>
        <p className="muted small form-hint">They run top to bottom. A step that uses an earlier result waits for it; the others run at the same time.</p>
        <div className="steps">
          {nodes.map((n, i) => (
            <Step
              key={i}
              index={i}
              node={n}
              count={nodes.length}
              agents={(agents.data ?? []).map((a) => ({ name: a.summary.name, description: a.spec?.description ?? '' }))}
              tools={(tools.data ?? []).map((t) => ({ name: t.spec.name, description: t.spec.description, schema: t.spec.input_schema as any }))}
              refs={refs(i)}
              dev={dev}
              onChange={(x) => setNode(i, x)}
              onMove={(by) => move(i, by)}
            />
          ))}
        </div>
        <div className="row gap wrap add-step">
          <span className="muted small">Add a step:</span>
          {KINDS.map((k) => (
            <Button variant="outline" size="sm" key={k.value} onClick={() => addStep(k.value)}>
              <IconPlus data-icon="inline-start" />
              {k.label}
            </Button>
          ))}
        </div>
      </fieldset>

      <Section title="Result">
        <SelectField
          label="The workflow's result is"
          options={[
            { value: '', label: 'The last step that ran' },
            ...nodes.map((n) => ({ value: n.id, label: `${n.id}'s result` })),
            ...(spec.output !== undefined && !outputStep ? [{ value: '__custom', label: 'Custom (set in YAML)' }] : []),
          ]}
          value={outputStep ?? (spec.output !== undefined ? '__custom' : '')}
          onValueChange={(v) => {
            if (v === '__custom') return;
            set({ output: v ? `{{ nodes.${v}.output }}` : undefined });
          }}
        />
      </Section>
    </>
  );
}

function Step({
  index,
  node,
  count,
  agents,
  tools,
  refs,
  dev,
  onChange,
  onMove,
}: {
  index: number;
  node: Node;
  count: number;
  agents: { name: string; description: string }[];
  tools: { name: string; description: string; schema?: { properties?: Record<string, { description?: string; type?: string }>; required?: string[] } }[];
  refs: { label: string; text: string; hint: string }[];
  dev: boolean;
  onChange: (n: Node | null) => void;
  onMove: (by: number) => void;
}) {
  const kind = nodeKind(node) ?? 'agent';
  const set = (patch: Node) => {
    const next = { ...node, ...patch };
    for (const k of Object.keys(next)) if (next[k] === undefined) delete next[k];
    onChange(next);
  };
  const switchKind = (k: string) => {
    const { agent: _a, tool: _t, human: _h, template: _p, input: _i, args: _g, ...rest } = node;
    void _a, _t, _h, _p, _i, _g;
    const fresh: Node = k === 'agent' ? { agent: agents[0]?.name ?? { name: node.id }, input: '' } : k === 'tool' ? { tool: 'http_fetch', args: { url: '' } } : k === 'human' ? { human: '' } : { template: '' };
    onChange({ ...rest, ...fresh });
  };

  const inline = kind === 'agent' && typeof node.agent === 'object';
  const tool = tools.find((t) => t.name === node.tool);
  const props = Object.entries(tool?.schema?.properties ?? {});

  return (
    <div className="step-card">
      <div className="step-head">
        <span className="step-num">{index + 1}</span>
        <input
          className="plain-input step-id"
          aria-label="Step name"
          title="A short name; later steps use it to refer to this one"
          value={node.id ?? ''}
          onChange={(e) => set({ id: e.target.value.replace(/[^A-Za-z0-9_]/g, '_') })}
        />
        <div className="step-kind">
          <SelectField aria-label="Kind of step" options={KINDS} value={kind} onValueChange={switchKind} />
        </div>
        <div className="spacer" />
        <button type="button" className="icon-btn" aria-label="Move up" disabled={index === 0} onClick={() => onMove(-1)}>
          ↑
        </button>
        <button type="button" className="icon-btn" aria-label="Move down" disabled={index === count - 1} onClick={() => onMove(1)}>
          ↓
        </button>
        <button type="button" className="icon-btn" aria-label="Remove step" onClick={() => onChange(null)}>
          <IconTrash />
        </button>
      </div>

      <div className="form-grid">
        {kind === 'agent' && (
          <>
            <SelectField
              label="Assistant"
              options={[
                ...agents.map((a) => ({ value: a.name, label: a.name, description: a.description })),
                ...(typeof node.agent === 'string' && !agents.some((a) => a.name === node.agent)
                  ? [{ value: node.agent, label: node.agent, description: 'not found among your assistants' }]
                  : []),
                { value: INLINE, label: 'A one-off helper', description: 'Uses the default model, with the instructions below' },
              ]}
              value={inline ? INLINE : (node.agent as string)}
              onValueChange={(v) => set({ agent: v === INLINE ? { name: node.id || 'helper' } : v })}
            />
            {inline && (
              <RefText
                label="Helper's instructions"
                rows={2}
                value={node.agent.system ?? ''}
                placeholder="You are a careful editor…"
                refs={[]}
                onChange={(v) => set({ agent: { ...node.agent, system: v || undefined } })}
              />
            )}
            <RefText label="Task" value={node.input ?? ''} placeholder="What should it do?" refs={refs} onChange={(v) => set({ input: v || undefined })} />
          </>
        )}

        {kind === 'tool' && (
          <>
            <SelectField
              label="Tool"
              options={tools.map((t) => ({ value: t.name, label: dev ? t.name : toolLabel(t.name), description: t.description }))}
              value={node.tool}
              onValueChange={(v) => set({ tool: v, args: {} })}
            />
            {props.map(([k, p]) => (
              <RefText
                key={k}
                label={`${k}${tool?.schema?.required?.includes(k) ? '' : ' (optional)'}`}
                rows={1}
                value={node.args?.[k] === undefined ? '' : typeof node.args[k] === 'string' ? node.args[k] : JSON.stringify(node.args[k])}
                placeholder={p.description}
                refs={refs}
                onChange={(v) => {
                  const args = { ...(node.args ?? {}) };
                  if (v === '') delete args[k];
                  else args[k] = v;
                  set({ args });
                }}
              />
            ))}
          </>
        )}

        {kind === 'human' && (
          <RefText label="What to ask" value={node.human ?? ''} placeholder="Publish this?" refs={refs} onChange={(v) => set({ human: v })} />
        )}

        {kind === 'template' && (
          <RefText label="Text" value={node.template ?? ''} placeholder="# Report\n\n…" refs={refs} onChange={(v) => set({ template: v })} />
        )}
      </div>

      <Disclosure summary={<span className="muted small">Options: only if…, repeat for each…, retries</span>} open={!!(node.if || node.map || node.retries)}>
        <div className="form-grid">
          <TextField
            label="Only run if"
            description="An expression, e.g. nodes.check.output.approved"
            value={node.if ?? ''}
            onChange={(e) => set({ if: e.target.value || undefined })}
          />
          <TextField
            label="Repeat for each item of"
            description="A list, e.g. inputs.names; use {{ item }} in the step"
            value={node.map ?? ''}
            onChange={(e) => set({ map: e.target.value || undefined })}
          />
          <NumberInput label="Retries if it fails" min={0} value={node.retries} onChange={(v) => set({ retries: v })} />
        </div>
      </Disclosure>
    </div>
  );
}

/** A text area with buttons that insert references to inputs and earlier results. */
function RefText({
  label,
  value,
  onChange,
  refs,
  placeholder,
  rows = 3,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  refs: { label: string; text: string; hint: string }[];
  placeholder?: string;
  rows?: number;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  return (
    <>
      <TextArea label={label} rows={rows} value={value} placeholder={placeholder} onChange={onChange} textareaRef={ref} />
      {refs.length > 0 && (
        <div className="template-vars span-all">
          <span className="muted small">Use:</span>
          {refs.map((r) => (
            <button key={r.text} type="button" className="chip small var-chip" title={r.hint} onClick={() => onChange(insertAt(ref.current, value, r.text))}>
              {r.label}
            </button>
          ))}
        </div>
      )}
    </>
  );
}
