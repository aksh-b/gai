import { useEffect, useState } from 'react';
import { Button, toast } from '@flame';
import { AgentForm } from '../AgentForm';
import { api, type AgentSpec, type AgentVersion } from '../api';
import { SpecEditor } from '../forms';
import { Empty, ErrorNote, PageHeader } from '../components';
import { IconChat, IconPlus, IconTrash } from '../icons';
import { navigate } from '../router';
import { useLoad } from '../util';
import { VersionHistory } from '../Versions';

const TEMPLATE: AgentSpec = {
  name: '',
  system: 'You are {{ agent }}, a helpful assistant. Today is {{ date }}.\nBe concise, and say so when you are unsure.',
  tools: ['calculator', 'current_time'],
  limits: { max_steps: 30, max_cost_usd: 1.0 },
};

export function Agents({ selected }: { selected?: string }) {
  const list = useLoad(() => api.agents(), []);
  const [spec, setSpec] = useState<AgentSpec>(TEMPLATE);
  const [loaded, setLoaded] = useState<string>('');
  const [note, setNote] = useState('');
  const [parseError, setParseError] = useState<string>();
  const [saving, setSaving] = useState(false);
  const [versions, setVersions] = useState<AgentVersion[]>([]);

  const current = list.data?.find((a) => a.summary.name === selected);

  const [viewing, setViewing] = useState<number>();

  useEffect(() => {
    if (selected === 'new') {
      setSpec(TEMPLATE);
      setLoaded(JSON.stringify(TEMPLATE));
      setVersions([]);
    } else if (current?.spec) {
      setSpec(current.spec);
      setLoaded(JSON.stringify(current.spec));
      setViewing(undefined);
      api.agentVersions(current.summary.name).then(setVersions).catch(() => setVersions([]));
    }
  }, [selected, current]);

  const dirty = JSON.stringify(spec) !== loaded;

  async function save() {
    if (!spec.name?.trim()) {
      setParseError('Give the agent a name.');
      return;
    }
    setParseError(undefined);
    setSaving(true);
    try {
      const r = await api.saveAgent(spec, note || undefined);
      toast.success(`Saved ${r.name} v${r.version}`);
      setNote('');
      list.reload();
      navigate(`/agents/${r.name}`);
    } catch (e) {
      setParseError((e as Error).message);
    } finally {
      setSaving(false);
    }
  }

  async function remove(name: string) {
    if (!confirm(`Delete ${name} and all its versions?`)) return;
    await api.deleteAgent(name).catch((e: Error) => toast.error(e.message));
    list.reload();
    navigate('/agents');
  }

  return (
    <div className="page">
      <PageHeader
        title="Agents"
        subtitle="Versioned agent definitions. Every save is a new immutable version."
        actions={
          <Button tone="accent" leading={<IconPlus />} onClick={() => navigate('/agents/new')}>
            New agent
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="agents-layout">
        <div className="agent-list">
          {list.data?.map(({ summary, spec }) => (
            <a
              key={summary.name}
              href={`#/agents/${summary.name}`}
              className="agent-card"
              aria-current={summary.name === selected ? 'true' : undefined}
            >
              <div className="row between">
                <strong>{summary.name}</strong>
                <span className="chip">v{summary.latest_version}</span>
              </div>
              <p className="muted small">{spec?.description ?? spec?.model ?? ''}</p>
              <div className="row gap wrap">
                {(spec?.tools ?? []).slice(0, 4).map((t) => (
                  <span key={t} className="chip mono small">
                    {t}
                  </span>
                ))}
                {Object.entries(summary.labels).map(([l, v]) => (
                  <span key={l} className="chip accent small">
                    {l}=v{v}
                  </span>
                ))}
              </div>
            </a>
          ))}
          {list.data?.length === 0 && (
            <Empty title="No stored agents">Create one, or save an agent file with `gai agents save`.</Empty>
          )}
        </div>

        {selected ? (
          <section className="card editor">
            <div className="row between">
              <h3>{selected === 'new' ? 'New agent' : selected}</h3>
              <div className="row gap">
                {current && (
                  <>
                    <Button
                      variant="outline"
                      size="sm"
                      leading={<IconChat />}
                      onClick={() => navigate(`/playground?agent=${current.summary.name}`)}
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
            <SpecEditor label="Agent" value={spec} onChange={setSpec}>
              <AgentForm key={selected} spec={spec} onChange={setSpec} isNew={selected === 'new'} />
            </SpecEditor>
            <div className="save-bar">
              <ErrorNote error={parseError} />
              <div className="row gap">
                {dirty && <span className="muted small nowrap">Unsaved changes</span>}
                <input className="plain-input" placeholder="Version note (optional)" value={note} onChange={(e) => setNote(e.target.value)} />
                <Button tone="accent" loading={saving} disabled={!dirty && selected !== 'new'} onClick={save}>
                  {selected === 'new' ? 'Create agent' : 'Save version'}
                </Button>
              </div>
            </div>
            {viewing !== undefined && current && viewing !== current.summary.latest_version && (
              <p className="note">
                Showing v{viewing} in the editor. Saving it makes a new version; <em>Roll back</em> below does the same without edits.
              </p>
            )}
            {current && versions.length > 0 && (
              <>
                <h4>Versions</h4>
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
            <Empty title="Pick an agent">Choose one on the left to edit it, or create a new one.</Empty>
          </section>
        )}
      </div>
    </div>
  );
}
