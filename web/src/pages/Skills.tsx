import { useState } from 'react';
import { Button, toast } from '@flame';
import { api, type SkillView } from '../api';
import { Disclosure, Empty, ErrorNote, Markdown, PageHeader } from '../components';
import { IconPlus, IconTrash } from '../icons';
import { useLoad } from '../util';

const TEMPLATE = `---
name: my-skill
description: What this skill does, and when an agent should use it.
---
# My skill

Step-by-step instructions the agent follows once it loads this skill.
`;

export function Skills() {
  const list = useLoad(() => api.skills(), []);
  return (
    <div className="page">
      <PageHeader
        title="Skills"
        subtitle={
          <>
            Packaged instructions (and scripts or templates) in the Agent Skills format. An agent with{' '}
            <span className="mono">skills: [name]</span> sees only each skill's description, and calls{' '}
            <span className="mono">load_skill</span> for the full instructions when a task needs them.
          </>
        }
      />
      <ErrorNote error={list.error} />
      <div className="tool-list">
        {list.data?.map((s) => (
          <SkillCard key={s.name} s={s} onChange={list.reload} />
        ))}
      </div>
      {list.data?.length === 0 && (
        <Empty title="No skills yet">Put skill folders (each with a SKILL.md) in ./skills, or upload one below.</Empty>
      )}
      <section className="card">
        <Disclosure summary={<strong>Upload a skill</strong>} open={list.data?.length === 0}>
          <Upload onSaved={list.reload} />
        </Disclosure>
      </section>
    </div>
  );
}

function SkillCard({ s, onChange }: { s: SkillView; onChange: () => void }) {
  const [open, setOpen] = useState(false);
  const detail = useLoad(() => (open ? api.skill(s.name) : Promise.resolve(undefined)), [open, s.name]);
  const stored = s.source.kind === 'stored';
  async function remove() {
    if (!confirm(`Delete the skill ${s.name}?`)) return;
    await api.deleteSkill(s.name).catch((e: Error) => toast.error(e.message));
    onChange();
  }
  return (
    <section className="card stack tight">
      <div className="row between">
        <strong className="mono">{s.name}</strong>
        <div className="row gap">
          <span className="chip small">{s.source.kind === 'stored' ? `uploaded v${s.source.version}` : 'folder'}</span>
          {stored && (
            <button type="button" className="icon-btn" aria-label={`Delete ${s.name}`} onClick={remove}>
              <IconTrash />
            </button>
          )}
        </div>
      </div>
      <p className="muted" style={{ margin: 0 }}>
        {s.description}
      </p>
      {s.source.kind === 'dir' && <span className="muted small mono">{s.source.path}</span>}
      {s.files.length > 0 && (
        <div className="row gap wrap">
          {s.files.map((f) => (
            <span key={f} className="chip small mono">
              {f}
            </span>
          ))}
        </div>
      )}
      <button type="button" className="link small" style={{ alignSelf: 'flex-start' }} onClick={() => setOpen(!open)}>
        {open ? 'Hide instructions' : 'Instructions'}
      </button>
      {open && detail.data && <Markdown text={detail.data.instructions} />}
    </section>
  );
}

function Upload({ onSaved }: { onSaved: () => void }) {
  const [content, setContent] = useState(TEMPLATE);
  const [files, setFiles] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  async function addFiles(list: FileList | null) {
    if (!list) return;
    const next = { ...files };
    for (const f of Array.from(list)) {
      if (f.name === 'SKILL.md') setContent(await f.text());
      else next[f.webkitRelativePath.split('/').slice(1).join('/') || f.name] = await f.text();
    }
    setFiles(next);
  }

  async function save() {
    setBusy(true);
    try {
      const r = await api.saveSkill(content, files);
      toast.success(`Saved ${r.name} v${r.version}`);
      setContent(TEMPLATE);
      setFiles({});
      onSaved();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="stack" style={{ paddingTop: 8 }}>
      <label className="label" htmlFor="skill-md">
        SKILL.md
      </label>
      <textarea id="skill-md" className="code-editor" style={{ minHeight: 220 }} value={content} onChange={(e) => setContent(e.target.value)} />
      <div className="row gap wrap">
        <label className="dropzone inline">
          <input type="file" multiple onChange={(e) => addFiles(e.target.files)} />
          <span className="small">Add files (scripts, templates, references; a SKILL.md replaces the text above)</span>
        </label>
        {Object.keys(files).map((f) => (
          <span key={f} className="chip small mono">
            {f}
            <button
              type="button"
              className="icon-btn"
              aria-label={`Remove ${f}`}
              onClick={() => setFiles(Object.fromEntries(Object.entries(files).filter(([k]) => k !== f)))}
            >
              ×
            </button>
          </span>
        ))}
      </div>
      <div>
        <Button tone="accent" leading={<IconPlus />} loading={busy} onClick={save}>
          Save skill
        </Button>
      </div>
    </div>
  );
}
