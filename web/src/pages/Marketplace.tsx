// The marketplace: bundles of assistants, workflows, prompts and the rest,
// published as rated listings. This server's own listings, and those of
// other gai servers added as registries, are browsed and installed here.

import { useEffect, useState } from 'react';
import { PackageIcon, StarIcon } from 'lucide-react';
import { toast } from 'sonner';
import { CheckboxField, SwitchField, TabBar, TextField } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldDescription, FieldGroup } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet';
import { Spinner } from '@/components/ui/spinner';
import { cn } from '@/lib/utils';
import { api, type BundleReport, type Listing, type Registry } from '../api';
import { TextArea } from '../forms';
import { IconPlus, IconTrash } from '../icons';
import { ago, useLoad } from '../util';

const LOCAL = 'local';

const KINDS: [keyof Listing['manifest']['contents'], string, string][] = [
  ['agents', 'assistant', 'assistants'],
  ['workflows', 'workflow', 'workflows'],
  ['prompts', 'prompt', 'prompts'],
  ['personas', 'persona', 'personas'],
  ['skills', 'skill', 'skills'],
  ['mcp_servers', 'connected app', 'connected apps'],
  ['knowledge', 'knowledge collection', 'knowledge collections'],
];

/** "2 assistants, 1 prompt". */
function holds(l: Listing): string {
  return KINDS.map(([k, one, many]) => {
    const n = ((l.manifest.contents[k] as string[] | undefined) ?? []).length;
    return n ? `${n} ${n === 1 ? one : many}` : '';
  })
    .filter(Boolean)
    .join(', ');
}

function Stars({ value, count }: { value?: number | null; count: number }) {
  if (!count) return <span className="text-xs text-muted-foreground">No ratings yet</span>;
  return (
    <span className="flex items-center gap-1 text-xs text-muted-foreground" title={`${value?.toFixed(1)} of 5 from ${count}`}>
      <StarIcon className="size-3.5 fill-warning text-warning" />
      {value?.toFixed(1)} ({count})
    </span>
  );
}

export function Marketplace() {
  const registries = useLoad(() => api.registries(), []);
  const [registry, setRegistry] = useState(LOCAL);
  const [q, setQ] = useState('');
  const [query, setQuery] = useState('');
  const list = useLoad(() => api.browse(registry, query), [registry, query]);
  const [open, setOpen] = useState<Listing>();
  const [publishing, setPublishing] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setQuery(q.trim()), 300);
    return () => clearTimeout(t);
  }, [q]);

  return (
    <Page wide>
      <PageHeader
        title="Marketplace"
        subtitle="Ready-made assistants, workflows and prompts, shared as bundles. Install one in a click; it never brings keys or tokens with it."
        actions={
          <Button onClick={() => setPublishing(true)}>
            <IconPlus data-icon="inline-start" />
            Publish
          </Button>
        }
      />
      <div className="flex flex-wrap items-center gap-3">
        <TabBar
          items={[{ id: LOCAL, label: 'This server' }, ...(registries.data ?? []).map((r) => ({ id: r.name, label: r.name }))]}
          value={registry}
          onValueChange={setRegistry}
          aria-label="Registry"
        />
        <Input className="ml-auto max-w-72" placeholder="Search…" aria-label="Search listings" value={q} onChange={(e) => setQ(e.target.value)} />
      </div>
      <ErrorNote error={list.error} />
      {list.loading && !list.data && <Spinner />}
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {list.data?.map((l) => (
          <Card key={`${l.registry}/${l.name}`} className="cursor-pointer transition-colors hover:border-primary" onClick={() => setOpen(l)}>
            <CardHeader>
              <CardTitle>{l.title}</CardTitle>
              {l.description && <CardDescription className="line-clamp-2">{l.description}</CardDescription>}
            </CardHeader>
            <CardContent className="flex flex-col gap-2">
              <span className="text-xs text-muted-foreground">{holds(l) || 'Empty'}</span>
              {l.tags.length > 0 && (
                <div className="flex flex-wrap gap-1">
                  {l.tags.map((t) => (
                    <Badge key={t} variant="secondary">
                      {t}
                    </Badge>
                  ))}
                </div>
              )}
            </CardContent>
            <CardFooter className="justify-between">
              <Stars value={l.rating} count={l.ratings} />
              <span className="text-xs text-muted-foreground">
                v{l.version} · {ago(l.updated_at)}
              </span>
            </CardFooter>
          </Card>
        ))}
      </div>
      {list.data?.length === 0 && (
        <EmptyState title={query ? 'Nothing matches' : 'Nothing published yet'} icon={<PackageIcon />}>
          {registry === LOCAL
            ? 'Publish assistants from this server, or add another gai server below to install from it.'
            : 'This registry has no listings yet.'}
        </EmptyState>
      )}
      <Registries
        onChange={() => {
          registries.reload();
          list.reload();
        }}
      />
      {open && (
        <ListingSheet
          listing={open}
          onClose={() => setOpen(undefined)}
          onChanged={() => {
            list.reload();
          }}
        />
      )}
      <PublishSheet
        open={publishing}
        onClose={() => setPublishing(false)}
        onPublished={() => {
          setRegistry(LOCAL);
          list.reload();
        }}
      />
    </Page>
  );
}

function ListingSheet({ listing, onClose, onChanged }: { listing: Listing; onClose: () => void; onChanged: () => void }) {
  const detail = useLoad(() => api.listing(listing.registry, listing.name), [listing.registry, listing.name]);
  const [stars, setStars] = useState(0);
  const [comment, setComment] = useState('');
  const [skip, setSkip] = useState(false);
  const [peek, setPeek] = useState<BundleReport>();
  const [done, setDone] = useState<BundleReport>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const l = detail.data?.listing ?? listing;
  const report = done ?? peek;

  async function install(dry: boolean) {
    setBusy(true);
    setError(undefined);
    try {
      const r = await api.installListing({ registry: l.registry, name: l.name, skip_existing: skip, dry_run: dry });
      if (dry) setPeek(r);
      else {
        setDone(r);
        toast.success(`Installed ${l.title}`);
      }
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function rate() {
    try {
      await api.rateListing(l.registry, l.name, stars, comment);
      toast.success('Thanks for rating');
      setComment('');
      detail.reload();
      onChanged();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <Sheet open onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="flex w-full flex-col overflow-y-auto sm:max-w-lg">
        <SheetHeader>
          <SheetTitle>{l.title}</SheetTitle>
          <SheetDescription>
            {l.description ?? 'No description.'} {l.author ? `By ${l.author}.` : ''}
          </SheetDescription>
        </SheetHeader>
        <div className="flex flex-col gap-4 px-4">
          <div className="flex flex-wrap items-center gap-3">
            <Stars value={l.rating} count={l.ratings} />
            <span className="text-xs text-muted-foreground">
              v{l.version} · {Math.max(1, Math.round(l.size / 1024))} KB · {l.registry === LOCAL ? 'this server' : l.registry}
            </span>
          </div>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            {KINDS.filter(([k]) => ((l.manifest.contents[k] as string[] | undefined) ?? []).length).map(([k, , many]) => (
              <div key={k} className="contents">
                <dt className="text-muted-foreground capitalize">{many}</dt>
                <dd>{(l.manifest.contents[k] as string[]).join(', ')}</dd>
              </div>
            ))}
            {l.manifest.requires.models.length > 0 && (
              <div className="contents">
                <dt className="text-muted-foreground">Models</dt>
                <dd>{l.manifest.requires.models.join(', ')}</dd>
              </div>
            )}
          </dl>

          {report && report.missing_env.length > 0 && (
            <Alert>
              <AlertTitle>Needs keys this server doesn’t have</AlertTitle>
              <AlertDescription>
                <ul className="font-mono text-xs">
                  {report.missing_env.map((v) => (
                    <li key={v.name}>
                      {v.name} <span className="font-sans text-muted-foreground">· {v.used_by}</span>
                    </li>
                  ))}
                </ul>
              </AlertDescription>
            </Alert>
          )}
          {report && report.missing_models.length > 0 && (
            <Alert>
              <AlertTitle>Some AI models aren’t connected here</AlertTitle>
              <AlertDescription>{report.missing_models.join(', ')}. Connect them in Settings, or pick another model after installing.</AlertDescription>
            </Alert>
          )}
          {peek && !done && report?.missing_env.length === 0 && report.missing_models.length === 0 && (
            <FieldDescription>Everything it needs is here.</FieldDescription>
          )}
          {done && (
            <ul className="flex flex-col gap-0.5 text-sm">
              {done.installed.map((i) => (
                <li key={i}>+ {i}</li>
              ))}
              {done.skipped.map((i) => (
                <li key={i} className="text-muted-foreground">
                  = {i} (already here)
                </li>
              ))}
            </ul>
          )}
          <ErrorNote error={error} />
          {!done && (
            <SwitchField
              label="Keep what’s already here"
              description="Otherwise items with the same name get a new version; the old ones stay in their history."
              checked={skip}
              onCheckedChange={setSkip}
            />
          )}

          <Separator />
          <div className="flex flex-col gap-2">
            <span className="text-sm font-medium">Your rating</span>
            <div className="flex gap-1" role="radiogroup" aria-label="Stars">
              {[1, 2, 3, 4, 5].map((n) => (
                <Button
                  key={n}
                  variant="ghost"
                  size="icon-sm"
                  role="radio"
                  aria-checked={stars === n}
                  aria-label={`${n} star${n > 1 ? 's' : ''}`}
                  onClick={() => setStars(n)}
                >
                  <StarIcon className={cn('size-4', n <= stars ? 'fill-warning text-warning' : 'text-muted-foreground')} />
                </Button>
              ))}
            </div>
            <Input placeholder="A comment (optional)" aria-label="Comment" value={comment} onChange={(e) => setComment(e.target.value)} />
            <Button variant="outline" size="sm" className="self-start" disabled={!stars} onClick={rate}>
              Rate
            </Button>
          </div>
          {(detail.data?.reviews.length ?? 0) > 0 && (
            <div className="flex flex-col gap-2">
              <span className="text-sm font-medium">Reviews</span>
              {detail.data?.reviews.map((r) => (
                <div key={r.rater} className="text-sm">
                  <span className="text-warning">{'★'.repeat(r.stars)}</span>
                  <span className="text-muted-foreground">{'★'.repeat(5 - r.stars)}</span> {r.comment}
                  <span className="text-xs text-muted-foreground">
                    {' '}
                    · {r.rater}, {ago(r.created_at)}
                  </span>
                </div>
              ))}
            </div>
          )}
          {(detail.data?.versions.length ?? 0) > 0 && (
            <div className="flex flex-col gap-1">
              <span className="text-sm font-medium">Versions</span>
              {detail.data?.versions.map((v) => (
                <span key={v.version} className="text-sm">
                  v{v.version} <span className="text-muted-foreground">· {ago(v.created_at)}</span> {v.note}
                </span>
              ))}
            </div>
          )}
        </div>
        <SheetFooter className="flex-row flex-wrap gap-2">
          {done ? (
            <Button onClick={onClose}>Done</Button>
          ) : (
            <>
              <Button variant="outline" onClick={() => install(true)} disabled={busy}>
                Check what it needs
              </Button>
              <Button onClick={() => install(false)} disabled={busy}>
                {busy && <Spinner data-icon="inline-start" />}
                Install
              </Button>
            </>
          )}
          {l.registry === LOCAL && !done && (
            <ConfirmAction
              title={`Unpublish ${l.title}?`}
              description="It disappears from the marketplace, with every version. What people already installed stays with them."
              action="Unpublish"
              destructive
              onConfirm={async () => {
                try {
                  await api.unpublishListing(l.name);
                  onChanged();
                  onClose();
                } catch (e) {
                  toast.error((e as Error).message);
                }
              }}
            >
              <Button variant="ghost" className="ml-auto">
                <IconTrash data-icon="inline-start" />
                Unpublish
              </Button>
            </ConfirmAction>
          )}
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}

function PublishSheet({ open, onClose, onPublished }: { open: boolean; onClose: () => void; onPublished: () => void }) {
  const agents = useLoad(() => (open ? api.agents() : Promise.resolve([])), [open]);
  const workflows = useLoad(() => (open ? api.workflows() : Promise.resolve([])), [open]);
  const prompts = useLoad(() => (open ? api.prompts() : Promise.resolve([])), [open]);
  const [title, setTitle] = useState('');
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [tags, setTags] = useState('');
  const [note, setNote] = useState('');
  const [picked, setPicked] = useState<Record<'agents' | 'workflows' | 'prompts', string[]>>({ agents: [], workflows: [], prompts: [] });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const slug = (t: string) =>
    t
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '');
  const toggle = (k: keyof typeof picked, v: string, on: boolean) =>
    setPicked((p) => ({ ...p, [k]: on ? [...p[k], v] : p[k].filter((x) => x !== v) }));
  const count = picked.agents.length + picked.workflows.length + picked.prompts.length;

  async function publish() {
    setBusy(true);
    setError(undefined);
    try {
      const l = await api.publishListing({
        name: name || slug(title),
        title,
        description: description || null,
        tags: tags
          .split(',')
          .map((t) => t.trim())
          .filter(Boolean),
        note: note || null,
        items: { ...picked },
      } as Parameters<typeof api.publishListing>[0]);
      toast.success(`Published ${l.title} v${l.version}`);
      onPublished();
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  const group = (k: keyof typeof picked, label: string, names: string[]) =>
    names.length > 0 && (
      <div className="flex flex-col gap-2">
        <span className="text-sm font-medium">{label}</span>
        {names.map((n) => (
          <CheckboxField key={n} label={n} checked={picked[k].includes(n)} onCheckedChange={(on) => toggle(k, n, on)} />
        ))}
      </div>
    );

  return (
    <Sheet open={open} onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="flex w-full flex-col overflow-y-auto sm:max-w-lg">
        <SheetHeader>
          <SheetTitle>Publish to the marketplace</SheetTitle>
          <SheetDescription>
            What you pick is bundled with what it uses (prompts, personas, skills, knowledge, connected apps), without keys or tokens.
            Publishing under the same name again makes a new version.
          </SheetDescription>
        </SheetHeader>
        <FieldGroup className="flex flex-col gap-4 px-4">
          <TextField
            label="Title"
            value={title}
            onChange={(e) => {
              if (!name || name === slug(title)) setName(slug(e.target.value));
              setTitle(e.target.value);
            }}
          />
          <TextField label="Name" description="Letters, digits, -, _ and ." value={name} onChange={(e) => setName(e.target.value)} />
          <TextArea label="Description" rows={3} value={description} onChange={setDescription} />
          <TextField label="Tags" description="Separated by commas" value={tags} onChange={(e) => setTags(e.target.value)} />
          <TextField label="What's new" value={note} onChange={(e) => setNote(e.target.value)} />
          {group('agents', 'Assistants', (agents.data ?? []).map((a) => a.summary.name))}
          {group('workflows', 'Workflows', (workflows.data ?? []).filter((w) => w.source === 'stored').map((w) => w.name))}
          {group('prompts', 'Prompts', (prompts.data ?? []).map((p) => p.summary.name))}
          <ErrorNote error={error} />
        </FieldGroup>
        <SheetFooter>
          <Button onClick={publish} disabled={busy || !title.trim() || count === 0}>
            {busy && <Spinner data-icon="inline-start" />}
            Publish {count > 0 ? `(${count})` : ''}
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}

/** Other gai servers to install from. */
function Registries({ onChange }: { onChange: () => void }) {
  const list = useLoad(() => api.registries(), []);
  const [draft, setDraft] = useState<Registry>({ name: '', url: '', token: '' });
  const [error, setError] = useState<string>();

  async function save(next: Registry[]) {
    setError(undefined);
    try {
      await api.saveRegistries(next);
      list.reload();
      onChange();
      return true;
    } catch (e) {
      setError((e as Error).message);
      return false;
    }
  }
  // Saved registries go back without tokens, which keeps theirs.
  const current = (list.data ?? []).map((r) => ({ name: r.name, url: r.url, token: null }));

  return (
    <Card>
      <CardHeader>
        <CardTitle>Registries</CardTitle>
        <CardDescription>
          Other gai servers whose marketplace you can install from. Give each a key made on that server: read access to browse and
          install, run access to rate.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        {list.data?.map((r) => (
          <div key={r.name} className="flex items-center gap-2 text-sm">
            <span className="font-medium">{r.name}</span>
            <span className="truncate text-muted-foreground">{r.url}</span>
            {!r.has_token && <Badge variant="outline">no key</Badge>}
            <Button
              variant="ghost"
              size="icon-sm"
              className="ml-auto"
              aria-label={`Remove ${r.name}`}
              onClick={() => save(current.filter((x) => x.name !== r.name))}
            >
              <IconTrash />
            </Button>
          </div>
        ))}
        <div className="flex flex-wrap gap-2">
          <Input className="w-32" placeholder="Name" aria-label="Registry name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
          <Input
            className="min-w-56 flex-1"
            placeholder="https://gai.example.com"
            aria-label="Registry address"
            value={draft.url}
            onChange={(e) => setDraft({ ...draft, url: e.target.value })}
          />
          <Input
            className="w-56"
            type="password"
            placeholder="Key for it"
            aria-label="Registry key"
            value={draft.token ?? ''}
            onChange={(e) => setDraft({ ...draft, token: e.target.value })}
          />
          <Button
            variant="outline"
            disabled={!draft.name || !draft.url}
            onClick={async () => {
              if (await save([...current, draft])) setDraft({ name: '', url: '', token: '' });
            }}
          >
            <IconPlus data-icon="inline-start" />
            Add
          </Button>
        </div>
        <ErrorNote error={error} />
      </CardContent>
    </Card>
  );
}
