import { useMemo, useRef, useState } from 'react';
import { Button, Select, Switch, TextField } from '@flame';
import { api, type AgentSpec } from './api';
import { Disclosure } from './components';
import { IconPlus, IconTrash } from './icons';
import { NumberInput, PickList, Section, TemplateVars, TextArea, getIn, insertAt, setIn, type PickItem } from './forms';
import { useLoad } from './util';

type Spec = AgentSpec & Record<string, unknown>;

interface BlockSpec {
  label: string;
  scope?: string;
  description?: string;
  value?: string;
  limit?: number;
  read_only?: boolean;
}

interface RemoteDelegate {
  a2a: string;
  name: string;
  description?: string;
}

const PROMPT_VARS = [
  { name: 'agent', hint: "The agent's name" },
  { name: 'date', hint: "Today's date" },
  { name: 'datetime', hint: 'The current date and time' },
  { name: 'model', hint: 'The model the run uses' },
  { name: 'workspace', hint: "The run's working folder" },
];

/** Tools the runtime builds per agent rather than keeping in the registry. */
const AGENT_TOOLS: PickItem[] = [
  { value: 'rag_search', description: 'Search all knowledge collections (picking collections below limits it to those).', group: 'Built for each agent' },
  { value: 'image_gen', description: 'Generate images with the media image model.', group: 'Built for each agent' },
  { value: 'speak', description: 'Turn text into speech with the media speech model.', group: 'Built for each agent' },
  { value: 'transcribe', description: 'Turn speech into text with the media transcription model.', group: 'Built for each agent' },
];

const EFFORTS = ['off', 'low', 'medium', 'high', 'xhigh', 'max'];

/** Every part of an agent spec as a form. */
export function AgentForm({ spec, onChange, isNew }: { spec: Spec; onChange: (s: Spec) => void; isNew: boolean }) {
  const models = useLoad(() => api.models(), []);
  const tools = useLoad(() => api.tools(), []);
  const agents = useLoad(() => api.agents(), []);
  const collections = useLoad(() => api.collections(), []);
  const skills = useLoad(() => api.skills(), []);
  const health = useLoad(() => api.health(), []);
  const prompt = useRef<HTMLTextAreaElement>(null);
  const [typing, setTyping] = useState(false);

  const set = (path: (string | number)[], v: unknown) => onChange(setIn(spec, path, v));
  const get = <T,>(path: (string | number)[]) => getIn(spec, path) as T | undefined;

  const chatModels = (models.data ?? []).filter((m) => m.kind === 'chat');
  const modelIds = chatModels.map((m) => m.id);
  const model = spec.model ?? '';
  const modelOptions = [
    { value: '', label: `Server default (${health.data?.default_model ?? 'none'})` },
    ...chatModels.map((m) => ({
      value: m.id,
      label: m.id,
      description: [m.key.status === 'missing' ? `needs ${m.key.env}` : '', m.capabilities.tools ? '' : 'no tools'].filter(Boolean).join(' · '),
    })),
    ...(model && !modelIds.includes(model) ? [{ value: model, label: model, description: 'not in the catalog' }] : []),
    { value: '__other', label: 'Other address…', description: 'Any provider/model, an alias, or router/<name>' },
  ];

  const toolItems: PickItem[] = useMemo(() => {
    const out: PickItem[] = [];
    const servers = new Set<string>();
    for (const t of tools.data ?? []) {
      const m = t.spec.name.match(/^mcp__(.+?)__/);
      const group = m ? `MCP: ${m[1]}` : t.spec.name.startsWith('a2a__') ? 'Remote agents (A2A)' : 'Built in';
      if (m) servers.add(m[1]);
      out.push({ value: t.spec.name, description: t.spec.description, group });
    }
    for (const s of servers) {
      out.unshift({ value: `mcp__${s}__*`, label: `all ${s} tools`, description: 'Every tool this MCP server has, now and later.', group: `MCP: ${s}` });
    }
    return [...out, ...AGENT_TOOLS];
  }, [tools.data]);
  const defaultPolicy = (name: string) => tools.data?.find((t) => t.spec.name === name)?.policy ?? 'auto';

  const delegates = (spec.delegates as unknown[] | undefined) ?? [];
  const localDelegates = delegates.filter((d): d is string => typeof d === 'string');
  const remoteDelegates = delegates.filter((d): d is RemoteDelegate => typeof d === 'object' && d !== null && 'a2a' in d);
  const otherDelegates = delegates.filter((d) => typeof d === 'object' && d !== null && !('a2a' in d));
  const setDelegates = (local: string[], remote: RemoteDelegate[]) => set(['delegates'], [...local, ...otherDelegates, ...remote]);

  const blocks = (get<BlockSpec[]>(['memory', 'blocks']) ?? []) as BlockSpec[];
  const setBlock = (i: number, b: BlockSpec | null) => {
    const next = [...blocks];
    if (b) next[i] = b;
    else next.splice(i, 1);
    set(['memory', 'blocks'], next);
  };

  const usesMedia = (spec.tools ?? []).some((t) => ['image_gen', 'speak', 'transcribe'].includes(t));
  const mediaModels = (kind: string) => (models.data ?? []).filter((m) => m.kind === kind).map((m) => ({ value: m.id, label: m.id }));
  const effort = get<string>(['reasoning', 'effort']) ?? '';

  return (
    <>
      <Section title="Basics">
        <TextField
          label="Name"
          value={spec.name ?? ''}
          disabled={!isNew}
          description={isNew ? 'Letters, digits, - and _. Cannot be changed later.' : 'Saving makes a new version of this agent.'}
          onChange={(e) => set(['name'], e.target.value)}
        />
        <TextField label="Description" value={spec.description ?? ''} description="Shown to people, and to agents that delegate to this one." onChange={(e) => set(['description'], e.target.value)} />
        <div>
          <Select
            label="Model"
            options={modelOptions}
            value={typing ? '__other' : model}
            onValueChange={(v) => {
              setTyping(v === '__other');
              if (v !== '__other') set(['model'], v);
            }}
          />
        </div>
        {typing && (
          <TextField
            label="Model address"
            placeholder="ollama/qwen3:4b, router/auto"
            value={model}
            autoFocus
            onChange={(e) => set(['model'], e.target.value)}
          />
        )}
      </Section>

      <Section title="Instructions" hint="The system prompt. It is a template: the variables below are filled in when a run starts.">
        <TextArea
          label="System prompt"
          rows={9}
          value={spec.system ?? ''}
          placeholder="You are a careful researcher. Today is {{ date }}…"
          onChange={(v) => set(['system'], v)}
          textareaRef={prompt}
        />
        <TemplateVars vars={PROMPT_VARS} onInsert={(t) => set(['system'], insertAt(prompt.current, spec.system ?? '', t))} />
      </Section>

      <Section title="Tools" hint="What the agent may call. For each chosen tool, pick whether it runs on its own, needs a person's approval, or is refused.">
        <PickList
          items={toolItems}
          selected={spec.tools ?? []}
          onChange={(v) => {
            let next = setIn(spec, ['tools'], v);
            // Drop policies of tools no longer chosen.
            const pol = { ...(spec.tool_policy ?? {}) };
            for (const k of Object.keys(pol)) if (!v.includes(k)) delete pol[k];
            next = setIn(next, ['tool_policy'], pol);
            onChange(next);
          }}
          extra={(name) =>
            name.endsWith('*') ? null : (
              <select
                className="plain-input policy-select"
                aria-label={`Policy for ${name}`}
                value={spec.tool_policy?.[name] ?? defaultPolicy(name)}
                onChange={(e) => {
                  const v = e.target.value;
                  set(['tool_policy', name], v === defaultPolicy(name) ? undefined : v);
                }}
              >
                <option value="auto">runs on its own</option>
                <option value="approve">needs approval</option>
                <option value="deny">refused</option>
              </select>
            )
          }
        />
      </Section>

      <Section title="Delegates" hint="Other agents this one can hand tasks to. Each becomes a tool named ask_<name>.">
        <PickList
          items={(agents.data ?? [])
            .filter((a) => a.summary.name !== spec.name)
            .map((a) => ({ value: a.summary.name, description: a.spec?.description ?? a.spec?.model ?? '' }))}
          selected={localDelegates}
          onChange={(v) => setDelegates(v, remoteDelegates)}
          empty="No other stored agents yet."
          maxHeight={200}
        />
        {otherDelegates.length > 0 && <p className="muted small span-all">{otherDelegates.length} more delegate(s) with custom names; edit them in YAML.</p>}
        <div className="span-all stack tight">
          <span className="label">Remote agents (A2A)</span>
          {remoteDelegates.map((r, i) => (
            <div key={i} className="row gap">
              <input className="plain-input" placeholder="name" aria-label="Remote agent name" value={r.name} onChange={(e) => setDelegates(localDelegates, remoteDelegates.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))} />
              <input className="plain-input" style={{ flex: 3 }} placeholder="https://host/a2a/agent or its agent card URL" aria-label="Remote agent URL" value={r.a2a} onChange={(e) => setDelegates(localDelegates, remoteDelegates.map((x, j) => (j === i ? { ...x, a2a: e.target.value } : x)))} />
              <button type="button" className="icon-btn" aria-label="Remove remote agent" onClick={() => setDelegates(localDelegates, remoteDelegates.filter((_, j) => j !== i))}>
                <IconTrash />
              </button>
            </div>
          ))}
          <div>
            <Button size="sm" variant="ghost" leading={<IconPlus />} onClick={() => setDelegates(localDelegates, [...remoteDelegates, { a2a: '', name: '' }])}>
              Add a remote agent
            </Button>
          </div>
        </div>
      </Section>

      <Section title="Knowledge and skills">
        <div className="span-all stack tight">
          <span className="label">Knowledge collections (searched with rag_search)</span>
          <PickList
            items={(collections.data ?? []).map((c) => ({ value: c.name, description: c.description ?? `${c.documents} documents` }))}
            selected={(spec.knowledge as string[] | undefined) ?? []}
            onChange={(v) => set(['knowledge'], v)}
            empty="No collections yet: add some on the Knowledge page."
            maxHeight={180}
          />
        </div>
        <div className="span-all stack tight">
          <span className="label">Skills (loaded on demand)</span>
          <PickList
            items={(skills.data ?? []).map((s) => ({ value: s.name, description: s.description }))}
            selected={(spec.skills as string[] | undefined) ?? []}
            onChange={(v) => set(['skills'], v)}
            empty="No skills yet: add some on the Skills page."
            maxHeight={180}
          />
        </div>
      </Section>

      <Section
        title="Memory"
        hint="Blocks the agent always sees and can edit as it learns (memory_append, memory_replace, memory_rethink)."
        aside={
          <Button size="sm" variant="ghost" leading={<IconPlus />} onClick={() => set(['memory', 'blocks'], [...blocks, { label: blocks.length ? `notes${blocks.length + 1}` : 'notes', limit: 2000 }])}>
            Add block
          </Button>
        }
      >
        {blocks.map((b, i) => (
          <div key={i} className="span-all block-row">
            <div className="row gap wrap">
              <input className="plain-input" aria-label="Block label" placeholder="label" value={b.label} onChange={(e) => setBlock(i, { ...b, label: e.target.value })} />
              <select className="plain-input" aria-label="Block scope" value={b.scope?.startsWith('shared:') ? 'shared' : (b.scope ?? 'agent')} onChange={(e) => setBlock(i, { ...b, scope: e.target.value === 'agent' ? undefined : e.target.value === 'shared' ? 'shared:team' : 'user' })}>
                <option value="agent">this agent</option>
                <option value="user">the user (shared by all agents)</option>
                <option value="shared">shared group</option>
              </select>
              {b.scope?.startsWith('shared:') && (
                <input className="plain-input" aria-label="Shared group name" value={b.scope.slice(7)} onChange={(e) => setBlock(i, { ...b, scope: `shared:${e.target.value}` })} />
              )}
              <input className="plain-input narrow" type="number" aria-label="Character limit" title="Character limit" value={b.limit ?? ''} onChange={(e) => setBlock(i, { ...b, limit: e.target.value ? Number(e.target.value) : undefined })} />
              <label className="row gap small">
                <input type="checkbox" checked={!!b.read_only} onChange={(e) => setBlock(i, { ...b, read_only: e.target.checked || undefined })} /> read-only
              </label>
              <button type="button" className="icon-btn" aria-label="Remove block" onClick={() => setBlock(i, null)}>
                <IconTrash />
              </button>
            </div>
            <input className="plain-input full" placeholder="What the block is for (shown to the agent)" aria-label="Block description" value={b.description ?? ''} onChange={(e) => setBlock(i, { ...b, description: e.target.value || undefined })} />
            <textarea className="form-textarea" rows={2} placeholder="Starting value (used when the block does not exist yet)" aria-label="Block starting value" value={b.value ?? ''} onChange={(e) => setBlock(i, { ...b, value: e.target.value || undefined })} />
          </div>
        ))}
        <div className="span-all">
          <Switch
            size="sm"
            label="Archival memory"
            description="Long-term facts the agent saves with remember and finds with memory_search."
            checked={!!get<boolean>(['memory', 'archival'])}
            onChange={(e) => set(['memory', 'archival'], e.target.checked || undefined)}
          />
        </div>
      </Section>

      {usesMedia && (
        <Section title="Media models" hint="For image_gen, speak and transcribe. Leave empty to use the server's [media] defaults.">
          <Select label="Image" options={[{ value: '', label: 'Server default' }, ...mediaModels('image'), { value: 'mock/image', label: 'mock/image' }]} value={get<string>(['media', 'image']) ?? ''} onValueChange={(v) => set(['media', 'image'], v)} />
          <Select label="Speech" options={[{ value: '', label: 'Server default' }, ...mediaModels('speech'), { value: 'mock/voice', label: 'mock/voice' }]} value={get<string>(['media', 'speech']) ?? ''} onValueChange={(v) => set(['media', 'speech'], v)} />
          <TextField label="Voice" placeholder="alloy, or a voice id" value={get<string>(['media', 'voice']) ?? ''} onChange={(e) => set(['media', 'voice'], e.target.value)} />
          <Select label="Transcription" options={[{ value: '', label: 'Server default' }, ...mediaModels('transcription'), { value: 'mock/ears', label: 'mock/ears' }]} value={get<string>(['media', 'transcription']) ?? ''} onValueChange={(v) => set(['media', 'transcription'], v)} />
        </Section>
      )}

      <Section title="Limits and reasoning" hint="A run stops when it reaches any limit.">
        <NumberInput label="Max steps" description="Model calls per run" min={1} value={get<number>(['limits', 'max_steps'])} onChange={(v) => set(['limits', 'max_steps'], v)} />
        <NumberInput label="Max cost (USD)" step={0.1} min={0} value={get<number>(['limits', 'max_cost_usd'])} onChange={(v) => set(['limits', 'max_cost_usd'], v)} />
        <NumberInput label="Max tokens" description="Input plus output, whole run" min={1} value={get<number>(['limits', 'max_tokens'])} onChange={(v) => set(['limits', 'max_tokens'], v)} />
        <NumberInput label="Time limit (seconds)" min={1} value={get<number>(['limits', 'timeout_secs'])} onChange={(v) => set(['limits', 'timeout_secs'], v)} />
        <Select
          label="Reasoning effort"
          options={[{ value: '', label: "Model's default" }, ...EFFORTS.map((e) => ({ value: e, label: e }))]}
          value={effort}
          onValueChange={(v) => set(['reasoning'], v ? { ...(spec.reasoning ?? {}), effort: v } : undefined)}
        />
        <NumberInput label="Temperature" step={0.1} min={0} value={spec.temperature} onChange={(v) => set(['temperature'], v)} />
        <NumberInput label="Max output tokens" description="Per model call" min={1} value={spec.max_tokens} onChange={(v) => set(['max_tokens'], v)} />
      </Section>

      <Disclosure summary={<span className="muted">Advanced: structured output and context</span>}>
        <Section title="Structured output" hint="A JSON Schema the final answer must follow. Leave empty for plain text.">
          <OutputSchema value={spec.output_schema} onChange={(v) => set(['output_schema'], v)} />
        </Section>
        <Section title="Context window">
          <NumberInput label="Compact at (0-1)" description="Summarize older turns at this share of the window" step={0.05} min={0} value={get<number>(['context', 'compact_at'])} onChange={(v) => set(['context', 'compact_at'], v)} />
          <NumberInput label="Clear tool results at (0-1)" step={0.05} min={0} value={get<number>(['context', 'clear_tool_results_at'])} onChange={(v) => set(['context', 'clear_tool_results_at'], v)} />
          <NumberInput label="Context window override (tokens)" min={1} value={get<number>(['context', 'max_context_tokens'])} onChange={(v) => set(['context', 'max_context_tokens'], v)} />
          <TextField label="Summary model" placeholder="the agent's model" value={get<string>(['context', 'summary_model']) ?? ''} onChange={(e) => set(['context', 'summary_model'], e.target.value)} />
        </Section>
      </Disclosure>
    </>
  );
}

/** JSON edited as text; kept as text until it parses. */
function OutputSchema({ value, onChange }: { value: unknown; onChange: (v: unknown) => void }) {
  const initial = value === undefined ? '' : JSON.stringify(value, null, 2);
  const text = useRef(initial);
  return (
    <TextArea
      label="JSON Schema"
      mono
      rows={6}
      value={text.current || initial}
      placeholder='{"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"]}'
      error={(() => {
        if (!text.current.trim()) return undefined;
        try {
          JSON.parse(text.current);
          return undefined;
        } catch (e) {
          return (e as Error).message;
        }
      })()}
      onChange={(t) => {
        text.current = t;
        if (!t.trim()) return onChange(undefined);
        try {
          onChange(JSON.parse(t));
        } catch {
          onChange(value); // re-render with the text kept; the error shows
        }
      }}
    />
  );
}
