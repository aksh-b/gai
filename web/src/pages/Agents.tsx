import { useEffect, useState } from 'react';
import { Button, toast } from '@flame';
import YAML from 'yaml';
import { AgentForm } from '../AgentForm';
import { api, streamRun, type AgentSpec, type AgentVersion, type Template } from '../api';
import { SpecEditor } from '../forms';
import { Empty, ErrorNote, PageHeader } from '../components';
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
      api.agentVersions(current.summary.name).then(setVersions).catch(() => setVersions([]));
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
    if (!confirm(`Delete ${name}? Its earlier versions are deleted too.`)) return;
    await api.deleteAgent(name).catch((e: Error) => toast.error(e.message));
    list.reload();
    navigate('/assistants');
  }

  const startNew = () => {
    setPicking(true);
    navigate('/assistants?new=1');
  };

  return (
    <div className="page">
      <PageHeader
        title="Assistants"
        subtitle="Each assistant has its own instructions, tools and knowledge. Every change is kept, so you can always go back."
        actions={
          <Button tone="accent" leading={<IconPlus />} onClick={startNew}>
            New assistant
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="agents-layout">
        <div className="agent-list">
          {list.data?.map(({ summary, spec }) => (
            <a
              key={summary.name}
              href={`#/assistants/${encodeURIComponent(summary.name)}`}
              className="agent-card"
              aria-current={summary.name === selected ? 'true' : undefined}
            >
              <div className="row between">
                <strong>{summary.name}</strong>
                {dev && <span className="chip">v{summary.latest_version}</span>}
              </div>
              <p className="muted small">{spec?.description ?? (spec?.model ? modelLabel(spec.model) : '')}</p>
              <div className="row gap wrap">
                {(spec?.tools ?? []).slice(0, 4).map((t) => (
                  <span key={t} className="chip small" title={t}>
                    {toolLabel(t)}
                  </span>
                ))}
                {dev &&
                  Object.entries(summary.labels).map(([l, v]) => (
                    <span key={l} className="chip accent small">
                      {l}=v{v}
                    </span>
                  ))}
              </div>
            </a>
          ))}
          {list.data?.length === 0 && !picking && (
            <div className="empty">
              <strong>No assistants yet</strong>
              <p className="muted">Start from a ready-made one and change it as you like.</p>
              <Button tone="accent" onClick={startNew}>
                Browse templates
              </Button>
            </div>
          )}
        </div>

        {picking && selected !== 'new' ? (
          <section className="card editor">
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
          </section>
        ) : selected ? (
          <section className="card editor">
            <div className="row between">
              <h3>{selected === 'new' ? 'New assistant' : selected}</h3>
              <div className="row gap">
                {current && (
                  <>
                    <Button
                      variant="outline"
                      size="sm"
                      leading={<IconChat />}
                      onClick={() => navigate(`/chat?agent=${encodeURIComponent(current.summary.name)}`)}
                    >
                      Chat
                    </Button>
                    <Button variant="ghost" tone="danger" size="sm" leading={<IconTrash />} onClick={() => remove(current.summary.name)}>
                      Delete
                    </Button>
                  </>
                )}
              </div>
            </div>
            <SpecEditor label="Assistant" value={spec} onChange={setSpec}>
              <AgentForm key={selected} spec={spec} onChange={setSpec} isNew={selected === 'new'} />
            </SpecEditor>
            <div className="save-bar">
              <ErrorNote error={parseError} />
              <div className="row gap">
                {dirty && <span className="muted small nowrap">Unsaved changes</span>}
                {dev && (
                  <input className="plain-input" placeholder="Version note (optional)" value={note} onChange={(e) => setNote(e.target.value)} />
                )}
                <Button tone="accent" loading={saving} disabled={!dirty && selected !== 'new'} onClick={save}>
                  {selected === 'new' ? 'Create assistant' : 'Save changes'}
                </Button>
              </div>
            </div>
            {viewing !== undefined && current && viewing !== current.summary.latest_version && (
              <p className="note">
                Showing an earlier version in the editor. Saving it makes it the current one; <em>Roll back</em> below does the same without
                edits.
              </p>
            )}
            {current && versions.length > 1 && (
              <>
                <h4>History</h4>
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
              </>
            )}
          </section>
        ) : (
          <section className="card editor">
            <Empty title="Pick an assistant">Choose one on the left to change it, or make a new one.</Empty>
          </section>
        )}
      </div>
    </div>
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
      await streamRun({ agent: BUILDER, input: `Design an assistant that: ${describe.trim()}` }, (e) => {
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
    <div className="stack">
      <div>
        <h3>New assistant</h3>
        <p className="muted small">Describe what you need, or start from a template. You can change everything afterwards.</p>
      </div>
      <form
        className="describe"
        onSubmit={(e) => {
          e.preventDefault();
          if (describe.trim()) draft();
        }}
      >
        <textarea
          className="plain-input"
          rows={2}
          placeholder="e.g. Reads web pages I give it and summarizes them in three bullet points"
          value={describe}
          onChange={(e) => setDescribe(e.target.value)}
        />
        <Button type="submit" tone="accent" leading={<IconSpark />} loading={drafting} disabled={!describe.trim()}>
          Draft it for me
        </Button>
      </form>
      <ErrorNote error={templates.error} />
      <div className="template-grid">
        {agents.map((t) => (
          <button key={t.id} type="button" className="template" onClick={() => install(t)} disabled={busy !== null}>
            <strong>{t.title}</strong>
            <span className="muted small">{t.description}</span>
            {t.installed ? <span className="chip small">already added</span> : busy === t.id && <span className="chip small">adding…</span>}
          </button>
        ))}
        <button type="button" className="template blank" onClick={onBlank}>
          <strong>Blank assistant</strong>
          <span className="muted small">Start from an empty form.</span>
        </button>
      </div>
    </div>
  );
}
