import { useEffect, useState } from 'react';
import { BotIcon, HistoryIcon } from 'lucide-react';
import { toast } from 'sonner';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupTextarea } from '@/components/ui/input-group';
import { Item, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Separator } from '@/components/ui/separator';
import { Spinner } from '@/components/ui/spinner';
import YAML from 'yaml';
import { AgentForm } from '../AgentForm';
import { BundleMenu } from '../Bundles';
import { api, streamRun, type AgentSpec, type AgentVersion, type Template } from '../api';
import { SpecEditor } from '../forms';
import { ConfirmAction, EmptyState, ErrorNote, LinkItem, Page, PageHeader } from '@/components/shared';
import { IconChat, IconPlus, IconSpark, IconTrash } from '../icons';
import { modelLabel, toolLabel } from '../labels';
import { useDevMode } from '../prefs';
import { navigate } from '../router';
import { useLoad } from '../util';
import { VersionHistory } from '../Versions';

const BLANK: AgentSpec = {
  name: '',
  system: 'You are {{ agent }}, a helpful assistant. Today is {{ date }}.\nBe concise, and say so when you are unsure.',
  tools: ['calculator', 'current_time'],
  limits: { max_steps: 30, max_cost_usd: 1.0 },
};

/** A spec handed from elsewhere (Chat's "Save as assistant") to the new-assistant editor. */
const DRAFT_KEY = 'gai.assistantDraft';

export function stashDraft(spec: AgentSpec) {
  try {
    sessionStorage.setItem(DRAFT_KEY, JSON.stringify(spec));
  } catch {
    /* the editor starts blank instead */
  }
}

function takeDraft(): AgentSpec | null {
  try {
    const raw = sessionStorage.getItem(DRAFT_KEY);
    sessionStorage.removeItem(DRAFT_KEY);
    return raw ? (JSON.parse(raw) as AgentSpec) : null;
  } catch {
    return null;
  }
}

export function Agents({ selected, creating = false }: { selected?: string; creating?: boolean }) {
  const [dev] = useDevMode();
  const list = useLoad(() => api.agents(), []);
  const [spec, setSpec] = useState<AgentSpec>(BLANK);
  const [loaded, setLoaded] = useState<string>('');
  const [note, setNote] = useState('');
  const [parseError, setParseError] = useState<string>();
  const [saving, setSaving] = useState(false);
  const [versions, setVersions] = useState<AgentVersion[]>([]);
  const [viewing, setViewing] = useState<number>();
  // Picking how to start a new assistant (templates, describe it, blank).
  const [picking, setPicking] = useState(creating);

  const current = list.data?.find((a) => a.summary.name === selected);

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
        .agentVersions(current.summary.name)
        .then(setVersions)
        .catch(() => setVersions([]));
    }
  }, [selected, current]);

  const dirty = JSON.stringify(spec) !== loaded;

  async function save() {
    if (!spec.name?.trim()) {
      setParseError('Give the assistant a name.');
      return;
    }
    setParseError(undefined);
    setSaving(true);
    try {
      const r = await api.saveAgent(spec, note || undefined);
      toast.success(selected === 'new' ? `Created ${r.name}` : `Saved ${r.name}${dev ? ` v${r.version}` : ''}`);
      setNote('');
      list.reload();
      navigate(`/assistants/${encodeURIComponent(r.name)}`);
    } catch (e) {
      setParseError((e as Error).message);
    } finally {
      setSaving(false);
    }
  }

  async function remove(name: string) {
    await api.deleteAgent(name).catch((e: Error) => toast.error(e.message));
    list.reload();
    navigate('/assistants');
  }

  const startNew = () => {
    setPicking(true);
    navigate('/assistants?new=1');
  };

  return (
    <Page wide>
      <PageHeader
        title="Assistants"
        subtitle="Each assistant has its own instructions, tools and knowledge. Every change is kept, so you can always go back."
        actions={
          <>
            <BundleMenu selected={current ? selected : undefined} onImported={() => list.reload()} />
            <Button onClick={startNew}>
              <IconPlus data-icon="inline-start" />
              New assistant
            </Button>
          </>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[19rem_minmax(0,1fr)]">
        <ItemGroup className="gap-2">
          {list.data?.map(({ summary, spec }) => (
            <LinkItem
              key={summary.name}
              href={`#/assistants/${encodeURIComponent(summary.name)}`}
              active={summary.name === selected}
              title={
                <>
                  <span className="truncate">{summary.name}</span>
                  {dev && (
                    <Badge variant="outline" className="ml-auto">
                      v{summary.latest_version}
                    </Badge>
                  )}
                </>
              }
            >
              <ItemDescription>{spec?.description ?? (spec?.model ? modelLabel(spec.model) : '')}</ItemDescription>
              <div className="flex flex-wrap gap-1 pt-1">
                {(spec?.tools ?? []).slice(0, 4).map((t) => (
                  <Badge key={t} variant="secondary" title={t}>
                    {toolLabel(t)}
                  </Badge>
                ))}
                {dev &&
                  Object.entries(summary.labels).map(([l, v]) => (
                    <Badge key={l} variant="outline">
                      {l}=v{v}
                    </Badge>
                  ))}
              </div>
            </LinkItem>
          ))}
          {list.data?.length === 0 && !picking && (
            <EmptyState title="No assistants yet" icon={<BotIcon />} action={<Button onClick={startNew}>Browse templates</Button>}>
              Start from a ready-made one and change it as you like.
            </EmptyState>
          )}
        </ItemGroup>

        {picking && selected !== 'new' ? (
          <TemplatePicker
            onBlank={() => navigate('/assistants/new')}
            onInstalled={(name) => {
              list.reload();
              navigate(`/assistants/${encodeURIComponent(name)}`);
            }}
            onDraft={(draft) => {
              stashDraft(draft);
              navigate('/assistants/new');
            }}
          />
        ) : selected ? (
          <Card>
            <CardHeader>
              <CardTitle className="text-lg">{selected === 'new' ? 'New assistant' : selected}</CardTitle>
              {current && (
                <CardAction className="flex gap-2">
                  <Button variant="outline" size="sm" onClick={() => navigate(`/chat?agent=${encodeURIComponent(current.summary.name)}`)}>
                    <IconChat data-icon="inline-start" />
                    Chat
                  </Button>
                  <ConfirmAction
                    title={`Delete ${current.summary.name}?`}
                    description="Its earlier versions are deleted too. Chats with it are kept."
                    action="Delete"
                    destructive
                    onConfirm={() => remove(current.summary.name)}
                  >
                    <Button variant="destructive" size="sm">
                      <IconTrash data-icon="inline-start" />
                      Delete
                    </Button>
                  </ConfirmAction>
                </CardAction>
              )}
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              {viewing !== undefined && current && viewing !== current.summary.latest_version && (
                <Alert>
                  <HistoryIcon />
                  <AlertDescription>
                    Showing an earlier version in the editor. Saving it makes it the current one; Roll back below does the same without
                    edits.
                  </AlertDescription>
                </Alert>
              )}
              <SpecEditor label="Assistant" schema="AgentSpec" value={spec} onChange={setSpec}>
                <AgentForm key={selected} spec={spec} onChange={setSpec} isNew={selected === 'new'} />
              </SpecEditor>
            </CardContent>
            <CardFooter className="sticky bottom-0 flex-col items-stretch gap-3 border-t bg-card">
              <ErrorNote error={parseError} />
              <div className="flex items-center justify-end gap-2">
                {dirty && <span className="text-sm whitespace-nowrap text-muted-foreground">Unsaved changes</span>}
                {dev && (
                  <Input
                    className="max-w-64"
                    placeholder="Version note (optional)"
                    value={note}
                    onChange={(e) => setNote(e.target.value)}
                  />
                )}
                <Button onClick={save} disabled={saving || (!dirty && selected !== 'new')}>
                  {saving && <Spinner data-icon="inline-start" />}
                  {selected === 'new' ? 'Create assistant' : 'Save changes'}
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
                    kind="agent"
                    name={current.summary.name}
                    versions={versions}
                    labels={current.summary.labels}
                    onView={(n) => {
                      const v = versions.find((x) => x.version === n);
                      if (v) {
                        setSpec(v.spec);
                        setViewing(n);
                      }
                    }}
                    onChanged={list.reload}
                  />
                </CardContent>
              </>
            )}
          </Card>
        ) : (
          <Card>
            <CardContent>
              <EmptyState title="Pick an assistant">Choose one on the left to change it, or make a new one.</EmptyState>
            </CardContent>
          </Card>
        )}
      </div>
    </Page>
  );
}

const BUILDER: AgentSpec = {
  name: 'assistant-builder',
  system: `You design AI assistants for the gai platform. Reply with only a YAML document in a \`\`\`yaml code block, with these keys:
name (lowercase-with-dashes), description (one sentence), system (the assistant's instructions, written to the assistant, 3-8 sentences),
tools (a list chosen only from: calculator, current_time, fs_read, fs_write, fs_edit, fs_list, http_fetch, rag_search).
Choose only the tools the assistant needs. Nothing else.`,
  tools: [],
  limits: { max_steps: 2 },
};

/** How to start a new assistant: describe it, a template, or blank. */
function TemplatePicker({
  onBlank,
  onInstalled,
  onDraft,
}: {
  onBlank: () => void;
  onInstalled: (name: string) => void;
  onDraft: (spec: AgentSpec) => void;
}) {
  const templates = useLoad(() => api.templates(), []);
  const [describe, setDescribe] = useState('');
  const [drafting, setDrafting] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  async function install(t: Template) {
    setBusy(t.id);
    try {
      const r = await api.installTemplate(t.id);
      toast.success(r.also.length ? `Added ${r.name} (and ${r.also.join(', ')}, which it works with)` : `Added ${r.name}`);
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
      await streamRun({ agent: BUILDER, persona: 'none', input: `Design an assistant that: ${describe.trim()}` }, (e) => {
        if (e.type === 'run_started') session = e.session_id;
        if (e.type === 'text_delta') text += e.text;
        if (e.type === 'run_finished' && e.status !== 'completed') throw new Error(e.error ?? `the model ${e.status}`);
      });
      const yaml = /```(?:yaml)?\s*([\s\S]*?)```/.exec(text)?.[1] ?? text;
      const spec = YAML.parse(yaml) as AgentSpec;
      if (!spec || typeof spec !== 'object' || !spec.system) throw new Error('the model did not return an assistant');
      onDraft({ ...BLANK, ...spec, limits: BLANK.limits });
    } catch (e) {
      toast.error(`Could not draft it: ${(e as Error).message}. Try a template, or a model that is better at following instructions.`);
    } finally {
      if (session) api.deleteSession(session).catch(() => {});
      setDrafting(false);
    }
  }

  const agents = (templates.data ?? []).filter((t) => t.kind === 'agent');

  return (
    <Card>
      <CardHeader>
        <CardTitle>New assistant</CardTitle>
        <CardDescription>Describe what you need, or start from a template. You can change everything afterwards.</CardDescription>
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
              aria-label="What the assistant should do"
              placeholder="e.g. Reads web pages I give it and summarizes them in three bullet points"
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
          {agents.map((t) => (
            <Item key={t.id} asChild variant="outline">
              <button type="button" className="text-left" onClick={() => install(t)} disabled={busy !== null}>
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
                <ItemTitle>Blank assistant</ItemTitle>
                <ItemDescription>Start from an empty form.</ItemDescription>
              </ItemContent>
            </button>
          </Item>
        </ItemGroup>
      </CardContent>
    </Card>
  );
}
