import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Button, Select, Switch, Tooltip, toast } from '@flame';
import {
  api,
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
import { IconCheck, IconMic, IconPlus, IconSend, IconSpark, IconSpeaker, IconStop, IconTrash, IconX } from '../icons';
import { blobToBase64, blobUrl, MediaView, mediaBlocks, useRecorder } from '../Media';
import { navigate, type Route } from '../router';
import { ago, blocksText, compactJson, money, tokens, totalIn } from '../util';

type Item =
  | { kind: 'user'; text: string; media?: ContentBlock[] }
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
  | { kind: 'note'; text: string; tone: 'muted' | 'warn' | 'bad' };

const ZERO: Usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, reasoning_tokens: 0 };

const SUGGESTIONS = [
  'What is 17% of 2,340, and what weekday is it today?',
  'Fetch https://example.com and summarize the page in one sentence.',
  'Write a short haiku about Rust into poem.txt, then read it back.',
];

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

export function Playground({ route }: { route: Route }) {
  const sessionParam = route.query.get('session');
  const agentParam = route.query.get('agent') ?? '';

  const [agents, setAgents] = useState<{ name: string; spec: AgentSpec | null }[]>([]);
  const [defaultSpec, setDefaultSpec] = useState<AgentSpec | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [defaultModel, setDefaultModel] = useState<string | null>(null);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [agent, setAgent] = useState(agentParam);
  const [model, setModel] = useState('');
  const [autoApprove, setAutoApprove] = useState(false);
  const [items, setItems] = useState<Item[]>([]);
  const [input, setInput] = useState('');
  const [running, setRunning] = useState<{ runId?: string; abort: AbortController } | null>(null);
  const [usage, setUsage] = useState<{ usage: Usage; cost: number }>({ usage: ZERO, cost: 0 });
  const [media, setMedia] = useState<MediaDefaults | null>(null);
  const recorder = useRecorder();
  const [transcribing, setTranscribing] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const sessionRef = useRef<string | null>(sessionParam);

  const loadSessions = useCallback(() => api.sessions().then(setSessions).catch(() => {}), []);

  useEffect(() => {
    api.agents().then((a) => setAgents(a.map((x) => ({ name: x.summary.name, spec: x.spec })))).catch(() => {});
    api.defaultAgent().then(setDefaultSpec).catch(() => {});
    api
      .models()
      .then((m) => setModels(m.filter((x) => x.kind === 'chat' && x.key.status !== 'missing').map((x) => x.id)))
      .catch(() => {});
    api.health().then((h) => setDefaultModel(h.default_model)).catch(() => {});
    api.mediaDefaults().then(setMedia).catch(() => {});
    loadSessions();
  }, [loadSessions]);

  // Load the transcript when the session in the URL changes.
  useEffect(() => {
    if (sessionParam === sessionRef.current && items.length) return;
    sessionRef.current = sessionParam;
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
  const effectiveModel = model || activeSpec?.model || defaultModel || '';

  const update = (fn: (items: Item[]) => Item[]) => setItems((prev) => fn([...prev]));

  const onEvent = useCallback((e: EventEnvelope) => {
    switch (e.type) {
      case 'run_started':
        setRunning((r) => (r ? { ...r, runId: e.run_id } : r));
        if (sessionRef.current !== e.session_id) {
          sessionRef.current = e.session_id;
          navigate(`/playground?session=${e.session_id}${agent ? `&agent=${agent}` : ''}`);
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
        update((it) => [...it, { kind: 'note', text: `Routed to ${e.model}: ${e.reason}`, tone: 'muted' }]);
        break;
      case 'compacted':
        update((it) => [...it, { kind: 'note', text: `Summarized ${e.summarized_messages} earlier messages to free context.`, tone: 'muted' }]);
        break;
      case 'run_finished':
        if (e.status !== 'completed') {
          update((it) => [
            ...it,
            {
              kind: 'note',
              text: `Run ${e.status.replace('_', ' ')}${e.error ? `: ${e.error}` : '.'}`,
              tone: e.status === 'cancelled' ? 'warn' : 'bad',
            },
          ]);
        }
        break;
    }
  }, [agent, loadSessions]);

  async function send(text?: string) {
    const message = (text ?? input).trim();
    if (!message || running) return;
    setInput('');
    stick.current = true;
    update((it) => [...it, { kind: 'user', text: message }]);
    const abort = new AbortController();
    setRunning({ abort });
    try {
      await streamRun(
        {
          agent: agent || undefined,
          input: message,
          session_id: sessionRef.current ?? undefined,
          model: model || undefined,
          auto_approve: autoApprove,
        },
        onEvent,
        abort.signal,
      );
    } catch (e) {
      if ((e as Error).name !== 'AbortError') {
        update((it) => [...it, { kind: 'note', text: (e as Error).message, tone: 'bad' }]);
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

  async function decide(runId: string | undefined, callId: string, approved: boolean) {
    if (!runId) return;
    try {
      await api.decide(runId, callId, approved, approved ? undefined : 'declined in the dashboard');
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  async function removeSession(id: string) {
    await api.deleteSession(id).catch((e: Error) => toast.error(e.message));
    if (sessionRef.current === id) navigate('/playground');
    loadSessions();
  }

  const agentOptions = [
    { value: '', label: 'assistant', description: 'Built-in general assistant' },
    ...agents.map((a) => ({ value: a.name, label: a.name, description: a.spec?.description ?? a.spec?.model ?? '' })),
  ];
  const modelOptions = [
    { value: '', label: activeSpec?.model ? `Agent default (${activeSpec.model})` : `Default (${defaultModel ?? 'none'})` },
    ...[...new Set([...models, 'mock/echo'])].map((m) => ({ value: m, label: m })),
  ];

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
            navigate(`/playground${agent ? `?agent=${agent}` : ''}`);
          }}
        >
          New chat
        </Button>
        <div className="session-list">
          {sessions.map((s) => (
            <div key={s.id} className="session" aria-current={s.id === sessionParam ? 'true' : undefined}>
              <a href={`#/playground?session=${s.id}`} className="session-link">
                <span className="session-title">{s.title || 'Untitled'}</span>
                <span className="muted small">
                  {s.agent ?? 'assistant'} · {ago(s.updated_at)}
                </span>
              </a>
              <button type="button" className="icon-btn" aria-label="Delete session" onClick={() => removeSession(s.id)}>
                <IconTrash />
              </button>
            </div>
          ))}
          {sessions.length === 0 && <p className="muted small pad">No conversations yet.</p>}
        </div>
      </aside>

      <section className="conversation">
        <div className="toolbar">
          <div className="toolbar-field">
            <Select aria-label="Agent" options={agentOptions} value={agent} onValueChange={setAgent} />
          </div>
          <div className="toolbar-field wide">
            <Select aria-label="Model" options={modelOptions} value={model} onValueChange={setModel} />
          </div>
          <Switch size="sm" label="Auto-approve tools" checked={autoApprove} onChange={(e) => setAutoApprove(e.target.checked)} />
          <div className="spacer" />
          <span className="chip mono" title="Tokens this conversation (since opened)">
            {tokens(totalIn(usage.usage))} in · {tokens(usage.usage.output_tokens)} out
            {usage.cost > 0 && ` · ${money(usage.cost)}`}
          </span>
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
              <h2>What should {agent || 'the assistant'} do?</h2>
              <p className="muted">
                Using <span className="mono">{effectiveModel || 'no model configured'}</span>
                {activeSpec?.tools?.length ? ` with ${activeSpec.tools.join(', ')}` : ''}.
              </p>
              <div className="suggestions">
                {SUGGESTIONS.map((s) => (
                  <button key={s} type="button" className="suggestion" onClick={() => send(s)}>
                    {s}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <div className="thread">
              {items.map((it, i) => (
                <TranscriptItem key={i} item={it} onDecide={decide} canSpeak={!!media?.speech} />
              ))}
              {running && items[items.length - 1]?.kind === 'user' && <div className="typing" aria-label="Working" />}
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
          <textarea
            value={input}
            placeholder={running ? 'Working… you can type your next message' : 'Message the agent  (Enter to send, Shift+Enter for a new line)'}
            rows={Math.min(8, Math.max(1, input.split('\n').length))}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
            autoFocus
          />
          {media?.transcription && recorder.supported && (
            <Tooltip content={recorder.recording ? 'Stop and transcribe' : `Dictate (${media.transcription})`}>
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
            <Tooltip content="Stop the run" shortcut="Esc">
              <Button type="button" tone="danger" variant="soft" onClick={stop} leading={<IconStop />}>
                Stop
              </Button>
            </Tooltip>
          ) : (
            <Button type="submit" tone="accent" disabled={!input.trim()} leading={<IconSend />}>
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
}: {
  item: Item;
  onDecide: (runId: string | undefined, id: string, approved: boolean) => void;
  canSpeak: boolean;
}) {
  switch (item.kind) {
    case 'user':
      return (
        <div className="msg user">
          {item.text}
          {item.media?.map((b, i) => (
            <MediaView key={i} block={b} />
          ))}
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
          <Disclosure summary={<span className="muted">Reasoning</span>}>
            <p className="muted pre">{item.text}</p>
          </Disclosure>
        </div>
      );
    case 'note':
      return (
        <div className="note" data-tone={item.tone}>
          {item.text}
        </div>
      );
    case 'tool': {
      const state = item.approval === 'pending' ? 'pending' : !item.result ? 'running' : item.result.ok ? 'ok' : 'bad';
      return (
        <div className="tool" data-state={state}>
          <Disclosure
            summary={
              <span className="tool-head">
                <span className="tool-dot" />
                <span className="mono tool-name">{item.name}</span>
                <span className="muted mono tool-args">{compactJson(item.input, 90)}</span>
                {item.result && item.result.ms > 0 && <span className="muted small">{item.result.ms} ms</span>}
              </span>
            }
          >
            <div className="tool-io">
              <div>
                <div className="label">Input</div>
                <Json value={item.input} />
              </div>
              {item.result && (
                <div>
                  <div className="label">{item.result.ok ? 'Result' : 'Error'}</div>
                  <Json value={item.result.text} />
                </div>
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
                <strong>{item.name}</strong> needs your approval.
              </span>
              <div className="row gap">
                <Button size="sm" tone="accent" leading={<IconCheck />} onClick={() => onDecide(item.runId, item.id, true)}>
                  Approve
                </Button>
                <Button size="sm" variant="outline" leading={<IconX />} onClick={() => onDecide(item.runId, item.id, false)}>
                  Decline
                </Button>
              </div>
            </div>
          )}
          {item.approval === 'declined' && <div className="muted small approval-note">Declined</div>}
        </div>
      );
    }
  }
}
