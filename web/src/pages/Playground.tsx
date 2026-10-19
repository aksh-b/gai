import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircleIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SwitchField } from '@/components/fields';
import { PickerMenu, type PickerOption } from '@/components/picker-menu';
import { Json, Markdown } from '@/components/shared';
import { Alert, AlertAction, AlertDescription } from '@/components/ui/alert';
import {
  Attachment,
  AttachmentAction,
  AttachmentActions,
  AttachmentContent,
  AttachmentDescription,
  AttachmentGroup,
  AttachmentMedia,
  AttachmentTitle,
} from '@/components/ui/attachment';
import { Badge } from '@/components/ui/badge';
import { Bubble, BubbleContent } from '@/components/ui/bubble';
import { Button } from '@/components/ui/button';
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupTextarea } from '@/components/ui/input-group';
import { Item as ListItem, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from '@/components/ui/item';
import { Kbd } from '@/components/ui/kbd';
import { Marker, MarkerContent } from '@/components/ui/marker';
import { Message, MessageContent, MessageFooter } from '@/components/ui/message';
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from '@/components/ui/message-scroller';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Spinner } from '@/components/ui/spinner';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from '@/components/ui/trace';
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
import {
  IconBrain,
  IconCheck,
  IconClip,
  IconHand,
  IconMic,
  IconPlus,
  IconSend,
  IconSpark,
  IconSpeaker,
  IconStop,
  IconTool,
  IconTrash,
  IconUpload,
  IconX,
} from '../icons';
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
interface PendingFile {
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
        if (t && t.kind === 'tool') t.result = { ok: !b.is_error, text: blocksText(b.content), ms: 0, media: mediaBlocks(b.content) };
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
  if (
    l.includes('exceeds the available context') ||
    l.includes('context length') ||
    l.includes('context window') ||
    l.includes('too many tokens')
  )
    return {
      text: "This doesn't fit in the model's memory: the conversation plus the assistant's tools are too long. Start a new chat, give the assistant fewer tools, or pick a model with more room.",
      action: { label: 'Models', to: '/settings' },
    };
  if (l.includes('does not support tools'))
    return {
      text: `${error}. Pick a model that can use tools, or an assistant without tools.`,
      action: { label: 'Models', to: '/settings' },
    };
  if (l.includes('connect') && (l.includes('refused') || l.includes('error sending')))
    return { text: `Could not reach the model's server. ${error}`, action: { label: 'Check models', to: '/settings' } };
  return { text: error };
}

type TraceItem = Extract<Item, { kind: 'tool' | 'reasoning' }>;
type Block = { kind: 'trace'; key: string; items: TraceItem[] } | { kind: 'item'; key: string; item: Item };

/**
 * Consecutive thinking and tool calls share one Trace; a text turn, a note
 * or a call waiting for an OK ends the run.
 */
function groupItems(items: Item[]): Block[] {
  const out: Block[] = [];
  items.forEach((item, i) => {
    const traced = item.kind === 'reasoning' || (item.kind === 'tool' && item.approval !== 'pending');
    const last = out[out.length - 1];
    if (traced && last?.kind === 'trace') last.items.push(item as TraceItem);
    else if (traced) out.push({ kind: 'trace', key: `t${i}`, items: [item as TraceItem] });
    else out.push({ kind: 'item', key: `i${i}`, item });
  });
  return out;
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
  const [files, setFiles] = useState<PendingFile[]>([]);
  const [dragging, setDragging] = useState(false);
  const [running, setRunning] = useState<{ runId?: string; abort: AbortController } | null>(null);
  const [usage, setUsage] = useState<{ usage: Usage; cost: number }>({ usage: ZERO, cost: 0 });
  const [media, setMedia] = useState<MediaDefaults | null>(null);
  const recorder = useRecorder();
  const [transcribing, setTranscribing] = useState(false);
  const filePicker = useRef<HTMLInputElement>(null);
  const sessionRef = useRef<string | null>(sessionParam);
  // Tools the person said may run without asking, for this chat.
  const allowed = useRef(new Set<string>());

  const loadSessions = useCallback(
    () =>
      api
        .sessions()
        .then(setSessions)
        .catch(() => {}),
    [],
  );

  useEffect(() => {
    api
      .agents()
      .then((a) => setAgents(a.map((x) => ({ name: x.summary.name, spec: x.spec }))))
      .catch(() => {});
    api
      .defaultAgent()
      .then(setDefaultSpec)
      .catch(() => {});
    api
      .mediaDefaults()
      .then(setMedia)
      .catch(() => {});
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
      .then((m) => setItems(itemsFromHistory(m)))
      .catch((e: Error) => toast.error(e.message));
  }, [sessionParam]);

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

  const agentOptions: PickerOption[] = [
    { value: '', label: 'General assistant', description: 'Built in: files, web pages, calculator' },
    ...agents.map((a) => ({ value: a.name, label: a.name, description: a.spec?.description ?? '', group: 'Your assistants' })),
  ];
  const defaultLabel = activeSpec?.model
    ? `${agent || 'Assistant'}'s model (${dev ? activeSpec.model : modelLabel(activeSpec.model)})`
    : `Default (${dev ? (groups.defaultModel ?? 'none') : modelLabel(groups.defaultModel)})`;
  const options: PickerOption[] = [
    { value: '', label: defaultLabel, description: 'Change the default in Settings' },
    ...modelOptions(groups, dev),
  ];
  const nothingConnected = groups.loaded && !groups.cloud.length && !groups.local.length;
  const suggestions = canUseTools && (activeSpec?.tools?.length ?? 0) > 0 ? SUGGESTIONS_TOOLS : SUGGESTIONS_CHAT;
  const blocks = groupItems(items);

  return (
    <div className="grid h-svh min-h-0 md:grid-cols-[16rem_minmax(0,1fr)]">
      <aside className="hidden min-h-0 flex-col gap-3 border-r p-3 md:flex">
        <Button
          variant="secondary"
          onClick={() => {
            sessionRef.current = null;
            setItems([]);
            navigate(`/chat${agent ? `?agent=${encodeURIComponent(agent)}` : ''}`);
          }}
        >
          <IconPlus data-icon="inline-start" />
          New chat
        </Button>
        <ScrollArea className="min-h-0 flex-1 [&_[data-slot=scroll-area-viewport]>div]:block!">
          <ItemGroup className="gap-1">
            {sessions.map((s) => (
              <ListItem key={s.id} size="xs" variant={s.id === sessionParam ? 'muted' : 'default'} className="group/session relative">
                <ItemContent className="min-w-0">
                  <ItemTitle className="w-full">
                    <a href={`#/chat?session=${s.id}`} className="truncate after:absolute after:inset-0">
                      {s.title || 'Untitled'}
                    </a>
                  </ItemTitle>
                  <ItemDescription>
                    {s.agent ?? 'assistant'} · {ago(s.updated_at)}
                  </ItemDescription>
                </ItemContent>
                <ItemActions className="relative opacity-0 group-focus-within/session:opacity-100 group-hover/session:opacity-100">
                  <Button variant="ghost" size="icon-xs" aria-label="Delete chat" onClick={() => removeSession(s.id)}>
                    <IconTrash />
                  </Button>
                </ItemActions>
              </ListItem>
            ))}
          </ItemGroup>
          {sessions.length === 0 && <p className="p-2 text-sm text-muted-foreground">Your chats will show up here.</p>}
        </ScrollArea>
      </aside>

      <section
        className="relative grid min-h-0 grid-rows-[auto_minmax(0,1fr)_auto]"
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
        <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2">
          <PickerMenu aria-label="Assistant" options={agentOptions} value={agent} onValueChange={setAgent} className="max-w-48" />
          <PickerMenu aria-label="Model" options={options} value={model} onValueChange={setModel} className="max-w-72" />
          <Button variant="link" size="sm" asChild>
            <a href="#/settings">Connect models</a>
          </Button>
          <div className="flex-1" />
          <Tooltip>
            <TooltipTrigger asChild>
              <div>
                <SwitchField
                  size="sm"
                  label="Act without asking"
                  checked={autoApprove}
                  onCheckedChange={(checked) => setAutoApprove(checked)}
                />
              </div>
            </TooltipTrigger>
            <TooltipContent>When off, it asks before doing anything that needs your OK</TooltipContent>
          </Tooltip>
          {dev ? (
            <Badge variant="outline" className="font-mono" title="Tokens this conversation (since opened)">
              {tokens(totalIn(usage.usage))} in · {tokens(usage.usage.output_tokens)} out
              {usage.cost > 0 && ` · ${money(usage.cost)}`}
            </Badge>
          ) : (
            usage.cost > 0 && (
              <Badge variant="outline" title="What this chat has cost so far">
                {money(usage.cost)}
              </Badge>
            )
          )}
        </div>

        {items.length === 0 ? (
          <Empty className="self-start pt-[10vh]">
            <EmptyHeader className="max-w-xl">
              <EmptyMedia variant="icon">
                <IconSpark />
              </EmptyMedia>
              <EmptyTitle>How can {agent || 'I'} help?</EmptyTitle>
              <EmptyDescription>
                {effectiveModel ? (
                  <>Using {dev ? <code>{effectiveModel}</code> : modelLabel(effectiveModel)}.</>
                ) : (
                  'No AI model is connected yet.'
                )}{' '}
                {!canUseTools && 'This model can chat but not use tools. '}
                You can also drop files here.
              </EmptyDescription>
            </EmptyHeader>
            <EmptyContent className="max-w-xl">
              {nothingConnected && (
                <Alert>
                  <IconSpark />
                  <AlertDescription>
                    {effectiveModel.startsWith('mock/')
                      ? 'You are using the demo model: it repeats what you say.'
                      : 'Connect an AI model to get real answers.'}
                  </AlertDescription>
                  <AlertAction>
                    <Button size="sm" onClick={() => navigate('/welcome')}>
                      Connect a model
                    </Button>
                  </AlertAction>
                </Alert>
              )}
              <div className="flex w-full flex-col gap-2">
                {suggestions.map((s) => (
                  <Button
                    key={s}
                    variant="outline"
                    className="h-auto justify-start py-2.5 text-left whitespace-normal"
                    onClick={() => send(s)}
                  >
                    {s}
                  </Button>
                ))}
              </div>
            </EmptyContent>
          </Empty>
        ) : (
          <MessageScrollerProvider autoScroll>
            <MessageScroller>
              <MessageScrollerViewport>
                <MessageScrollerContent className="mx-auto w-full max-w-3xl px-4 py-6">
                  {blocks.map((b) => (
                    <MessageScrollerItem key={b.key} messageId={b.key} scrollAnchor={b.kind === 'item' && b.item.kind === 'user'}>
                      {b.kind === 'trace' ? (
                        <Trace>
                          {b.items.map((it, i) =>
                            it.kind === 'tool' ? <ToolLine key={i} item={it} dev={dev} /> : <ThoughtLine key={i} text={it.text} />,
                          )}
                        </Trace>
                      ) : (
                        <TranscriptItem item={b.item} onDecide={decide} canSpeak={!!media?.speech} />
                      )}
                    </MessageScrollerItem>
                  ))}
                  {running && items[items.length - 1]?.kind === 'user' && (
                    <MessageScrollerItem messageId="working">
                      <TraceLine status="running">
                        <TraceTitle>Working</TraceTitle>
                      </TraceLine>
                    </MessageScrollerItem>
                  )}
                  {!running && items.length > 1 && (
                    <MessageScrollerItem messageId="save-setup">
                      <Button variant="link" size="sm" className="self-start px-0" onClick={saveAsAssistant}>
                        Save this setup as an assistant
                      </Button>
                    </MessageScrollerItem>
                  )}
                </MessageScrollerContent>
              </MessageScrollerViewport>
              <MessageScrollerButton />
            </MessageScroller>
          </MessageScrollerProvider>
        )}

        <form
          className="mx-auto w-full max-w-3xl px-4 pt-2 pb-4"
          onSubmit={(e) => {
            e.preventDefault();
            send();
          }}
        >
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
          <InputGroup>
            {files.length > 0 && (
              <InputGroupAddon align="block-start">
                <AttachmentGroup>
                  {files.map((f, i) => (
                    <Attachment key={i} size="xs" state={f.text === '' ? 'processing' : 'done'}>
                      <AttachmentMedia variant="icon">
                        <IconClip />
                      </AttachmentMedia>
                      <AttachmentContent>
                        <AttachmentTitle>{f.name}</AttachmentTitle>
                        {f.knowledge && <AttachmentDescription>In your documents</AttachmentDescription>}
                      </AttachmentContent>
                      <AttachmentActions>
                        <AttachmentAction aria-label={`Remove ${f.name}`} onClick={() => setFiles((x) => x.filter((_, j) => j !== i))}>
                          <IconX />
                        </AttachmentAction>
                      </AttachmentActions>
                    </Attachment>
                  ))}
                </AttachmentGroup>
              </InputGroupAddon>
            )}
            <InputGroupTextarea
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
            <InputGroupAddon align="block-end">
              <Tooltip>
                <TooltipTrigger asChild>
                  <InputGroupButton size="icon-sm" type="button" aria-label="Attach files" onClick={() => filePicker.current?.click()}>
                    <IconClip />
                  </InputGroupButton>
                </TooltipTrigger>
                <TooltipContent>Attach files (or drop them here)</TooltipContent>
              </Tooltip>
              {media?.transcription && recorder.supported && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <InputGroupButton
                      type="button"
                      variant={recorder.recording ? 'destructive' : 'ghost'}
                      size={recorder.recording ? 'sm' : 'icon-sm'}
                      onClick={toggleMic}
                      aria-label={recorder.recording ? 'Stop recording' : 'Record a message'}
                      disabled={transcribing}
                    >
                      {transcribing ? <Spinner data-icon="inline-start" /> : <IconMic data-icon="inline-start" />}
                      {recorder.recording ? `${recorder.seconds}s` : undefined}
                    </InputGroupButton>
                  </TooltipTrigger>
                  <TooltipContent>{recorder.recording ? 'Stop and turn into text' : 'Speak your message'}</TooltipContent>
                </Tooltip>
              )}
              {running ? (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <InputGroupButton variant="destructive" size="sm" type="button" className="ml-auto" onClick={stop}>
                      <IconStop data-icon="inline-start" />
                      Stop
                    </InputGroupButton>
                  </TooltipTrigger>
                  <TooltipContent>
                    Stop <Kbd>Esc</Kbd>
                  </TooltipContent>
                </Tooltip>
              ) : (
                <InputGroupButton variant="default" size="sm" type="submit" className="ml-auto" disabled={!input.trim() && !files.length}>
                  <IconSend data-icon="inline-start" />
                  Send
                </InputGroupButton>
              )}
            </InputGroupAddon>
          </InputGroup>
        </form>

        {dragging && (
          <Empty className="pointer-events-none absolute inset-2 border bg-background/90" aria-hidden="true">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <IconUpload />
              </EmptyMedia>
              <EmptyTitle>Drop files to add them to your message</EmptyTitle>
            </EmptyHeader>
          </Empty>
        )}
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
    <Button
      variant="ghost"
      size="icon-xs"
      onClick={play}
      disabled={state === 'loading'}
      aria-label={state === 'playing' ? 'Stop reading aloud' : 'Read aloud'}
      title={state === 'playing' ? 'Stop' : 'Read aloud'}
    >
      {state === 'loading' ? <Spinner /> : state === 'playing' ? <IconStop /> : <IconSpeaker />}
    </Button>
  );
}

function ThoughtLine({ text }: { text: string }) {
  return (
    <TraceCollapsible>
      <TraceCollapsibleTrigger>
        <TraceLine>
          <TraceIcon>
            <IconBrain />
          </TraceIcon>
          <TraceTitle>Thought</TraceTitle>
        </TraceLine>
      </TraceCollapsibleTrigger>
      <TraceCollapsibleContent>
        <p className="whitespace-pre-wrap text-muted-foreground">{text}</p>
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}

/** One tool call: a line while it runs, expandable once there is a result. */
function ToolLine({ item, dev }: { item: Extract<Item, { kind: 'tool' }>; dev: boolean }) {
  const sentence = toolSentence(item.name, item.input);
  const declined = item.approval === 'declined';
  const status = !item.result && !declined ? 'running' : declined || !item.result?.ok ? 'error' : undefined;
  const line = (
    <TraceLine status={status}>
      <TraceIcon>
        <IconTool />
      </TraceIcon>
      <TraceTitle title={item.name}>{sentence}</TraceTitle>
      {declined && <TraceDescription>not allowed</TraceDescription>}
      {!declined && status === 'error' && <TraceDescription>didn't work</TraceDescription>}
      {dev && <TraceDescription className="font-mono">{item.name}</TraceDescription>}
      {dev && item.result && item.result.ms > 0 && <TraceDescription>{item.result.ms} ms</TraceDescription>}
    </TraceLine>
  );
  const media = item.result?.media ?? [];
  if (!item.result) return line;
  return (
    <>
      <TraceCollapsible>
        <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
        <TraceCollapsibleContent className="flex flex-col gap-2">
          {dev && <Json value={item.input} />}
          <Json value={item.result.text} />
        </TraceCollapsibleContent>
      </TraceCollapsible>
      {media.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {media.map((b, i) => (
            <MediaView key={i} block={b} />
          ))}
        </div>
      )}
    </>
  );
}

function TranscriptItem({
  item,
  onDecide,
  canSpeak,
}: {
  item: Item;
  onDecide: (runId: string | undefined, id: string, approved: boolean, name?: string, always?: boolean) => void;
  canSpeak: boolean;
}) {
  switch (item.kind) {
    case 'user':
      return (
        <Message align="end">
          <MessageContent>
            {item.text && (
              <Bubble variant="secondary" align="end">
                <BubbleContent className="whitespace-pre-wrap">{item.text}</BubbleContent>
              </Bubble>
            )}
            {item.media?.map((b, i) => (
              <MediaView key={i} block={b} />
            ))}
            {!!item.files?.length && (
              <AttachmentGroup>
                {item.files.map((f) => (
                  <Attachment key={f} size="xs">
                    <AttachmentMedia variant="icon">
                      <IconClip />
                    </AttachmentMedia>
                    <AttachmentContent>
                      <AttachmentTitle>{f}</AttachmentTitle>
                    </AttachmentContent>
                  </Attachment>
                ))}
              </AttachmentGroup>
            )}
          </MessageContent>
        </Message>
      );
    case 'assistant':
      return (
        <Message align="start">
          <MessageContent>
            <Markdown text={item.text} />
            {canSpeak && item.text.trim() && (
              <MessageFooter>
                <SpeakButton text={item.text} />
              </MessageFooter>
            )}
          </MessageContent>
        </Message>
      );
    case 'note':
      if (item.tone === 'muted')
        return (
          <Marker variant="separator">
            <MarkerContent>{item.text}</MarkerContent>
          </Marker>
        );
      return (
        <Alert variant={item.tone === 'bad' ? 'destructive' : 'default'}>
          <AlertCircleIcon />
          <AlertDescription>{item.text}</AlertDescription>
          {item.action && (
            <AlertAction>
              <Button size="sm" variant="outline" onClick={() => navigate(item.action!.to)}>
                {item.action.label}
              </Button>
            </AlertAction>
          )}
        </Alert>
      );
    case 'reasoning':
      return (
        <Trace>
          <ThoughtLine text={item.text} />
        </Trace>
      );
    case 'tool': {
      // A call waiting for an OK is a prompt, not a trace line.
      const sentence = toolSentence(item.name, item.input);
      return (
        <ListItem variant="outline">
          <ItemMedia variant="icon">
            <IconHand />
          </ItemMedia>
          <ItemContent>
            <ItemTitle>OK to {sentence.charAt(0).toLowerCase() + sentence.slice(1)}?</ItemTitle>
            <ItemDescription>The assistant waits until you decide.</ItemDescription>
          </ItemContent>
          <ItemActions className="flex-wrap">
            <Button size="sm" onClick={() => onDecide(item.runId, item.id, true)}>
              <IconCheck data-icon="inline-start" />
              Allow once
            </Button>
            <Button variant="secondary" size="sm" onClick={() => onDecide(item.runId, item.id, true, item.name, true)}>
              Always allow “{toolLabel(item.name)}” here
            </Button>
            <Button variant="outline" size="sm" onClick={() => onDecide(item.runId, item.id, false)}>
              <IconX data-icon="inline-start" />
              Don't allow
            </Button>
          </ItemActions>
        </ListItem>
      );
    }
  }
}
