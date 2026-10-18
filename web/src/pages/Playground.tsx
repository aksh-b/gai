import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Button, Select, Switch, Tooltip, toast } from '@flame';
import {
  api,
  fileToBase64,
  streamRun,
  type AgentSpec,
  type ContentBlock,
  type EventEnvelope,
  type MediaDefaults,
  type Session,
  type StoredMessage,
  type Usage,
} from '../api';
import { Disclosure, Json, Markdown } from '../components';
import { IconCheck, IconClip, IconMic, IconPlus, IconSend, IconSpark, IconSpeaker, IconStop, IconTrash, IconX } from '../icons';
import { modelLabel, toolLabel, toolSentence } from '../labels';
import { blobToBase64, blobUrl, MediaView, mediaBlocks, useRecorder } from '../Media';
import { modelOptions, useModelGroups } from '../models';
import { useDevMode } from '../prefs';
import { navigate, type Route } from '../router';
import { ago, blocksText, money, tokens, totalIn } from '../util';
import { stashDraft } from './Agents';

type Item =
  | { kind: 'user'; text: string; media?: ContentBlock[]; files?: string[] }
  | { kind: 'assistant'; text: string }
  | { kind: 'reasoning'; text: string }
  | {
      kind: 'tool';
      id: string;
      name: string;
      input: unknown;
      runId?: string;
      result?: { ok: boolean; text: string; ms: number; media?: ContentBlock[] };
      approval?: 'pending' | 'approved' | 'declined';
    }
  | { kind: 'note'; text: string; tone: 'muted' | 'warn' | 'bad'; action?: { label: string; to: string } };

/** A file waiting in the composer. */
interface Attachment {
  name: string;
  /** Images and audio go to the model as they are. */
  block?: ContentBlock;
  /** Small text files are pasted into the message. */
  text?: string;
  /** Other documents are added to Knowledge for the assistant to search. */
  knowledge?: boolean;
}

const ZERO: Usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, reasoning_tokens: 0 };

const SUGGESTIONS_TOOLS = [
  'What is 17% of 2,340, and what weekday is it today?',
  'Read https://example.com and sum it up in one sentence.',
  'Write a short poem about autumn into poem.txt, then read it back.',
];
const SUGGESTIONS_CHAT = [
  'Explain compound interest with a quick example.',
  'Give me three ideas for a weekend project.',
  'Write a short poem about autumn.',
];

/** The collection files dropped into a chat go to. */
const DROP_COLLECTION = 'my-documents';
const TEXT_TYPES = /^(text\/|application\/(json|xml|x-yaml|yaml|javascript|typescript))/;
const TEXT_EXT = /\.(txt|md|markdown|csv|json|ya?ml|xml|html?|js|ts|tsx|py|rs|go|java|c|cpp|h|sh|toml|ini|log)$/i;

function itemsFromHistory(messages: StoredMessage[]): Item[] {
  const items: Item[] = [];
  for (const { message } of messages) {
    if (message.role === 'user') {
      const media = mediaBlocks(message.content);
      if (media.length) items.push({ kind: 'user', text: blocksText(message.content), media });
      if (media.length) continue;
    }
    for (const b of message.content) {
      if (b.type === 'text' && b.text.trim()) items.push({ kind: message.role === 'user' ? 'user' : 'assistant', text: b.text });
      else if (b.type === 'thinking' && b.text.trim()) items.push({ kind: 'reasoning', text: b.text });
      else if (b.type === 'tool_use') items.push({ kind: 'tool', id: b.id, name: b.name, input: b.input });
      else if (b.type === 'tool_result') {
        const t = items.find((i) => i.kind === 'tool' && i.id === b.tool_use_id);
        if (t && t.kind === 'tool')
          t.result = { ok: !b.is_error, text: blocksText(b.content), ms: 0, media: mediaBlocks(b.content) };
      }
    }
  }
  return items;
}

/** A run error, and where to fix it when we can tell. */
function explain(error: string): { text: string; action?: { label: string; to: string } } {
  const l = error.toLowerCase();
  if (l.includes('api key') || l.includes('no key') || l.includes('_api_key') || l.includes('unauthorized') || l.includes('401'))
    return { text: `The model could not be used: ${error}`, action: { label: 'Connect a model', to: '/settings' } };
  if (l.includes('no model') || l.includes('no default model'))
    return { text: 'No AI model is set up yet.', action: { label: 'Connect one', to: '/welcome' } };
  if (l.includes('exceeds the available context') || l.includes('context length') || l.includes('context window') || l.includes('too many tokens'))
    return {
      text: "This doesn't fit in the model's memory: the conversation plus the assistant's tools are too long. Start a new chat, give the assistant fewer tools, or pick a model with more room.",
      action: { label: 'Models', to: '/settings' },
    };
  if (l.includes('does not support tools'))
    return { text: `${error}. Pick a model that can use tools, or an assistant without tools.`, action: { label: 'Models', to: '/settings' } };
  if (l.includes('connect') && (l.includes('refused') || l.includes('error sending')))
    return { text: `Could not reach the model's server. ${error}`, action: { label: 'Check models', to: '/settings' } };
  return { text: error };
}

export function Playground({ route }: { route: Route }) {
  const [dev] = useDevMode();
  const sessionParam = route.query.get('session');
  const agentParam = route.query.get('agent') ?? '';

  const [agents, setAgents] = useState<{ name: string; spec: AgentSpec | null }[]>([]);
  const [defaultSpec, setDefaultSpec] = useState<AgentSpec | null>(null);
  const groups = useModelGroups();
  const [sessions, setSessions] = useState<Session[]>([]);
  const [agent, setAgent] = useState(agentParam);
  const [model, setModel] = useState('');
  const [autoApprove, setAutoApprove] = useState(false);
  const [items, setItems] = useState<Item[]>([]);
  const [input, setInput] = useState('');
  const [files, setFiles] = useState<Attachment[]>([]);
  const [dragging, setDragging] = useState(false);
  const [running, setRunning] = useState<{ runId?: string; abort: AbortController } | null>(null);
  const [usage, setUsage] = useState<{ usage: Usage; cost: number }>({ usage: ZERO, cost: 0 });
  const [media, setMedia] = useState<MediaDefaults | null>(null);
  const recorder = useRecorder();
  const [transcribing, setTranscribing] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const filePicker = useRef<HTMLInputElement>(null);
  const stick = useRef(true);
  const sessionRef = useRef<string | null>(sessionParam);
  // Tools the person said may run without asking, for this chat.
  const allowed = useRef(new Set<string>());

  const loadSessions = useCallback(() => api.sessions().then(setSessions).catch(() => {}), []);

  useEffect(() => {
    api.agents().then((a) => setAgents(a.map((x) => ({ name: x.summary.name, spec: x.spec })))).catch(() => {});
    api.defaultAgent().then(setDefaultSpec).catch(() => {});
    api.mediaDefaults().then(setMedia).catch(() => {});
    loadSessions();
  }, [loadSessions]);

  useEffect(() => setAgent(agentParam), [agentParam]);

  // Load the transcript when the session in the URL changes.
  useEffect(() => {
    if (sessionParam === sessionRef.current && items.length) return;
    sessionRef.current = sessionParam;
    allowed.current = new Set();
    setUsage({ usage: ZERO, cost: 0 });
    if (!sessionParam) {
      setItems([]);
      return;
    }
    api
      .messages(sessionParam)
      .then((m) => {
        setItems(itemsFromHistory(m));
        stick.current = true;
      })
      .catch((e: Error) => toast.error(e.message));
  }, [sessionParam]);

  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items]);

  const activeSpec = useMemo(
    () => (agent ? agents.find((a) => a.name === agent)?.spec : defaultSpec) ?? null,
    [agent, agents, defaultSpec],
  );
  const effectiveModel = model || activeSpec?.model || groups.defaultModel || '';
  const modelInfo = [...groups.cloud, ...groups.local, ...groups.automatic, ...groups.demo].find((m) => m.id === effectiveModel);
  const canUseTools = modelInfo?.tools ?? true;

  const update = (fn: (items: Item[]) => Item[]) => setItems((prev) => fn([...prev]));

  async function decide(runId: string | undefined, callId: string, approved: boolean, name?: string, always = false) {
    if (!runId) return;
    if (always && name) allowed.current.add(name);
    try {
      await api.decide(runId, callId, approved, approved ? undefined : 'declined in the dashboard');
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const onEvent = useCallback(
    (e: EventEnvelope) => {
      switch (e.type) {
        case 'run_started':
          setRunning((r) => (r ? { ...r, runId: e.run_id } : r));
          if (sessionRef.current !== e.session_id) {
            sessionRef.current = e.session_id;
            navigate(`/chat?session=${e.session_id}${agent ? `&agent=${encodeURIComponent(agent)}` : ''}`);
            loadSessions();
          }
          break;
        case 'text_delta':
          update((it) => {
            const last = it[it.length - 1];
            if (last?.kind === 'assistant') it[it.length - 1] = { ...last, text: last.text + e.text };
            else it.push({ kind: 'assistant', text: e.text });
            return it;
          });
          break;
        case 'reasoning_delta':
          update((it) => {
            const last = it[it.length - 1];
            if (last?.kind === 'reasoning') it[it.length - 1] = { ...last, text: last.text + e.text };
            else it.push({ kind: 'reasoning', text: e.text });
            return it;
          });
          break;
        case 'tool_call':
          update((it) => [...it, { kind: 'tool', id: e.id, name: e.name, input: e.input, runId: e.run_id }]);
          break;
        case 'approval_required':
          if (allowed.current.has(e.name)) {
            decide(e.run_id, e.id, true);
            break;
          }
          update((it) => it.map((x) => (x.kind === 'tool' && x.id === e.id ? { ...x, approval: 'pending', runId: e.run_id } : x)));
          break;
        case 'approval_resolved':
          update((it) =>
            it.map((x) => (x.kind === 'tool' && x.id === e.id ? { ...x, approval: e.approved ? 'approved' : 'declined' } : x)),
          );
          break;
        case 'tool_result':
          update((it) =>
            it.map((x) =>
              x.kind === 'tool' && x.id === e.id
                ? {
                    ...x,
                    result: {
                      ok: !e.output.is_error,
                      text: blocksText(e.output.content),
                      ms: e.duration_ms,
                      media: mediaBlocks(e.output.content),
                    },
                  }
                : x,
            ),
          );
          break;
        case 'usage':
          setUsage((u) => ({
            usage: {
              input_tokens: u.usage.input_tokens + e.usage.input_tokens,
              output_tokens: u.usage.output_tokens + e.usage.output_tokens,
              cache_read_tokens: u.usage.cache_read_tokens + e.usage.cache_read_tokens,
              cache_write_tokens: u.usage.cache_write_tokens + e.usage.cache_write_tokens,
              reasoning_tokens: u.usage.reasoning_tokens + e.usage.reasoning_tokens,
            },
            cost: u.cost + (e.cost ?? 0),
          }));
          break;
        case 'warning':
          update((it) => [...it, { kind: 'note', text: e.message, tone: 'warn' }]);
          break;
        case 'model_routed':
          if (dev) update((it) => [...it, { kind: 'note', text: `Routed to ${e.model}: ${e.reason}`, tone: 'muted' }]);
          break;
        case 'compacted':
          update((it) => [...it, { kind: 'note', text: 'Summarized earlier messages to make room.', tone: 'muted' }]);
          break;
        case 'run_finished':
          if (e.status !== 'completed') {
            const why = e.status === 'cancelled' ? { text: 'Stopped.' } : explain(e.error ?? e.status.replace('_', ' '));
            update((it) => [...it, { kind: 'note', ...why, tone: e.status === 'cancelled' ? 'muted' : 'bad' }]);
          }
          break;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [agent, loadSessions, dev],
  );

  async function addFiles(list: FileList | File[]) {
    for (const file of Array.from(list)) {
      try {
        if (file.type.startsWith('image/') || file.type.startsWith('audio/')) {
          const blob = await api.uploadBlob(file, file.name);
          const source = { kind: 'blob' as const, id: blob.id, media_type: file.type };
          const block: ContentBlock = file.type.startsWith('image/') ? { type: 'image', source } : { type: 'audio', source };
          setFiles((f) => [...f, { name: file.name, block }]);
        } else if ((TEXT_TYPES.test(file.type) || TEXT_EXT.test(file.name)) && file.size < 150_000) {
          setFiles((f) => [...f, { name: file.name, text: '' }]);
          const text = await file.text();
          setFiles((f) => f.map((x) => (x.name === file.name && x.text === '' ? { ...x, text } : x)));
        } else {
          toast.loading(`Adding ${file.name} to your documents…`, { id: `kn-${file.name}` });
          const cols = await api.collections();
          if (!cols.some((c) => c.name === DROP_COLLECTION)) {
            await api.createCollection({ name: DROP_COLLECTION, description: 'Files added from chats.' });
          }
          await api.addDocument(DROP_COLLECTION, {
            filename: file.name,
            content_type: file.type || undefined,
            content_base64: await fileToBase64(file),
          });
          toast.success(`${file.name} is in your documents; ask about it`, { id: `kn-${file.name}` });
          setFiles((f) => [...f, { name: file.name, knowledge: true }]);
        }
      } catch (e) {
        toast.error(`${file.name}: ${(e as Error).message}`, { id: `kn-${file.name}` });
      }
    }
  }

  async function send(text?: string) {
    const typed = (text ?? input).trim();
    if ((!typed && !files.length) || running) return;
    const attached = files;
    setInput('');
    setFiles([]);
    stick.current = true;
    const blocks: ContentBlock[] = [];
    const notes = attached
      .filter((f) => f.text !== undefined)
      .map((f) => `\n\n--- ${f.name} ---\n${f.text}`)
      .join('');
    const docs = attached.filter((f) => f.knowledge).map((f) => f.name);
    const message =
      (typed || (docs.length ? 'Please look at the attached file.' : 'Please look at this.')) +
      (docs.length ? `\n\n(I added ${docs.join(', ')} to my documents; search them to answer.)` : '') +
      notes;
    for (const f of attached) if (f.block) blocks.push(f.block);
    update((it) => [
      ...it,
      { kind: 'user', text: typed, media: blocks.length ? blocks : undefined, files: attached.filter((f) => !f.block).map((f) => f.name) },
    ]);
    const abort = new AbortController();
    setRunning({ abort });
    try {
      await streamRun(
        {
          agent: agent || undefined,
          input: blocks.length ? [{ type: 'text', text: message }, ...blocks] : message,
          session_id: sessionRef.current ?? undefined,
          model: model || undefined,
          auto_approve: autoApprove,
        },
        onEvent,
        abort.signal,
      );
    } catch (e) {
      if ((e as Error).name !== 'AbortError') {
        update((it) => [...it, { kind: 'note', ...explain((e as Error).message), tone: 'bad' }]);
      }
    } finally {
      setRunning(null);
      loadSessions();
    }
  }

  /** Mic button: start recording, or stop and put the words in the box. */
  async function toggleMic() {
    if (!recorder.recording) {
      try {
        await recorder.start();
      } catch (e) {
        toast.error(`Microphone: ${(e as Error).message}`);
      }
      return;
    }
    const audio = await recorder.stop();
    if (!audio.size) return;
    setTranscribing(true);
    try {
      const t = await api.transcribe({ audio_base64: await blobToBase64(audio), media_type: audio.type || 'audio/webm' });
      setInput((v) => (v.trim() ? `${v.trimEnd()} ${t.text}` : t.text));
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setTranscribing(false);
    }
  }

  async function stop() {
    if (running?.runId) await api.cancel(running.runId).catch(() => {});
    else running?.abort.abort();
  }

  useEffect(() => {
    if (!running) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') stop();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  async function removeSession(id: string) {
    await api.deleteSession(id).catch((e: Error) => toast.error(e.message));
    if (sessionRef.current === id) navigate('/chat');
    loadSessions();
  }

  function saveAsAssistant() {
    const base = activeSpec ?? { name: '' };
    stashDraft({
      ...base,
      name: '',
      description: base.description,
      model: model || base.model,
      system: base.system,
      tools: base.tools,
    });
    navigate('/assistants/new');
  }

  const agentOptions = [
    { value: '', label: 'General assistant', description: 'Built in: files, web pages, calculator' },
    ...agents.map((a) => ({ value: a.name, label: a.name, description: a.spec?.description ?? '' })),
  ];
  const defaultLabel = activeSpec?.model
    ? `${agent || 'Assistant'}'s model (${dev ? activeSpec.model : modelLabel(activeSpec.model)})`
    : `Default (${dev ? (groups.defaultModel ?? 'none') : modelLabel(groups.defaultModel)})`;
  const options = [{ value: '', label: defaultLabel, description: 'Change the default in Settings' }, ...modelOptions(groups, dev)];
  const nothingConnected = groups.loaded && !groups.cloud.length && !groups.local.length;
  const suggestions = canUseTools && (activeSpec?.tools?.length ?? 0) > 0 ? SUGGESTIONS_TOOLS : SUGGESTIONS_CHAT;

  return (
    <div className="playground">
      <aside className="sessions">
        <Button
          variant="soft"
          tone="accent"
          fullWidth
          leading={<IconPlus />}
          onClick={() => {
            sessionRef.current = null;
            setItems([]);
            navigate(`/chat${agent ? `?agent=${encodeURIComponent(agent)}` : ''}`);
          }}
        >
          New chat
        </Button>
        <div className="session-list">
          {sessions.map((s) => (
            <div key={s.id} className="session" aria-current={s.id === sessionParam ? 'true' : undefined}>
              <a href={`#/chat?session=${s.id}`} className="session-link">
                <span className="session-title">{s.title || 'Untitled'}</span>
                <span className="muted small">
                  {s.agent ?? 'assistant'} · {ago(s.updated_at)}
                </span>
              </a>
              <button type="button" className="icon-btn" aria-label="Delete chat" onClick={() => removeSession(s.id)}>
                <IconTrash />
              </button>
            </div>
          ))}
          {sessions.length === 0 && <p className="muted small pad">Your chats will show up here.</p>}
        </div>
      </aside>

      <section
        className="conversation"
        data-dragging={dragging || undefined}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes('Files')) {
            e.preventDefault();
            setDragging(true);
          }
        }}
        onDragLeave={(e) => {
          if (e.currentTarget === e.target) setDragging(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          if (e.dataTransfer.files.length) addFiles(e.dataTransfer.files);
        }}
      >
        <div className="toolbar">
          <div className="toolbar-field">
            <Select aria-label="Assistant" options={agentOptions} value={agent} onValueChange={setAgent} />
          </div>
          <div className="toolbar-field wide">
            <Select aria-label="Model" options={options} value={model} onValueChange={setModel} />
          </div>
          <a className="small link-quiet" href="#/settings">
            + Connect models
          </a>
          <div className="spacer" />
          <Tooltip content="When off, it asks before doing anything that needs your OK">
            <Switch size="sm" label="Act without asking" checked={autoApprove} onChange={(e) => setAutoApprove(e.target.checked)} />
          </Tooltip>
          {dev ? (
            <span className="chip mono" title="Tokens this conversation (since opened)">
              {tokens(totalIn(usage.usage))} in · {tokens(usage.usage.output_tokens)} out
              {usage.cost > 0 && ` · ${money(usage.cost)}`}
            </span>
          ) : (
            usage.cost > 0 && (
              <span className="chip" title="What this chat has cost so far">
                {money(usage.cost)}
              </span>
            )
          )}
        </div>

        <div
          className="transcript"
          ref={scroller}
          onScroll={(e) => {
            const el = e.currentTarget;
            stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
          }}
        >
          {items.length === 0 ? (
            <div className="welcome">
              <div className="welcome-mark">
                <IconSpark width={28} height={28} />
              </div>
              <h2>How can {agent || 'I'} help?</h2>
              <p className="muted">
                {effectiveModel ? (
                  <>Using {dev ? <span className="mono">{effectiveModel}</span> : modelLabel(effectiveModel)}.</>
                ) : (
                  'No AI model is connected yet.'
                )}{' '}
                {!canUseTools && 'This model can chat but not use tools. '}
                You can also drop files here.
              </p>
              {nothingConnected && (
                <div className="setup-nudge">
                  <span>
                    {effectiveModel.startsWith('mock/')
                      ? 'You are using the demo model: it repeats what you say.'
                      : 'Connect an AI model to get real answers.'}
                  </span>
                  <Button size="sm" tone="accent" onClick={() => navigate('/welcome')}>
                    Connect a model
                  </Button>
                </div>
              )}
              <div className="suggestions">
                {suggestions.map((s) => (
                  <button key={s} type="button" className="suggestion" onClick={() => send(s)}>
                    {s}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <div className="thread">
              {items.map((it, i) => (
                <TranscriptItem key={i} item={it} onDecide={decide} canSpeak={!!media?.speech} dev={dev} />
              ))}
              {running && items[items.length - 1]?.kind === 'user' && <div className="typing" aria-label="Working" />}
              {!running && items.length > 1 && (
                <div className="thread-foot">
                  <button type="button" className="link small" onClick={saveAsAssistant}>
                    Save this setup as an assistant
                  </button>
                </div>
              )}
            </div>
          )}
          {dragging && (
            <div className="drop-hint" aria-hidden="true">
              Drop files to add them to your message
            </div>
          )}
        </div>

        <form
          className="composer"
          onSubmit={(e) => {
            e.preventDefault();
            send();
          }}
        >
          {files.length > 0 && (
            <div className="attachments">
              {files.map((f, i) => (
                <span key={i} className="chip attachment">
                  {f.name}
                  {f.knowledge && <em className="muted"> · in your documents</em>}
                  <button type="button" className="icon-btn" aria-label={`Remove ${f.name}`} onClick={() => setFiles((x) => x.filter((_, j) => j !== i))}>
                    <IconX width={12} height={12} />
                  </button>
                </span>
              ))}
            </div>
          )}
          <input
            ref={filePicker}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              if (e.target.files?.length) addFiles(e.target.files);
              e.target.value = '';
            }}
          />
          <Tooltip content="Attach files (or drop them here)">
            <Button type="button" variant="ghost" leading={<IconClip />} aria-label="Attach files" onClick={() => filePicker.current?.click()} />
          </Tooltip>
          <textarea
            value={input}
            placeholder={running ? 'Working… you can type your next message' : 'Message  (Enter to send, Shift+Enter for a new line)'}
            rows={Math.min(8, Math.max(1, input.split('\n').length))}
            onChange={(e) => setInput(e.target.value)}
            onPaste={(e) => {
              if (e.clipboardData.files.length) {
                e.preventDefault();
                addFiles(e.clipboardData.files);
              }
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
            autoFocus
          />
          {media?.transcription && recorder.supported && (
            <Tooltip content={recorder.recording ? 'Stop and turn into text' : 'Speak your message'}>
              <Button
                type="button"
                variant={recorder.recording ? 'soft' : 'ghost'}
                tone={recorder.recording ? 'danger' : undefined}
                loading={transcribing}
                onClick={toggleMic}
                leading={<IconMic />}
                aria-label={recorder.recording ? 'Stop recording' : 'Record a message'}
              >
                {recorder.recording ? `${recorder.seconds}s` : undefined}
              </Button>
            </Tooltip>
          )}
          {running ? (
            <Tooltip content="Stop" shortcut="Esc">
              <Button type="button" tone="danger" variant="soft" onClick={stop} leading={<IconStop />}>
                Stop
              </Button>
            </Tooltip>
          ) : (
            <Button type="submit" tone="accent" disabled={!input.trim() && !files.length} leading={<IconSend />}>
              Send
            </Button>
          )}
        </form>
      </section>
    </div>
  );
}

/** Read a reply aloud with the speech model. */
function SpeakButton({ text }: { text: string }) {
  const [state, setState] = useState<'idle' | 'loading' | 'playing'>('idle');
  const audio = useRef<HTMLAudioElement | null>(null);
  async function play() {
    if (state === 'playing') {
      audio.current?.pause();
      setState('idle');
      return;
    }
    setState('loading');
    try {
      const r = await api.speak({ text: text.slice(0, 4000) });
      const url = await blobUrl(r.blobs[0].id);
      const a = new Audio(url);
      audio.current = a;
      a.onended = () => setState('idle');
      await a.play();
      setState('playing');
    } catch (e) {
      toast.error((e as Error).message);
      setState('idle');
    }
  }
  return (
    <button
      type="button"
      className="icon-btn speak-btn"
      onClick={play}
      aria-label={state === 'playing' ? 'Stop reading aloud' : 'Read aloud'}
      title={state === 'playing' ? 'Stop' : 'Read aloud'}
      data-state={state}
    >
      {state === 'playing' ? <IconStop /> : <IconSpeaker />}
    </button>
  );
}

function TranscriptItem({
  item,
  onDecide,
  canSpeak,
  dev,
}: {
  item: Item;
  onDecide: (runId: string | undefined, id: string, approved: boolean, name?: string, always?: boolean) => void;
  canSpeak: boolean;
  dev: boolean;
}) {
  switch (item.kind) {
    case 'user':
      return (
        <div className="msg user">
          {item.text}
          {item.media?.map((b, i) => (
            <MediaView key={i} block={b} />
          ))}
          {!!item.files?.length && <div className="small muted">📎 {item.files.join(', ')}</div>}
        </div>
      );
    case 'assistant':
      return (
        <div className="msg assistant">
          <Markdown text={item.text} />
          {canSpeak && item.text.trim() && <SpeakButton text={item.text} />}
        </div>
      );
    case 'reasoning':
      return (
        <div className="reasoning">
          <Disclosure summary={<span className="muted">Thinking</span>}>
            <p className="muted pre">{item.text}</p>
          </Disclosure>
        </div>
      );
    case 'note':
      return (
        <div className="note" data-tone={item.tone}>
          {item.text}
          {item.action && (
            <>
              {' '}
              <button type="button" className="link" onClick={() => navigate(item.action!.to)}>
                {item.action.label} →
              </button>
            </>
          )}
        </div>
      );
    case 'tool': {
      const state = item.approval === 'pending' ? 'pending' : !item.result ? 'running' : item.result.ok ? 'ok' : 'bad';
      const sentence = toolSentence(item.name, item.input);
      return (
        <div className="tool" data-state={state}>
          <Disclosure
            summary={
              <span className="tool-head">
                <span className="tool-dot" />
                <span className="tool-name" title={item.name}>
                  {sentence}
                </span>
                {state === 'running' && <span className="muted small">working…</span>}
                {state === 'bad' && <span className="small bad-text">didn't work</span>}
                {dev && <span className="muted mono small">{item.name}</span>}
                {dev && item.result && item.result.ms > 0 && <span className="muted small">{item.result.ms} ms</span>}
              </span>
            }
          >
            <div className="tool-io">
              {dev && (
                <div>
                  <div className="label">Input</div>
                  <Json value={item.input} />
                </div>
              )}
              {item.result ? (
                <div>
                  <div className="label">{item.result.ok ? 'Result' : 'What went wrong'}</div>
                  <Json value={item.result.text} />
                </div>
              ) : (
                !dev && <p className="muted small">No result yet.</p>
              )}
            </div>
          </Disclosure>
          {!!item.result?.media?.length && (
            <div className="media-row">
              {item.result.media.map((b, i) => (
                <MediaView key={i} block={b} />
              ))}
            </div>
          )}
          {item.approval === 'pending' && (
            <div className="approval">
              <span>
                OK to <strong>{sentence.charAt(0).toLowerCase() + sentence.slice(1)}</strong>?
              </span>
              <div className="row gap wrap">
                <Button size="sm" tone="accent" leading={<IconCheck />} onClick={() => onDecide(item.runId, item.id, true)}>
                  Allow once
                </Button>
                <Button size="sm" variant="soft" onClick={() => onDecide(item.runId, item.id, true, item.name, true)}>
                  Always allow “{toolLabel(item.name)}” in this chat
                </Button>
                <Button size="sm" variant="outline" leading={<IconX />} onClick={() => onDecide(item.runId, item.id, false)}>
                  Don't allow
                </Button>
              </div>
            </div>
          )}
          {item.approval === 'declined' && <div className="muted small approval-note">Not allowed</div>}
        </div>
      );
    }
  }
}
