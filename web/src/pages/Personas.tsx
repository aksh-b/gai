import { useEffect, useRef, useState } from 'react';
import { HistoryIcon, SmileIcon } from 'lucide-react';
import { toast } from 'sonner';
import YAML from 'yaml';
import { SelectField } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, LinkItem, Markdown, Page, PageHeader } from '@/components/shared';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Bubble, BubbleContent } from '@/components/ui/bubble';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldDescription } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupTextarea } from '@/components/ui/input-group';
import { Item, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from '@/components/ui/item';
import { Message, MessageAvatar, MessageContent } from '@/components/ui/message';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet';
import { Spinner } from '@/components/ui/spinner';
import { api, streamRun, type AgentSpec, type PersonaSpec, type PersonaVersion, type Placement, type Template } from '../api';
import { SpecEditor } from '../forms';
import { IconChat, IconPlus, IconSend, IconSpark, IconTrash } from '../icons';
import { PersonaAvatar, personaName } from '../Persona';
import { PersonaForm } from '../PersonaForm';
import { useDevMode } from '../prefs';
import { navigate } from '../router';
import { useLoad } from '../util';
import { VersionHistory } from '../Versions';

const BLANK: PersonaSpec = { name: '' };

/** A drafted persona handed to the new-persona editor. */
const DRAFT_KEY = 'gai.personaDraft';

function stashDraft(spec: PersonaSpec) {
  try {
    sessionStorage.setItem(DRAFT_KEY, JSON.stringify(spec));
  } catch {
    /* the editor starts blank instead */
  }
}

function takeDraft(): PersonaSpec | null {
  try {
    const raw = sessionStorage.getItem(DRAFT_KEY);
    sessionStorage.removeItem(DRAFT_KEY);
    return raw ? (JSON.parse(raw) as PersonaSpec) : null;
  } catch {
    return null;
  }
}

export function Personas({ selected, creating = false }: { selected?: string; creating?: boolean }) {
  const [dev] = useDevMode();
  const list = useLoad(() => api.personas(), []);
  const [spec, setSpec] = useState<PersonaSpec>(BLANK);
  const [loaded, setLoaded] = useState('');
  const [note, setNote] = useState('');
  const [error, setError] = useState<string>();
  const [saving, setSaving] = useState(false);
  const [versions, setVersions] = useState<PersonaVersion[]>([]);
  const [viewing, setViewing] = useState<number>();
  const [picking, setPicking] = useState(creating);
  const [trying, setTrying] = useState(false);

  const current = list.data?.find((p) => p.summary.name === selected);

  useEffect(() => {
    if (creating) setPicking(true);
  }, [creating]);

  useEffect(() => {
    if (selected === 'new') {
      const draft = takeDraft();
      const start = draft ?? BLANK;
      setSpec(start);
      setLoaded(draft ? '' : JSON.stringify(start));
      setVersions([]);
      setPicking(false);
    } else if (current?.spec) {
      setSpec(current.spec);
      setLoaded(JSON.stringify(current.spec));
      setViewing(undefined);
      setPicking(false);
      api
        .personaVersions(current.summary.name)
        .then(setVersions)
        .catch(() => setVersions([]));
    }
  }, [selected, current]);

  const dirty = JSON.stringify(spec) !== loaded;

  async function save() {
    if (!spec.name?.trim()) {
      setError('Give the persona a name.');
      return;
    }
    setError(undefined);
    setSaving(true);
    try {
      const r = await api.savePersona(spec, note || undefined);
      toast.success(selected === 'new' ? `Created ${r.name}` : `Saved ${r.name}${dev ? ` v${r.version}` : ''}`);
      setNote('');
      list.reload();
      navigate(`/personas/${encodeURIComponent(r.name)}`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  }

  async function remove(name: string) {
    try {
      await api.deletePersona(name);
      list.reload();
      navigate('/personas');
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const startNew = () => {
    setPicking(true);
    navigate('/personas?new=1');
  };

  return (
    <Page wide>
      <PageHeader
        title="Personas"
        subtitle="Voices your assistants can speak in. Give any assistant a persona, or pick one for a single chat."
        actions={
          <Button onClick={startNew}>
            <IconPlus data-icon="inline-start" />
            New persona
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[19rem_minmax(0,1fr)]">
        <div className="flex flex-col gap-4">
          <DefaultPersona personas={(list.data ?? []).map((p) => p.summary.name)} />
          <ItemGroup className="gap-2">
            {list.data?.map(({ summary, spec, used_by }) => (
              <LinkItem
                key={summary.name}
                href={`#/personas/${encodeURIComponent(summary.name)}`}
                active={summary.name === selected}
                title={
                  <>
                    <span className="flex min-w-0 items-center gap-2">
                      <PersonaAvatar persona={{ name: summary.name, ...spec, avatar: summary.avatar }} className="size-6" />
                      <span className="truncate">{summary.display_name || summary.name}</span>
                    </span>
                    {dev && (
                      <Badge variant="outline" className="ml-auto">
                        v{summary.latest_version}
                      </Badge>
                    )}
                  </>
                }
              >
                {summary.description && <ItemDescription>{summary.description}</ItemDescription>}
                {used_by.length > 0 && (
                  <div className="flex flex-wrap gap-1 pt-1">
                    <Badge variant="secondary">
                      Used by {used_by.length} assistant{used_by.length === 1 ? '' : 's'}
                    </Badge>
                  </div>
                )}
              </LinkItem>
            ))}
            {list.data?.length === 0 && !picking && (
              <EmptyState title="No personas yet" icon={<SmileIcon />} action={<Button onClick={startNew}>Browse templates</Button>}>
                Start from a ready-made voice and change it as you like.
              </EmptyState>
            )}
          </ItemGroup>
        </div>

        {picking && selected !== 'new' ? (
          <TemplatePicker
            onBlank={() => navigate('/personas/new')}
            onInstalled={(name) => {
              list.reload();
              navigate(`/personas/${encodeURIComponent(name)}`);
            }}
            onDraft={(draft) => {
              stashDraft(draft);
              navigate('/personas/new');
            }}
          />
        ) : selected ? (
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-lg">
                {selected !== 'new' && <PersonaAvatar persona={spec} />}
                {selected === 'new' ? 'New persona' : personaName(spec)}
              </CardTitle>
              <CardAction className="flex gap-2">
                <Button variant="outline" size="sm" onClick={() => setTrying(true)}>
                  <IconSpark data-icon="inline-start" />
                  Try it
                </Button>
                {current && (
                  <>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => navigate(`/chat?persona=${encodeURIComponent(current.summary.name)}`)}
                    >
                      <IconChat data-icon="inline-start" />
                      Chat
                    </Button>
                    <ConfirmAction
                      title={`Delete ${current.summary.name}?`}
                      description="Its earlier versions are deleted too. Assistants that use it, or a default set to it, must change first."
                      action="Delete"
                      destructive
                      onConfirm={() => remove(current.summary.name)}
                    >
                      <Button variant="destructive" size="sm">
                        <IconTrash data-icon="inline-start" />
                        Delete
                      </Button>
                    </ConfirmAction>
                  </>
                )}
              </CardAction>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              {viewing !== undefined && current && viewing !== current.summary.latest_version && (
                <Alert>
                  <HistoryIcon />
                  <AlertDescription>
                    Showing an earlier version in the editor. Saving it makes it the current one; Roll back below does the same without edits.
                  </AlertDescription>
                </Alert>
              )}
              {current && current.used_by.length > 0 && (
                <FieldDescription>
                  Used by{' '}
                  {current.used_by.map((a, i) => (
                    <span key={a}>
                      {i > 0 && ', '}
                      <a href={`#/assistants/${encodeURIComponent(a)}`}>{a}</a>
                    </span>
                  ))}
                  . Saving changes how they talk from their next message.
                </FieldDescription>
              )}
              <SpecEditor label="Persona" value={spec} onChange={setSpec}>
                <PersonaForm key={selected} spec={spec} onChange={setSpec} isNew={selected === 'new'} />
              </SpecEditor>
            </CardContent>
            <CardFooter className="sticky bottom-0 flex-col items-stretch gap-3 border-t bg-card">
              <ErrorNote error={error} />
              <div className="flex items-center justify-end gap-2">
                {dirty && <span className="text-sm whitespace-nowrap text-muted-foreground">Unsaved changes</span>}
                {dev && (
                  <Input className="max-w-64" placeholder="Version note (optional)" value={note} onChange={(e) => setNote(e.target.value)} />
                )}
                <Button onClick={save} disabled={saving || (!dirty && selected !== 'new')}>
                  {saving && <Spinner data-icon="inline-start" />}
                  {selected === 'new' ? 'Create persona' : 'Save changes'}
                </Button>
              </div>
            </CardFooter>
            {current && versions.length > 1 && (
              <>
                <Separator />
                <CardHeader>
                  <CardTitle>History</CardTitle>
                  <CardDescription>Every saved version. Open one to load it into the editor.</CardDescription>
                </CardHeader>
                <CardContent>
                  <VersionHistory
                    kind="persona"
                    name={current.summary.name}
                    versions={versions}
                    labels={current.summary.labels}
                    onView={(n) => {
                      const v = versions.find((x) => x.version === n);
                      if (v) {
                        setSpec(v.spec as PersonaSpec);
                        setViewing(n);
                      }
                    }}
                    onChanged={list.reload}
                  />
                </CardContent>
              </>
            )}
            <TryIt persona={spec} open={trying} onOpenChange={setTrying} />
          </Card>
        ) : (
          <Card>
            <CardContent>
              <EmptyState title="Pick a persona">Choose one on the left to change it, or make a new one.</EmptyState>
            </CardContent>
          </Card>
        )}
      </div>
    </Page>
  );
}

const PLACEMENTS = [
  { value: 'prepend', label: 'Before the instructions', description: 'recommended' },
  { value: 'append', label: 'After the instructions', description: 'the voice has the last word' },
];

/** The persona chats use when neither the chat nor its assistant picks one. */
function DefaultPersona({ personas }: { personas: string[] }) {
  const defaults = useLoad(() => api.personaDefaults(), []);
  const d = defaults.data;

  async function choose(persona: string | null, placement: Placement) {
    try {
      await api.setPersonaDefaults({ persona, placement });
      defaults.reload();
      toast.success(persona ? `Chats now speak as ${persona} by default` : 'No default persona');
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle>Default for all chats</CardTitle>
        <CardDescription>Used when neither the chat nor its assistant picks a persona.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <SelectField
          aria-label="Default persona"
          options={[{ value: '', label: 'None' }, ...personas.map((p) => ({ value: p, label: p }))]}
          value={d?.persona ?? ''}
          onValueChange={(v) => choose(v || null, d?.placement ?? 'prepend')}
        />
        <SelectField
          label="Where it goes"
          options={PLACEMENTS}
          value={d?.placement ?? 'prepend'}
          onValueChange={(v) => choose(d?.persona ?? null, v as Placement)}
        />
        {d?.source === 'config' && d.persona && <FieldDescription>Set in gai.toml. Choosing here overrides it.</FieldDescription>}
      </CardContent>
    </Card>
  );
}

const BUILDER: AgentSpec = {
  name: 'persona-builder',
  system: `You design personas (voices) for AI assistants. Reply with only a YAML document in a \`\`\`yaml code block, with these keys:
name (lowercase-with-dashes), display_name, description (one sentence), avatar (one emoji), greeting (one short sentence),
starters (2 suggested first messages), tone (a few words), formality (casual, neutral or formal), verbosity (terse, balanced or detailed),
dos (2-4 short rules), donts (1-3 short rules), examples (one item with user and assistant keys, showing the voice).
Describe only how it talks, never what it can do.`,
  tools: [],
  limits: { max_steps: 2 },
};

/** How to start a new persona: describe it, a template, or blank. */
function TemplatePicker({
  onBlank,
  onInstalled,
  onDraft,
}: {
  onBlank: () => void;
  onInstalled: (name: string) => void;
  onDraft: (spec: PersonaSpec) => void;
}) {
  const templates = useLoad(() => api.templates(), []);
  const [describe, setDescribe] = useState('');
  const [drafting, setDrafting] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  async function install(t: Template) {
    setBusy(t.id);
    try {
      const r = await api.installTemplate(t.id);
      toast.success(`Added ${r.name}`);
      onInstalled(r.name);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function draft() {
    setDrafting(true);
    let text = '';
    let session: string | undefined;
    try {
      // `none`: the builder must not take on the default persona's voice.
      await streamRun({ agent: BUILDER, persona: 'none', input: `Design a persona that: ${describe.trim()}` }, (e) => {
        if (e.type === 'run_started') session = e.session_id;
        if (e.type === 'text_delta') text += e.text;
        if (e.type === 'run_finished' && e.status !== 'completed') throw new Error(e.error ?? `the model ${e.status}`);
      });
      const yaml = /```(?:yaml)?\s*([\s\S]*?)```/.exec(text)?.[1] ?? text;
      const spec = YAML.parse(yaml) as PersonaSpec;
      if (!spec || typeof spec !== 'object' || !spec.name) throw new Error('the model did not return a persona');
      onDraft(spec);
    } catch (e) {
      toast.error(`Could not draft it: ${(e as Error).message}. Try a template, or a model that is better at following instructions.`);
    } finally {
      if (session) api.deleteSession(session).catch(() => {});
      setDrafting(false);
    }
  }

  const personas = (templates.data ?? []).filter((t) => t.kind === 'persona');

  return (
    <Card>
      <CardHeader>
        <CardTitle>New persona</CardTitle>
        <CardDescription>Describe the voice you want, or start from a template. You can change everything afterwards.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (describe.trim()) draft();
          }}
        >
          <InputGroup>
            <InputGroupTextarea
              rows={2}
              aria-label="How the persona should talk"
              placeholder="e.g. A calm, plain-spoken support agent who never uses jargon"
              value={describe}
              onChange={(e) => setDescribe(e.target.value)}
            />
            <InputGroupAddon align="block-end">
              <InputGroupButton variant="default" size="sm" type="submit" className="ml-auto" disabled={drafting || !describe.trim()}>
                {drafting ? <Spinner data-icon="inline-start" /> : <IconSpark data-icon="inline-start" />}
                Draft it for me
              </InputGroupButton>
            </InputGroupAddon>
          </InputGroup>
        </form>
        <ErrorNote error={templates.error} />
        <ItemGroup className="grid gap-2 sm:grid-cols-2">
          {personas.map((t) => (
            <Item key={t.id} asChild variant="outline">
              <button type="button" className="text-left" onClick={() => install(t)} disabled={busy !== null}>
                <ItemMedia>
                  <PersonaAvatar persona={{ name: t.id, display_name: t.title, avatar: templateAvatar(t) }} />
                </ItemMedia>
                <ItemContent>
                  <ItemTitle>
                    {t.title}
                    {t.installed ? (
                      <Badge variant="secondary">Already added</Badge>
                    ) : (
                      busy === t.id && (
                        <Badge variant="outline">
                          <Spinner data-icon="inline-start" />
                          Adding
                        </Badge>
                      )
                    )}
                  </ItemTitle>
                  <ItemDescription>{t.description}</ItemDescription>
                </ItemContent>
              </button>
            </Item>
          ))}
          <Item asChild variant="outline">
            <button type="button" className="text-left" onClick={onBlank}>
              <ItemContent>
                <ItemTitle>Blank persona</ItemTitle>
                <ItemDescription>Start from an empty form.</ItemDescription>
              </ItemContent>
            </button>
          </Item>
        </ItemGroup>
      </CardContent>
    </Card>
  );
}

/** A template's avatar, read from its YAML. */
function templateAvatar(t: Template): string | undefined {
  try {
    return (YAML.parse(t.yaml) as PersonaSpec).avatar ?? undefined;
  } catch {
    return undefined;
  }
}

type Turn = { role: 'user' | 'assistant'; text: string };

/**
 * A side chat with the built-in assistant speaking as this persona, unsaved
 * edits included. The chat is thrown away when the panel closes.
 */
function TryIt({ persona, open, onOpenChange }: { persona: PersonaSpec; open: boolean; onOpenChange: (o: boolean) => void }) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState('');
  const [busy, setBusy] = useState(false);
  const session = useRef<string | undefined>(undefined);

  function reset() {
    if (session.current) api.deleteSession(session.current).catch(() => {});
    session.current = undefined;
    setTurns([]);
  }

  async function send(text: string) {
    const msg = text.trim();
    if (!msg || busy) return;
    setInput('');
    setBusy(true);
    setTurns((t) => [...t, { role: 'user', text: msg }, { role: 'assistant', text: '' }]);
    try {
      await streamRun(
        { input: msg, session_id: session.current, persona: { ...persona, name: persona.name || 'draft' } },
        (e) => {
          if (e.type === 'run_started') session.current = e.session_id;
          if (e.type === 'text_delta')
            setTurns((t) => {
              const next = [...t];
              const last = next[next.length - 1];
              next[next.length - 1] = { ...last, text: last.text + e.text };
              return next;
            });
          if (e.type === 'run_finished' && e.status !== 'completed') throw new Error(e.error ?? e.status);
        },
      );
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Sheet
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
    >
      <SheetContent className="flex w-full flex-col sm:max-w-md">
        <SheetHeader>
          <SheetTitle className="flex items-center gap-2">
            <PersonaAvatar persona={persona} />
            Try {personaName({ ...persona, name: persona.name || 'this persona' })}
          </SheetTitle>
          <SheetDescription>Uses your unsaved changes. This chat is not kept.</SheetDescription>
        </SheetHeader>
        <ScrollArea className="min-h-0 flex-1 px-4">
          <div className="flex flex-col gap-4 pb-4">
            {turns.length === 0 && (
              <>
                {persona.greeting && <Markdown text={persona.greeting} />}
                <div className="flex flex-col gap-2">
                  {(persona.starters?.length ? persona.starters : ['Introduce yourself.', 'Explain how a rainbow forms.']).map((s) => (
                    <Button key={s} variant="outline" className="h-auto justify-start py-2 text-left whitespace-normal" onClick={() => send(s)}>
                      {s}
                    </Button>
                  ))}
                </div>
              </>
            )}
            {turns.map((t, i) =>
              t.role === 'user' ? (
                <Message key={i} align="end">
                  <MessageContent>
                    <Bubble variant="secondary" align="end">
                      <BubbleContent className="whitespace-pre-wrap">{t.text}</BubbleContent>
                    </Bubble>
                  </MessageContent>
                </Message>
              ) : (
                <Message key={i} align="start">
                  <MessageAvatar>
                    <PersonaAvatar persona={persona} />
                  </MessageAvatar>
                  <MessageContent>{t.text ? <Markdown text={t.text} /> : <Spinner />}</MessageContent>
                </Message>
              ),
            )}
          </div>
        </ScrollArea>
        <SheetFooter>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              send(input);
            }}
          >
            <InputGroup>
              <InputGroupTextarea
                rows={1}
                aria-label="Message"
                placeholder="Say something…"
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    send(input);
                  }
                }}
              />
              <InputGroupAddon align="inline-end">
                <InputGroupButton type="submit" size="icon-xs" aria-label="Send" disabled={busy || !input.trim()}>
                  {busy ? <Spinner /> : <IconSend />}
                </InputGroupButton>
              </InputGroupAddon>
            </InputGroup>
          </form>
          {turns.length > 0 && (
            <Button variant="ghost" size="sm" onClick={reset}>
              Start over
            </Button>
          )}
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
