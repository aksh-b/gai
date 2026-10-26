import { useMemo, useRef, useState } from 'react';
import { CheckboxField, SelectField, SwitchField, TextField } from '@/components/fields';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { api, type AgentSpec } from './api';
import { Disclosure } from '@/components/shared';
import { IconPlus, IconTrash } from './icons';
import { NumberInput, PickList, Section, TemplateVars, TextArea, getIn, insertAt, setIn, type PickItem } from './forms';
import { modelLabel, toolLabel } from './labels';
import { modelOptions as groupedModelOptions, useModelGroups } from './models';
import { useDevMode } from './prefs';
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
  { name: 'persona', hint: 'The persona, placed here instead of before or after the instructions' },
];

const PLACEMENTS = [
  { value: '', label: 'Same as the default', description: 'set on the Personas page' },
  { value: 'prepend', label: 'Before the instructions' },
  { value: 'append', label: 'After the instructions' },
];

/** Tools the runtime builds per agent rather than keeping in the registry. */
const AGENT_TOOLS: PickItem[] = [
  {
    value: 'rag_search',
    description: 'Search your documents on the Knowledge page (picking folders below limits it to those).',
    group: 'Built in',
  },
  { value: 'image_gen', description: 'Make images with the image model.', group: 'Voice & images' },
  { value: 'speak', description: 'Turn text into speech.', group: 'Voice & images' },
  { value: 'transcribe', description: 'Turn speech into text.', group: 'Voice & images' },
  { value: 'meeting_notes', description: 'Notes on a meeting recording: decisions, action items, speakers.', group: 'Voice & images' },
  { value: 'analyze_audio', description: 'Who spoke how much, the language, sentiment and key moments.', group: 'Voice & images' },
  { value: 'summarize_video', description: 'Summarize a video from its frames and speech, in chapters.', group: 'Voice & images' },
];

const EFFORTS = ['off', 'low', 'medium', 'high', 'xhigh', 'max'];

const POLICIES = [
  { value: 'auto', label: 'Acts on its own' },
  { value: 'approve', label: 'Asks you first' },
  { value: 'deny', label: 'Never' },
];

const SCOPES = [
  { value: 'agent', label: 'This assistant only' },
  { value: 'user', label: 'About you (shared by all assistants)' },
  { value: 'shared', label: 'Shared with a group' },
];

/** Every part of an agent spec as a form. */
export function AgentForm({ spec, onChange, isNew }: { spec: Spec; onChange: (s: Spec) => void; isNew: boolean }) {
  const [dev] = useDevMode();
  const models = useLoad(() => api.models(), []);
  const groups = useModelGroups();
  const tools = useLoad(() => api.tools(), []);
  const agents = useLoad(() => api.agents(), []);
  const collections = useLoad(() => api.collections(), []);
  const skills = useLoad(() => api.skills(), []);
  const personas = useLoad(() => api.personas(), []);
  const health = useLoad(() => api.health(), []);
  const prompt = useRef<HTMLTextAreaElement>(null);
  const [typing, setTyping] = useState(false);

  const set = (path: (string | number)[], v: unknown) => onChange(setIn(spec, path, v));
  const get = <T,>(path: (string | number)[]) => getIn(spec, path) as T | undefined;

  const chatModels = (models.data ?? []).filter((m) => m.kind === 'chat');
  const modelIds = chatModels.map((m) => m.id);
  const model = spec.model ?? '';
  const usable = groupedModelOptions(groups, dev);
  const modelOptions = [
    {
      value: '',
      label: `Same as new chats (${dev ? (health.data?.default_model ?? 'none') : modelLabel(health.data?.default_model)})`,
      description: 'Follows the choice in Settings',
    },
    ...usable,
    ...(model && !usable.some((o) => o.value === model)
      ? [
          {
            value: model,
            label: dev ? model : modelLabel(model),
            description: modelIds.includes(model) ? 'not connected yet' : 'not in the catalog',
          },
        ]
      : []),
    ...(dev ? [{ value: '__other', label: 'Other address…', description: 'Any provider/model, an alias, or router/<name>' }] : []),
  ];

  const toolItems: PickItem[] = useMemo(() => {
    const out: PickItem[] = [];
    const servers = new Set<string>();
    for (const t of tools.data ?? []) {
      const m = t.spec.name.match(/^mcp__(.+?)__/);
      const group = m ? `Connected app: ${m[1]}` : t.spec.name.startsWith('a2a__') ? 'Remote assistants' : 'Built in';
      if (m) servers.add(m[1]);
      out.push({ value: t.spec.name, label: dev ? t.spec.name : toolLabel(t.spec.name), description: t.spec.description, group });
    }
    for (const s of servers) {
      out.unshift({
        value: `mcp__${s}__*`,
        label: `Everything from ${s}`,
        description: 'Every tool this app has, now and later.',
        group: `Connected app: ${s}`,
      });
    }
    return [...out, ...AGENT_TOOLS.map((t) => ({ ...t, label: dev ? t.value : toolLabel(t.value) }))];
  }, [tools.data, dev]);
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

  const usesMedia = (spec.tools ?? []).some((t) =>
    ['image_gen', 'speak', 'transcribe', 'meeting_notes', 'analyze_audio', 'summarize_video'].includes(t),
  );
  const mediaModels = (kind: string) => (models.data ?? []).filter((m) => m.kind === kind).map((m) => ({ value: m.id, label: m.id }));
  const effort = get<string>(['reasoning', 'effort']) ?? '';

  return (
    <>
      <Section title="Basics">
        <TextField
          label="Name"
          value={spec.name ?? ''}
          disabled={!isNew}
          description={
            isNew ? 'Letters, digits, - and _. It cannot be changed later.' : 'Changes are kept as versions, so you can go back.'
          }
          onChange={(e) => set(['name'], e.target.value)}
        />
        <TextField
          label="Description"
          value={spec.description ?? ''}
          description="One sentence about what it does. Other assistants read it too."
          onChange={(e) => set(['description'], e.target.value)}
        />
        <SelectField
          label="Model"
          options={modelOptions}
          value={typing ? '__other' : model}
          onValueChange={(v) => {
            setTyping(v === '__other');
            if (v !== '__other') set(['model'], v);
          }}
        />
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

      <Section
        title="Instructions"
        hint="What the assistant should do, and how. Words in {{ }} are filled in each time, like today's date."
      >
        <TextArea
          label="Instructions"
          rows={9}
          value={spec.system ?? ''}
          placeholder="You are a careful researcher. Today is {{ date }}…"
          onChange={(v) => set(['system'], v)}
          textareaRef={prompt}
        />
        <TemplateVars vars={PROMPT_VARS} onInsert={(t) => set(['system'], insertAt(prompt.current, spec.system ?? '', t))} />
        <SelectField
          label="Persona"
          description={
            <>
              The voice it speaks in. A chat can still pick another. <a href="#/personas">Manage personas</a>
            </>
          }
          options={[
            { value: '', label: 'Same as the default', description: 'set on the Personas page' },
            { value: 'none', label: 'No persona' },
            ...(personas.data ?? []).map((p) => ({
              value: p.summary.name,
              label: `${p.summary.avatar ? `${p.summary.avatar} ` : ''}${p.summary.display_name || p.summary.name}`,
            })),
            ...(spec.persona && spec.persona !== 'none' && !(personas.data ?? []).some((p) => p.summary.name === spec.persona)
              ? [{ value: spec.persona, label: spec.persona, description: 'a version or label, kept as written' }]
              : []),
          ]}
          value={spec.persona ?? ''}
          onValueChange={(v) => set(['persona'], v)}
        />
        {spec.persona !== 'none' && (
          <SelectField
            label="Where the persona goes"
            description={
              /\{\{-?\s*persona\b/.test(spec.system ?? '')
                ? 'Your instructions place it with {{ persona }}, so this is not used.'
                : 'Before keeps answers faster with models that cache prompts.'
            }
            options={PLACEMENTS}
            value={spec.persona_placement ?? ''}
            onValueChange={(v) => set(['persona_placement'], v)}
          />
        )}
      </Section>

      <Section title="Abilities" hint="What the assistant can do. For each one, choose whether it acts on its own or asks you first.">
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
              <SelectField
                aria-label={`Policy for ${name}`}
                className="w-40"
                options={POLICIES}
                value={spec.tool_policy?.[name] ?? defaultPolicy(name)}
                onValueChange={(v) => set(['tool_policy', name], v === defaultPolicy(name) ? undefined : v)}
              />
            )
          }
        />
      </Section>

      <Section title="Can ask other assistants" hint="Hand parts of a task to other assistants, as a team lead would.">
        <PickList
          items={(agents.data ?? [])
            .filter((a) => a.summary.name !== spec.name)
            .map((a) => ({ value: a.summary.name, description: a.spec?.description ?? a.spec?.model ?? '' }))}
          selected={localDelegates}
          onChange={(v) => setDelegates(v, remoteDelegates)}
          empty="No other assistants yet."
          maxHeight={200}
        />
        {otherDelegates.length > 0 && (
          <FieldDescription className="col-span-full">
            {otherDelegates.length} more with custom names; edit them with developer details on.
          </FieldDescription>
        )}
        {dev && (
          <Field className="col-span-full">
            <FieldLabel>Remote agents (A2A)</FieldLabel>
            {remoteDelegates.map((r, i) => (
              <div key={i} className="flex gap-2">
                <Input
                  className="w-40"
                  placeholder="name"
                  aria-label="Remote agent name"
                  value={r.name}
                  onChange={(e) =>
                    setDelegates(
                      localDelegates,
                      remoteDelegates.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)),
                    )
                  }
                />
                <Input
                  placeholder="https://host/a2a/agent or its agent card URL"
                  aria-label="Remote agent URL"
                  value={r.a2a}
                  onChange={(e) =>
                    setDelegates(
                      localDelegates,
                      remoteDelegates.map((x, j) => (j === i ? { ...x, a2a: e.target.value } : x)),
                    )
                  }
                />
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Remove remote agent"
                  onClick={() =>
                    setDelegates(
                      localDelegates,
                      remoteDelegates.filter((_, j) => j !== i),
                    )
                  }
                >
                  <IconTrash />
                </Button>
              </div>
            ))}
            <Button
              variant="ghost"
              size="sm"
              className="w-fit"
              onClick={() => setDelegates(localDelegates, [...remoteDelegates, { a2a: '', name: '' }])}
            >
              <IconPlus data-icon="inline-start" />
              Add a remote agent
            </Button>
          </Field>
        )}
      </Section>

      <Section title="Knowledge and skills">
        <Field className="col-span-full">
          <FieldLabel>Document folders it can search</FieldLabel>
          <PickList
            items={(collections.data ?? []).map((c) => ({ value: c.name, description: c.description ?? `${c.documents} documents` }))}
            selected={(spec.knowledge as string[] | undefined) ?? []}
            onChange={(v) => set(['knowledge'], v)}
            empty="No documents yet: add some on the Knowledge page."
            maxHeight={180}
          />
        </Field>
        <Field className="col-span-full">
          <FieldLabel>Skills (step-by-step know-how it reads when needed)</FieldLabel>
          <PickList
            items={(skills.data ?? []).map((s) => ({ value: s.name, description: s.description }))}
            selected={(spec.skills as string[] | undefined) ?? []}
            onChange={(v) => set(['skills'], v)}
            empty="No skills yet: add some on the Skills page."
            maxHeight={180}
          />
        </Field>
      </Section>

      <Section
        title="Safety"
        hint="Applies on top of the server's settings. Leave a field on “Server default” to keep what gai.toml says."
      >
        <SelectField
          label="Personal data"
          description="Emails, phone and card numbers, IBANs, IDs and IP addresses in messages, replies and tool results."
          options={[
            { value: '', label: 'Server default' },
            { value: 'off', label: 'Allow' },
            { value: 'warn', label: 'Allow, but note it' },
            { value: 'mask', label: 'Mask it before the model sees it' },
            { value: 'block', label: 'Refuse messages that contain it' },
          ]}
          value={spec.safety?.pii ?? ''}
          onValueChange={(v) => set(['safety', 'pii'], v || undefined)}
        />
        <SelectField
          label="Instructions hidden in web pages and files"
          description="Text in tool results that tries to give the assistant orders (prompt injection)."
          options={[
            { value: '', label: 'Server default (warn the assistant)' },
            { value: 'warn', label: 'Warn the assistant' },
            { value: 'strip', label: 'Remove those lines' },
            { value: 'block', label: 'Withhold the whole result' },
            { value: 'off', label: 'Don’t check' },
          ]}
          value={spec.safety?.injection ?? ''}
          onValueChange={(v) => set(['safety', 'injection'], v || undefined)}
        />
        <TextField
          label="Refuse requests about"
          description="Comma-separated topics, e.g. medical advice, legal advice."
          value={(spec.safety?.deny_topics ?? []).join(', ')}
          onChange={(e) => {
            const t = e.target.value
              .split(',')
              .map((x) => x.trim())
              .filter(Boolean);
            set(['safety', 'deny_topics'], t.length ? t : undefined);
          }}
        />
        <NumberInput
          label="Keep conversations for (days)"
          description="Older conversations with this assistant are deleted. Empty keeps them."
          min={0}
          value={spec.safety?.retention_days ?? undefined}
          onChange={(v) => set(['safety', 'retention_days'], v)}
        />
      </Section>

      <Disclosure summary="Advanced: memory, limits, reasoning and answer format" open={dev}>
        <div className="flex flex-col gap-4">
          <Section
            title="Memory"
            hint="Notes the assistant always sees and updates as it learns, like what it knows about you."
            aside={
              <Button
                variant="ghost"
                size="sm"
                onClick={() =>
                  set(['memory', 'blocks'], [...blocks, { label: blocks.length ? `notes${blocks.length + 1}` : 'notes', limit: 2000 }])
                }
              >
                <IconPlus data-icon="inline-start" />
                Add block
              </Button>
            }
          >
            {blocks.map((b, i) => (
              <Card key={i} size="sm" className="col-span-full">
                <CardContent className="flex flex-col gap-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <Input
                      className="w-36"
                      aria-label="Block label"
                      placeholder="label"
                      value={b.label}
                      onChange={(e) => setBlock(i, { ...b, label: e.target.value })}
                    />
                    <SelectField
                      aria-label="Block scope"
                      className="w-64"
                      options={SCOPES}
                      value={b.scope?.startsWith('shared:') ? 'shared' : (b.scope ?? 'agent')}
                      onValueChange={(v) =>
                        setBlock(i, { ...b, scope: v === 'agent' ? undefined : v === 'shared' ? 'shared:team' : 'user' })
                      }
                    />
                    {b.scope?.startsWith('shared:') && (
                      <Input
                        className="w-36"
                        aria-label="Shared group name"
                        value={b.scope.slice(7)}
                        onChange={(e) => setBlock(i, { ...b, scope: `shared:${e.target.value}` })}
                      />
                    )}
                    <Input
                      className="w-28"
                      type="number"
                      aria-label="Character limit"
                      title="Character limit"
                      value={b.limit ?? ''}
                      onChange={(e) => setBlock(i, { ...b, limit: e.target.value ? Number(e.target.value) : undefined })}
                    />
                    <CheckboxField
                      className="w-auto"
                      label="Read-only"
                      checked={!!b.read_only}
                      onCheckedChange={(c) => setBlock(i, { ...b, read_only: c || undefined })}
                    />
                    <Button variant="ghost" size="icon" className="ml-auto" aria-label="Remove block" onClick={() => setBlock(i, null)}>
                      <IconTrash />
                    </Button>
                  </div>
                  <Input
                    placeholder="What the block is for (shown to the agent)"
                    aria-label="Block description"
                    value={b.description ?? ''}
                    onChange={(e) => setBlock(i, { ...b, description: e.target.value || undefined })}
                  />
                  <Textarea
                    rows={2}
                    placeholder="Starting value (used when the block does not exist yet)"
                    aria-label="Block starting value"
                    value={b.value ?? ''}
                    onChange={(e) => setBlock(i, { ...b, value: e.target.value || undefined })}
                  />
                </CardContent>
              </Card>
            ))}
            <SwitchField
              size="sm"
              className="col-span-full"
              label="Long-term memory"
              description="Facts the assistant saves and looks up later, beyond its notes."
              checked={!!get<boolean>(['memory', 'archival'])}
              onCheckedChange={(checked) => set(['memory', 'archival'], checked || undefined)}
            />
          </Section>

          {usesMedia && (
            <Section title="Voice and image models" hint="Leave empty to use the ones in Settings → Voice & images.">
              <SelectField
                label="Image"
                options={[{ value: '', label: 'Server default' }, ...mediaModels('image'), { value: 'mock/image', label: 'mock/image' }]}
                value={get<string>(['media', 'image']) ?? ''}
                onValueChange={(v) => set(['media', 'image'], v)}
              />
              <SelectField
                label="Speech"
                options={[{ value: '', label: 'Server default' }, ...mediaModels('speech'), { value: 'mock/voice', label: 'mock/voice' }]}
                value={get<string>(['media', 'speech']) ?? ''}
                onValueChange={(v) => set(['media', 'speech'], v)}
              />
              <TextField
                label="Voice"
                placeholder="alloy, or a voice id"
                value={get<string>(['media', 'voice']) ?? ''}
                onChange={(e) => set(['media', 'voice'], e.target.value)}
              />
              <SelectField
                label="Transcription"
                options={[
                  { value: '', label: 'Server default' },
                  ...mediaModels('transcription'),
                  { value: 'mock/ears', label: 'mock/ears' },
                ]}
                value={get<string>(['media', 'transcription']) ?? ''}
                onValueChange={(v) => set(['media', 'transcription'], v)}
              />
              <TextField
                label="Vision (video frames)"
                placeholder="The assistant's own model"
                value={get<string>(['media', 'vision']) ?? ''}
                onChange={(e) => set(['media', 'vision'], e.target.value)}
              />
            </Section>
          )}
          {(usesMedia || get<string>(['media', 'realtime'])) && (
            <Section
              title="Live voice"
              hint="With a realtime model, voice chats go straight to it (speech in and out); otherwise they are transcribed, answered and spoken."
            >
              <TextField
                label="Realtime model"
                placeholder="openai/gpt-realtime"
                value={get<string>(['media', 'realtime']) ?? ''}
                onChange={(e) => set(['media', 'realtime'], e.target.value)}
              />
            </Section>
          )}

          <Section title="Limits and reasoning" hint="The assistant stops when it reaches any of these.">
            <NumberInput
              label="Max steps"
              description="Model calls per run"
              min={1}
              value={get<number>(['limits', 'max_steps'])}
              onChange={(v) => set(['limits', 'max_steps'], v)}
            />
            <NumberInput
              label="Max cost (USD)"
              step={0.1}
              min={0}
              value={get<number>(['limits', 'max_cost_usd'])}
              onChange={(v) => set(['limits', 'max_cost_usd'], v)}
            />
            <NumberInput
              label="Max tokens"
              description="Input plus output, whole run"
              min={1}
              value={get<number>(['limits', 'max_tokens'])}
              onChange={(v) => set(['limits', 'max_tokens'], v)}
            />
            <NumberInput
              label="Time limit (seconds)"
              min={1}
              value={get<number>(['limits', 'timeout_secs'])}
              onChange={(v) => set(['limits', 'timeout_secs'], v)}
            />
            <SelectField
              label="Reasoning effort"
              options={[{ value: '', label: "Model's default" }, ...EFFORTS.map((e) => ({ value: e, label: e }))]}
              value={effort}
              onValueChange={(v) => set(['reasoning'], v ? { ...(spec.reasoning ?? {}), effort: v } : undefined)}
            />
            <NumberInput label="Temperature" step={0.1} min={0} value={spec.temperature} onChange={(v) => set(['temperature'], v)} />
            <NumberInput
              label="Max output tokens"
              description="Per model call"
              min={1}
              value={spec.max_tokens}
              onChange={(v) => set(['max_tokens'], v)}
            />
          </Section>

          <Section title="Answer format" hint="A JSON Schema the final answer must follow. Leave empty for plain text.">
            <OutputSchema value={spec.output_schema} onChange={(v) => set(['output_schema'], v)} />
          </Section>
          <Section title="Long conversations" hint="How the assistant makes room when a chat gets too long for the model.">
            <Percent
              label="Summarize older messages at"
              description="of the model's memory"
              value={get<number>(['context', 'compact_at'])}
              onChange={(v) => set(['context', 'compact_at'], v)}
            />
            <Percent
              label="Drop old tool results at"
              description="of the model's memory"
              value={get<number>(['context', 'clear_tool_results_at'])}
              onChange={(v) => set(['context', 'clear_tool_results_at'], v)}
            />
            <NumberInput
              label="Context window override (tokens)"
              min={1}
              value={get<number>(['context', 'max_context_tokens'])}
              onChange={(v) => set(['context', 'max_context_tokens'], v)}
            />
            <TextField
              label="Summary model"
              placeholder="the agent's model"
              value={get<string>(['context', 'summary_model']) ?? ''}
              onChange={(e) => set(['context', 'summary_model'], e.target.value)}
            />
          </Section>
        </div>
      </Disclosure>
    </>
  );
}

/** A 0-1 share, edited as a percentage. */
function Percent({
  label,
  description,
  value,
  onChange,
}: {
  label: string;
  description?: string;
  value?: number;
  onChange: (v?: number) => void;
}) {
  return (
    <NumberInput
      label={`${label} (%)`}
      description={description}
      min={0}
      step={5}
      value={value === undefined ? undefined : Math.round(value * 100)}
      onChange={(v) => onChange(v === undefined ? undefined : Math.min(100, Math.max(0, v)) / 100)}
    />
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
