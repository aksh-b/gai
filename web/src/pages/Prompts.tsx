// Prompt templates: instructions kept and versioned on their own, which any
// assistant can use (its "Start from a prompt template" setting). Labels
// let assistants follow a version (`support-tone@prod`) while you work on
// the next one; the prompt optimizer saves its improvements here too.

import { useEffect, useRef, useState } from 'react';
import { ScrollTextIcon } from 'lucide-react';
import { toast } from 'sonner';
import { TabBar, TextField } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, Json, LinkItem, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldDescription } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { ItemDescription, ItemGroup } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { api, type PromptEntry, type PromptSpec } from '../api';
import { insertAt, Section, TextArea } from '../forms';
import { IconPlus, IconTrash } from '../icons';
import { useDevMode } from '../prefs';
import { navigate } from '../router';
import { ago, useLoad } from '../util';
import { VersionHistory } from '../Versions';

const BLANK: PromptSpec = { name: '', text: '' };

/** Variables a template uses: `{{ name }}`, minus the built-in ones. */
const BUILT_IN = new Set(['agent', 'model', 'date', 'datetime', 'workspace', 'persona']);
function usedVars(text: string): string[] {
  const out = new Set<string>();
  for (const m of text.matchAll(/\{\{-?\s*([A-Za-z_][A-Za-z0-9_]*)/g)) if (!BUILT_IN.has(m[1])) out.add(m[1]);
  return [...out];
}

export function Prompts({ selected, creating }: { selected?: string; creating?: boolean }) {
  const list = useLoad(() => api.prompts(), []);
  const [isNew, setNew] = useState(!!creating);
  useEffect(() => setNew(!!creating), [creating]);
  const current = isNew ? undefined : (list.data?.find((p) => p.summary.name === selected) ?? list.data?.[0]);

  return (
    <Page wide>
      <PageHeader
        title="Prompts"
        subtitle="Instructions kept on their own, with versions. Several assistants can share one, and labels like prod let them follow a version while you work on the next."
        actions={
          <Button onClick={() => setNew(true)}>
            <IconPlus data-icon="inline-start" />
            New prompt
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[19rem_minmax(0,1fr)]">
        <ItemGroup className="gap-2">
          {list.data?.map((p) => (
            <LinkItem
              key={p.summary.name}
              href={`#/prompts/${encodeURIComponent(p.summary.name)}`}
              active={current?.summary.name === p.summary.name}
              onClick={() => setNew(false)}
              title={
                <>
                  <span className="truncate">{p.summary.title || p.summary.name}</span>
                  {p.used_by.length > 0 && (
                    <Badge variant="outline" className="ml-auto">
                      {p.used_by.length} using
                    </Badge>
                  )}
                </>
              }
            >
              {p.summary.description && <ItemDescription>{p.summary.description}</ItemDescription>}
            </LinkItem>
          ))}
          {list.data?.length === 0 && !isNew && (
            <EmptyState title="No prompts yet" icon={<ScrollTextIcon />}>
              Write instructions once and use them from any assistant.
            </EmptyState>
          )}
        </ItemGroup>
        {isNew ? (
          <Editor
            key="new"
            initial={BLANK}
            isNew
            onSaved={(name) => {
              setNew(false);
              list.reload();
              navigate(`/prompts/${encodeURIComponent(name)}`);
            }}
          />
        ) : current ? (
          <PromptPanel key={current.summary.name} entry={current} onChange={list.reload} />
        ) : null}
      </div>
    </Page>
  );
}

function PromptPanel({ entry, onChange }: { entry: PromptEntry; onChange: () => void }) {
  const [tab, setTab] = useState('edit');
  const versions = useLoad(() => api.promptVersions(entry.summary.name), [entry.summary.name, entry.summary.latest_version]);
  const name = entry.summary.name;
  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg">
            {entry.summary.title || name}
            <Badge variant="outline">v{entry.summary.latest_version}</Badge>
            {Object.entries(entry.summary.labels).map(([l, v]) => (
              <Badge key={l} variant="secondary">
                {l} → v{v}
              </Badge>
            ))}
          </CardTitle>
          <CardDescription>
            {entry.used_by.length ? `Used by ${entry.used_by.join(', ')}` : 'No assistant uses it yet'} · changed{' '}
            {ago(entry.summary.updated_at)}
          </CardDescription>
          <CardAction>
            <ConfirmAction
              title={`Delete ${name}?`}
              description="Every version is deleted. Prompts that an assistant uses can't be deleted."
              action="Delete"
              destructive
              onConfirm={async () => {
                try {
                  await api.deletePrompt(name);
                  navigate('/prompts');
                  onChange();
                } catch (e) {
                  toast.error((e as Error).message);
                }
              }}
            >
              <Button variant="ghost" size="sm">
                <IconTrash data-icon="inline-start" />
                Delete
              </Button>
            </ConfirmAction>
          </CardAction>
        </CardHeader>
      </Card>
      <TabBar
        items={[
          { id: 'edit', label: 'Edit' },
          { id: 'versions', label: 'Versions' },
        ]}
        value={tab}
        onValueChange={setTab}
        variant="underline"
        aria-label="Prompt view"
      />
      {tab === 'edit' && entry.spec && (
        <Editor key={entry.summary.latest_version} initial={entry.spec} isNew={false} onSaved={onChange} />
      )}
      {tab === 'versions' && (
        <Card>
          <CardContent className="flex flex-col gap-3">
            <ErrorNote error={versions.error} />
            <FieldDescription>
              Point a label (like <code>prod</code>) at a version, and use <code>{name}@prod</code> in an assistant to follow it.
            </FieldDescription>
            <VersionHistory
              kind="prompt"
              name={name}
              versions={versions.data ?? []}
              labels={entry.summary.labels}
              onChanged={() => {
                versions.reload();
                onChange();
              }}
            />
          </CardContent>
        </Card>
      )}
    </div>
  );
}

function Editor({ initial, isNew, onSaved }: { initial: PromptSpec; isNew: boolean; onSaved: (name: string) => void }) {
  const [dev] = useDevMode();
  const [spec, setSpec] = useState<PromptSpec>(initial);
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [preview, setPreview] = useState<string>();
  const textRef = useRef<HTMLTextAreaElement>(null);
  const vars = spec.variables ?? {};
  const set = (patch: Partial<PromptSpec>) => setSpec((s) => ({ ...s, ...patch }));
  const missing = usedVars(spec.text).filter((v) => !(v in vars));

  // The text as an assistant would get it, with the defaults.
  useEffect(() => {
    if (!spec.text.trim()) return setPreview(undefined);
    const t = setTimeout(() => {
      api
        .renderPrompt({ ...spec, name: spec.name || 'draft' })
        .then((r) => {
          setPreview(r.text);
          setError(undefined);
        })
        .catch((e: Error) => setError(e.message));
    }, 400);
    return () => clearTimeout(t);
  }, [spec]);

  async function save() {
    setBusy(true);
    setError(undefined);
    try {
      const r = await api.savePrompt(spec, note || undefined);
      toast.success(`Saved ${r.name} v${r.version}`);
      setNote('');
      onSaved(r.name);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <Section title="Basics">
        <TextField
          label="Name"
          value={spec.name}
          disabled={!isNew}
          description={isNew ? 'Letters, digits, -, _ and . Assistants refer to it by this.' : 'Saving makes a new version.'}
          onChange={(e) => set({ name: e.target.value.replace(/[^A-Za-z0-9_.-]/g, '-') })}
        />
        <TextField label="Title" value={spec.title ?? ''} onChange={(e) => set({ title: e.target.value || undefined })} />
        <TextField
          className="col-span-full"
          label="Description"
          value={spec.description ?? ''}
          onChange={(e) => set({ description: e.target.value || undefined })}
        />
      </Section>
      <Section title="Instructions" hint="Words in {{ }} are filled in each time: today's {{ date }}, or variables of your own below.">
        <TextArea
          label="Text"
          rows={12}
          value={spec.text}
          placeholder="You answer customers of {{ company }}. Be brief and kind…"
          textareaRef={textRef}
          onChange={(text) => set({ text })}
        />
        {missing.length > 0 && (
          <div className="col-span-full flex flex-wrap items-center gap-1.5">
            <span className="text-sm text-muted-foreground">Uses variables without a default:</span>
            {missing.map((v) => (
              <Button
                key={v}
                variant="secondary"
                size="xs"
                onClick={() => set({ variables: { ...vars, [v]: { description: undefined, default: undefined } } })}
              >
                <IconPlus data-icon="inline-start" />
                {v}
              </Button>
            ))}
          </div>
        )}
      </Section>
      <Card>
        <CardHeader>
          <CardTitle>Variables</CardTitle>
          <CardDescription>A run can pass its own values; these are used when it doesn't.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          {Object.entries(vars).map(([k, v]) => (
            <div key={k} className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                size="sm"
                className="w-36 justify-start font-mono"
                title="Insert into the text"
                onClick={() => set({ text: insertAt(textRef.current, spec.text, `{{ ${k} }}`) })}
              >
                {k}
              </Button>
              <Input
                className="min-w-40 flex-1"
                aria-label={`What ${k} is`}
                placeholder="What it is"
                value={v.description ?? ''}
                onChange={(e) => set({ variables: { ...vars, [k]: { ...v, description: e.target.value || undefined } } })}
              />
              <Input
                className="w-48"
                aria-label={`Default for ${k}`}
                placeholder="Default"
                value={v.default == null ? '' : String(v.default)}
                onChange={(e) => set({ variables: { ...vars, [k]: { ...v, default: e.target.value === '' ? undefined : e.target.value } } })}
              />
              <Button
                variant="ghost"
                size="icon"
                aria-label={`Remove ${k}`}
                onClick={() => {
                  const next = { ...vars };
                  delete next[k];
                  set({ variables: next });
                }}
              >
                <IconTrash />
              </Button>
            </div>
          ))}
          {Object.keys(vars).length === 0 && <FieldDescription>None.</FieldDescription>}
        </CardContent>
      </Card>
      {preview !== undefined && (
        <Card>
          <CardHeader>
            <CardTitle>Preview</CardTitle>
            <CardDescription>With the defaults filled in.</CardDescription>
          </CardHeader>
          <CardContent>
            <p className="text-sm whitespace-pre-wrap">{preview}</p>
            {dev && <Json value={spec} className="mt-3" />}
          </CardContent>
        </Card>
      )}
      <ErrorNote error={error} />
      <div className="flex items-center justify-end gap-2">
        {!isNew && (
          <Input className="max-w-64" placeholder="Version note (optional)" aria-label="Version note" value={note} onChange={(e) => setNote(e.target.value)} />
        )}
        <Button onClick={save} disabled={busy || !spec.name || !spec.text.trim()}>
          {busy && <Spinner data-icon="inline-start" />}
          {isNew ? 'Create prompt' : 'Save as a new version'}
        </Button>
      </div>
    </div>
  );
}
