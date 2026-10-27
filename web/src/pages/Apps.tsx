// Apps: an assistant or workflow published as a form with its own link, and
// as an endpoint with an input and output schema. Built here without YAML;
// shared with a link that carries the app's own key, or made public.

import { useEffect, useMemo, useRef, useState } from 'react';
import { AppWindowIcon, CopyIcon, ExternalLinkIcon, GlobeIcon, LockIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField, SwitchField, TabBar, TextField, ToggleField } from '@/components/fields';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldDescription, FieldGroup } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { ItemDescription, ItemGroup } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { ConfirmAction, Disclosure, EmptyState, ErrorNote, LinkItem, Page, PageHeader, Snippet } from '@/components/shared';
import { api, appClient, getOrg, type AppField, type AppForm, type AppSpec, type AppView } from '../api';
import { IconKey, IconPlus, IconTrash } from '../icons';
import { insertAt, Section, TextArea } from '../forms';
import { navigate } from '../router';
import { ago, useLoad } from '../util';
import { AppRunner } from './PublicApp';

const BLANK: AppSpec = { name: '', fields: [{ name: 'request', label: 'Your request', type: 'long_text', required: true }] };

const TYPES = [
  { value: 'text', label: 'Short text' },
  { value: 'long_text', label: 'Long text' },
  { value: 'number', label: 'Number' },
  { value: 'boolean', label: 'Yes / no' },
  { value: 'select', label: 'Choice' },
  { value: 'list', label: 'List (one per line)' },
];

export function Apps({ selected, creating }: { selected?: string; creating?: boolean }) {
  const list = useLoad(() => api.apps(), []);
  const [isNew, setNew] = useState(!!creating);
  useEffect(() => setNew(!!creating), [creating]);
  const current = isNew ? undefined : (selected ?? list.data?.[0]?.name);
  const app = list.data?.find((a) => a.name === current);

  return (
    <Page wide>
      <PageHeader
        title="Apps"
        subtitle="Turn an assistant or workflow into a small app: a form people fill in, with its own link. Share it, or call it as an API."
        actions={
          <Button onClick={() => setNew(true)}>
            <IconPlus data-icon="inline-start" />
            New app
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[19rem_minmax(0,1fr)]">
        <ItemGroup className="gap-2">
          {list.data?.map((a) => (
            <LinkItem
              key={a.name}
              href={`#/apps/${encodeURIComponent(a.name)}`}
              active={current === a.name}
              onClick={() => setNew(false)}
              title={
                <>
                  <span className="truncate">{a.spec.title || a.name}</span>
                  <Badge variant="outline" className="ml-auto">
                    {a.spec.public ? 'public' : a.key_prefix ? 'link' : 'team only'}
                  </Badge>
                </>
              }
            >
              {a.spec.description && <ItemDescription>{a.spec.description}</ItemDescription>}
            </LinkItem>
          ))}
          {list.data?.length === 0 && !isNew && (
            <EmptyState title="No apps yet" icon={<AppWindowIcon />}>
              Pick an assistant or workflow, choose what people fill in, and share the link.
            </EmptyState>
          )}
        </ItemGroup>
        {isNew ? (
          <Editor
            initial={BLANK}
            isNew
            onSaved={(a) => {
              setNew(false);
              list.reload();
              navigate(`/apps/${encodeURIComponent(a.name)}`);
            }}
          />
        ) : app ? (
          <AppPanel key={app.name} app={app} onChange={list.reload} />
        ) : null}
      </div>
    </Page>
  );
}

function AppPanel({ app, onChange }: { app: AppView; onChange: () => void }) {
  const [tab, setTab] = useState('share');
  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">{app.spec.title || app.name}</CardTitle>
          <CardDescription>
            Runs {app.spec.agent ? `the assistant ${app.spec.agent}` : `the workflow ${app.spec.workflow}`} · changed {ago(app.updated_at)}
          </CardDescription>
          <CardAction>
            <ConfirmAction
              title={`Delete ${app.spec.title || app.name}?`}
              description="Its link and key stop working. Runs it already made are kept."
              action="Delete"
              destructive
              onConfirm={async () => {
                await api.deleteApp(app.name).catch((e: Error) => toast.error(e.message));
                navigate('/apps');
                onChange();
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
          { id: 'share', label: 'Share' },
          { id: 'try', label: 'Try it' },
          { id: 'edit', label: 'Edit' },
        ]}
        value={tab}
        onValueChange={setTab}
        variant="underline"
        aria-label="App view"
      />
      {tab === 'share' && <SharePanel app={app} onChange={onChange} />}
      {tab === 'try' && <TryPanel name={app.name} />}
      {tab === 'edit' && (
        <Editor
          initial={app.spec}
          isNew={false}
          onSaved={() => {
            onChange();
            setTab('try');
          }}
        />
      )}
    </div>
  );
}

/** The app's address, with its key when given. */
function appLink(name: string, key?: string) {
  const q = new URLSearchParams();
  if (key) q.set('key', key);
  const org = getOrg();
  if (org) q.set('org', org);
  const qs = q.toString();
  return `${location.origin}${location.pathname}#/app/${encodeURIComponent(name)}${qs ? `?${qs}` : ''}`;
}

function copy(text: string) {
  navigator.clipboard.writeText(text).then(
    () => toast.success('Copied'),
    () => toast.error('Copy failed'),
  );
}

function SharePanel({ app, onChange }: { app: AppView; onChange: () => void }) {
  const [secret, setSecret] = useState<string>();
  const [busy, setBusy] = useState(false);
  const form = useLoad(() => appClient(app.name, null, null).form(), [app.name, app.updated_at]);

  async function setPublic(on: boolean) {
    setBusy(true);
    try {
      await api.saveApp({ ...app.spec, public: on });
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function makeKey() {
    try {
      const k = await api.newAppKey(app.name);
      setSecret(k.secret);
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const link = app.spec.public ? appLink(app.name) : secret ? appLink(app.name, secret) : undefined;
  const example = Object.fromEntries((form.data?.fields ?? []).map((f) => [f.name, exampleValue(f)]));
  const keyNote = app.spec.public ? '' : ` \\\n  -H "Authorization: Bearer ${secret ?? '<the app key>'}"`;
  const curl = `curl -X POST ${location.origin}/v1/apps/${app.name}/run${keyNote} \\\n  -H "content-type: application/json" \\\n  -d '${JSON.stringify(example)}'`;

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>Who can use it</CardTitle>
          <CardDescription>
            People in your organization can always use it from the dashboard. Others need its link.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <SwitchField
            label="Anyone with the link"
            description="No key needed. Runs are paid by your organization and count towards its limits."
            checked={!!app.spec.public}
            disabled={busy}
            onCheckedChange={setPublic}
          />
          {!app.spec.public && (
            <div className="flex flex-wrap items-center gap-2">
              <LockIcon className="size-4 text-muted-foreground" />
              <span className="text-sm">
                {app.key_prefix ? (
                  <>
                    The link carries the app's own key (<code>{app.key_prefix}…</code>). It runs this app and nothing else.
                  </>
                ) : (
                  'Make a key to get a link you can share. The key runs this app and nothing else.'
                )}
              </span>
              <Button variant="outline" size="sm" className="ml-auto" onClick={makeKey}>
                <IconKey data-icon="inline-start" />
                {app.key_prefix ? 'New key' : 'Make a key'}
              </Button>
              {app.key_prefix && (
                <ConfirmAction
                  title="Remove the key?"
                  description="Links with it stop working."
                  action="Remove"
                  destructive
                  onConfirm={() =>
                    api
                      .removeAppKey(app.name)
                      .then(() => {
                        setSecret(undefined);
                        onChange();
                      })
                      .catch((e: Error) => toast.error(e.message))
                  }
                >
                  <Button variant="ghost" size="sm">
                    Remove key
                  </Button>
                </ConfirmAction>
              )}
            </div>
          )}
          {link ? (
            <div className="flex flex-col gap-2">
              {secret && (
                <Alert>
                  <IconKey />
                  <AlertTitle>Copy the link now</AlertTitle>
                  <AlertDescription>The key is shown only once. Making a new key stops this link from working.</AlertDescription>
                </Alert>
              )}
              <div className="flex gap-2">
                <Input readOnly value={link} aria-label="App link" className="font-mono text-xs" onFocus={(e) => e.target.select()} />
                <Button variant="outline" size="icon" aria-label="Copy link" onClick={() => copy(link)}>
                  <CopyIcon />
                </Button>
                <Button variant="outline" size="icon" aria-label="Open the app" asChild>
                  <a href={link} target="_blank" rel="noreferrer">
                    <ExternalLinkIcon />
                  </a>
                </Button>
              </div>
            </div>
          ) : (
            app.key_prefix &&
            !app.spec.public && <FieldDescription>Lost the link? Make a new key to get a new one.</FieldDescription>
          )}
          {app.spec.public && (
            <FieldDescription className="flex items-center gap-1.5">
              <GlobeIcon className="size-3.5" /> Public: anyone who has the link can use it.
            </FieldDescription>
          )}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Call it as an API</CardTitle>
          <CardDescription>
            Send the form's fields as JSON; the answer has the result in <code>output</code>. Add <code>?mode=background</code> to get a
            run id at once and fetch <code>/v1/apps/{app.name}/runs/&lt;id&gt;</code> later.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <Snippet title="Run it" code={curl} />
          <FieldDescription>
            The input and output schemas, as OpenAPI:{' '}
            <a className="underline" href={`/v1/apps/${encodeURIComponent(app.name)}/openapi.json`} target="_blank" rel="noreferrer">
              /v1/apps/{app.name}/openapi.json
            </a>
            . Your own access keys work too.
          </FieldDescription>
        </CardContent>
      </Card>
    </>
  );
}

function exampleValue(f: AppField): unknown {
  if (f.default != null) return f.default;
  switch (f.type) {
    case 'number':
      return 1;
    case 'boolean':
      return true;
    case 'select':
      return f.options?.[0] ?? '';
    case 'list':
      return ['…'];
    default:
      return '…';
  }
}

/** The app as people see it, run with your own sign-in. */
function TryPanel({ name }: { name: string }) {
  const client = useMemo(() => appClient(name, null, null), [name]);
  const [form, setForm] = useState<AppForm>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    client.form().then(setForm, (e: Error) => setError(e.message));
  }, [client]);
  if (error) return <ErrorNote error={error} />;
  if (!form) return <Spinner />;
  return <AppRunner form={form} client={client} />;
}

/** Make or change an app, without YAML. */
function Editor({ initial, isNew, onSaved }: { initial: AppSpec; isNew: boolean; onSaved: (a: AppView) => void }) {
  const [spec, setSpec] = useState<AppSpec>(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [schemaText, setSchemaText] = useState(() => (initial.output_schema ? JSON.stringify(initial.output_schema, null, 2) : ''));
  const [schemaError, setSchemaError] = useState<string>();
  const agents = useLoad(() => api.agents(), []);
  const workflows = useLoad(() => api.workflows(), []);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const kind = spec.workflow != null ? 'workflow' : 'agent';
  const fields = spec.fields ?? [];
  const set = (patch: Partial<AppSpec>) => setSpec((s) => ({ ...s, ...patch }));
  const setField = (i: number, f: AppField | null) => {
    const next = [...fields];
    if (f) next[i] = f;
    else next.splice(i, 1);
    set({ fields: next });
  };

  async function fromWorkflow(name: string) {
    try {
      const w = await api.workflow(name);
      set({
        fields: Object.entries(w.spec.inputs ?? {}).map(([k, v]) => ({
          name: k,
          label: k.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase()),
          type: Array.isArray(v.default) ? 'list' : 'text',
          required: v.default == null,
          default: v.default ?? undefined,
          help: v.description ?? undefined,
        })),
      });
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  async function save() {
    setBusy(true);
    setError(undefined);
    try {
      const saved = await api.saveApp(clean(spec));
      toast.success(`Saved ${saved.spec.title || saved.name}`);
      onSaved(saved);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  const targets =
    kind === 'agent'
      ? (agents.data ?? []).map((a) => ({ value: a.summary.name, label: a.summary.name, description: a.spec?.description ?? undefined }))
      : (workflows.data ?? []).map((w) => ({ value: w.name, label: w.name, description: w.description ?? undefined }));

  return (
    <div className="flex flex-col gap-4">
      <Section title="Basics">
        <TextField
          label="Title"
          value={spec.title ?? ''}
          placeholder="Summarize a page"
          onChange={(e) => {
            const title = e.target.value;
            const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
            set(isNew && (spec.name === '' || spec.name === slugOf(spec.title)) ? { title, name: slug } : { title });
          }}
        />
        <TextField
          label="Address"
          value={spec.name}
          disabled={!isNew}
          description={isNew ? 'Part of the link. Letters, digits, - and _.' : 'The link stays the same.'}
          onChange={(e) => set({ name: e.target.value.replace(/[^A-Za-z0-9_-]/g, '-') })}
        />
        <TextField
          className="col-span-full"
          label="Description"
          value={spec.description ?? ''}
          placeholder="Shown at the top of the form"
          onChange={(e) => set({ description: e.target.value || undefined })}
        />
        <TextField label="Button text" value={spec.submit_label ?? ''} placeholder="Run" onChange={(e) => set({ submit_label: e.target.value || undefined })} />
      </Section>

      <Section title="What it runs">
        <ToggleField
          label="It runs"
          options={[
            { value: 'agent', label: 'An assistant' },
            { value: 'workflow', label: 'A workflow' },
          ]}
          value={kind}
          onValueChange={(k) =>
            set(k === 'agent' ? { agent: '', workflow: undefined } : { workflow: '', agent: undefined, prompt: undefined })
          }
        />
        <SelectField
          label={kind === 'agent' ? 'Assistant' : 'Workflow'}
          options={targets}
          placeholder="Choose…"
          value={(kind === 'agent' ? spec.agent : spec.workflow) ?? ''}
          onValueChange={(v) => {
            if (kind === 'agent') set({ agent: v });
            else {
              set({ workflow: v });
              if (v && fields.length <= 1) fromWorkflow(v);
            }
          }}
        />
      </Section>

      <Card>
        <CardHeader>
          <CardTitle>What people fill in</CardTitle>
          <CardDescription>
            {kind === 'workflow'
              ? "Each field is one of the workflow's inputs. With no fields, the form asks for all of them."
              : 'The assistant gets these as its task (see below).'}
          </CardDescription>
          {kind === 'workflow' && spec.workflow && (
            <CardAction>
              <Button variant="ghost" size="sm" onClick={() => fromWorkflow(spec.workflow!)}>
                Use the workflow's inputs
              </Button>
            </CardAction>
          )}
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {fields.map((f, i) => (
            <FieldRow key={i} field={f} onChange={(x) => setField(i, x)} />
          ))}
          {fields.length === 0 && <FieldDescription>No fields.</FieldDescription>}
        </CardContent>
        <CardFooter>
          <Button
            variant="outline"
            size="sm"
            onClick={() => {
              let n = `field${fields.length + 1}`;
              while (fields.some((f) => f.name === n)) n += '_';
              set({ fields: [...fields, { name: n, label: '', type: 'text', required: true }] });
            }}
          >
            <IconPlus data-icon="inline-start" />
            Add a field
          </Button>
        </CardFooter>
      </Card>

      {kind === 'agent' && (
        <Section title="The task" hint="What the assistant is asked. Leave it empty to send the fields as they are.">
          <TextArea
            label="Task"
            rows={4}
            value={spec.prompt ?? ''}
            placeholder={fields.length ? `Summarize {{ ${fields[0].name} }} in three bullet points.` : ''}
            textareaRef={promptRef}
            onChange={(v) => set({ prompt: v || undefined })}
          />
          {fields.length > 0 && (
            <div className="col-span-full flex flex-wrap items-center gap-1.5">
              <span className="text-sm text-muted-foreground">Use:</span>
              {fields.map((f) => (
                <Button
                  key={f.name}
                  type="button"
                  variant="secondary"
                  size="xs"
                  onClick={() => set({ prompt: insertAt(promptRef.current, spec.prompt ?? '', `{{ ${f.name} }}`) })}
                >
                  {f.label || f.name}
                </Button>
              ))}
            </div>
          )}
        </Section>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Result</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <ToggleField
            aria-label="Kind of result"
            options={[
              { value: 'text', label: 'Text' },
              { value: 'json', label: 'Data (JSON)' },
            ]}
            value={spec.output_schema ? 'json' : 'text'}
            onValueChange={(v) => {
              if (v === 'text') {
                set({ output_schema: undefined });
                setSchemaText('');
              } else {
                const starter = { type: 'object', properties: { answer: { type: 'string' } }, required: ['answer'] };
                set({ output_schema: starter });
                setSchemaText(JSON.stringify(starter, null, 2));
              }
            }}
          />
          {spec.output_schema != null && (
            <Disclosure summary="The shape of the data (JSON Schema)" open>
              <TextArea
                label="Schema"
                mono
                rows={8}
                value={schemaText}
                error={schemaError}
                description={
                  kind === 'agent'
                    ? 'The assistant is asked to answer in this shape. Callers of the API get it in output.'
                    : "The workflow's result is checked against it."
                }
                onChange={(t) => {
                  setSchemaText(t);
                  try {
                    const v = JSON.parse(t);
                    if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error('A schema is a JSON object.');
                    setSchemaError(undefined);
                    set({ output_schema: v });
                  } catch (e) {
                    setSchemaError((e as Error).message);
                  }
                }}
              />
            </Disclosure>
          )}
        </CardContent>
      </Card>

      <ErrorNote error={error} />
      <div className="flex justify-end">
        <Button onClick={save} disabled={busy || !!schemaError || !spec.name || !(spec.agent || spec.workflow)}>
          {busy && <Spinner data-icon="inline-start" />}
          {isNew ? 'Create app' : 'Save changes'}
        </Button>
      </div>
    </div>
  );
}

const slugOf = (title?: string | null) =>
  (title ?? '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '');

/** Drop empty optional parts before saving. */
function clean(spec: AppSpec): AppSpec {
  return {
    ...spec,
    agent: spec.agent || undefined,
    workflow: spec.workflow || undefined,
    title: spec.title || undefined,
    fields: (spec.fields ?? []).map((f) => ({
      ...f,
      label: f.label || undefined,
      help: f.help || undefined,
      placeholder: f.placeholder || undefined,
      options: f.type === 'select' ? f.options : undefined,
      default: typedDefault(f),
    })),
  };
}

/** A default typed in as text, as its field's type. */
function typedDefault(f: AppField): unknown {
  if (f.default === '' || f.default == null) return undefined;
  if (f.type === 'number' && typeof f.default === 'string' && f.default.trim() !== '' && !isNaN(Number(f.default))) return Number(f.default);
  if (f.type === 'boolean' && typeof f.default === 'string') return ['true', 'yes', 'on'].includes(f.default.toLowerCase());
  if (f.type === 'list' && typeof f.default === 'string')
    return f.default
      .split(/[,\n]/)
      .map((x) => x.trim())
      .filter(Boolean);
  return f.default;
}

function FieldRow({ field: f, onChange }: { field: AppField; onChange: (f: AppField | null) => void }) {
  const set = (patch: Partial<AppField>) => onChange({ ...f, ...patch });
  return (
    <Card size="sm">
      <CardContent className="flex flex-col gap-3">
        <FieldGroup className="grid gap-3 sm:grid-cols-[1fr_1fr_11rem_auto]">
          <TextField
            label="Label"
            value={f.label ?? ''}
            placeholder="Web address"
            onChange={(e) => {
              const label = e.target.value;
              const auto = f.name === nameOf(f.label ?? '') || /^field\d+_*$/.test(f.name);
              set(auto && label ? { label, name: nameOf(label) } : { label });
            }}
          />
          <TextField
            label="Key"
            className="font-mono"
            value={f.name}
            description="The name in the API"
            onChange={(e) => set({ name: e.target.value.replace(/[^A-Za-z0-9_]/g, '_') })}
          />
          <SelectField
            label="Type"
            options={TYPES}
            value={f.type ?? 'text'}
            onValueChange={(v) => set({ type: v as AppField['type'], options: v === 'select' ? (f.options ?? ['yes', 'no']) : undefined })}
          />
          <Button variant="ghost" size="icon" className="self-end" aria-label={`Remove ${f.label || f.name}`} onClick={() => onChange(null)}>
            <IconTrash />
          </Button>
        </FieldGroup>
        {f.type === 'select' && (
          <TextField
            label="Choices"
            description="Separated by commas"
            value={(f.options ?? []).join(', ')}
            onChange={(e) => set({ options: e.target.value.split(',').map((o) => o.trim()).filter(Boolean) })}
          />
        )}
        <Disclosure summary="Help text, default, required" open={!!(f.help || f.default != null || f.required === false)}>
          <FieldGroup className="grid gap-3 sm:grid-cols-3">
            <TextField label="Help text" value={f.help ?? ''} onChange={(e) => set({ help: e.target.value || undefined })} />
            <TextField
              label="Default"
              value={f.default == null ? '' : Array.isArray(f.default) ? f.default.join(', ') : String(f.default)}
              description={f.type === 'list' ? 'Items separated by commas' : undefined}
              onChange={(e) => set({ default: e.target.value === '' ? undefined : e.target.value })}
            />
            <SwitchField
              label="Required"
              checked={f.required !== false}
              onCheckedChange={(required) => set({ required })}
            />
          </FieldGroup>
        </Disclosure>
      </CardContent>
    </Card>
  );
}

/** `Web address` → `web_address`. */
const nameOf = (label: string) =>
  label
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .replace(/^(\d)/, 'f$1') || 'field';
