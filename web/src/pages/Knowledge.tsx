import { useRef, useState } from 'react';
import { ChevronRightIcon, FilesIcon, SearchXIcon } from 'lucide-react';
import { toast } from 'sonner';
import { TabBar, TextField, ToggleField } from '@/components/fields';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { Field, FieldDescription, FieldGroup, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '@/components/ui/input-group';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { Textarea } from '@/components/ui/textarea';
import { api, fileToBase64, type Collection, type KnowledgeDocument, type SearchHit, type SearchMode } from '../api';
import { ConfirmAction, Disclosure, EmptyState, ErrorNote, LinkItem, Page, PageHeader } from '@/components/shared';
import { IconPlus, IconSearch, IconTrash, IconUpload } from '../icons';
import { navigate } from '../router';
import { ago, useLoad } from '../util';
import { useDevMode } from '../prefs';
import { ExtractPanel, GraphPanel } from './KnowledgeExtras';

/** Where dropped files go when no folder is open. */
const DEFAULT_FOLDER = 'my-documents';

export function Knowledge({ selected }: { selected?: string }) {
  const [dev] = useDevMode();
  const list = useLoad(() => api.collections(), []);
  const current = list.data?.find((c) => c.name === selected) ?? (selected ? undefined : list.data?.[0]);
  const [dragging, setDragging] = useState(false);
  const [adding, setAdding] = useState(0);
  const picker = useRef<HTMLInputElement>(null);

  // Files dropped anywhere go to the open folder, or to "my-documents".
  async function addFiles(files: FileList | File[]) {
    const list_ = Array.from(files);
    if (!list_.length) return;
    let folder = current?.name;
    try {
      if (!folder) {
        const all = await api.collections();
        if (!all.some((c) => c.name === DEFAULT_FOLDER)) {
          await api.createCollection({ name: DEFAULT_FOLDER, description: 'Documents you added.' });
        }
        folder = DEFAULT_FOLDER;
      }
    } catch (e) {
      toast.error((e as Error).message);
      return;
    }
    setAdding((n) => n + list_.length);
    for (const f of list_) {
      try {
        const r = await api.addDocument(folder, {
          filename: f.name,
          content_type: f.type || undefined,
          content_base64: await fileToBase64(f),
        });
        toast.success(r.unchanged ? `${f.name} was already there` : `Added ${f.name}`);
      } catch (e) {
        toast.error(`${f.name}: ${(e as Error).message}`);
      } finally {
        setAdding((n) => n - 1);
      }
    }
    list.reload();
    if (!current) navigate(`/knowledge/${encodeURIComponent(folder)}`);
  }

  return (
    <div
      className="relative"
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes('Files')) {
          e.preventDefault();
          setDragging(true);
        }
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node)) setDragging(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        addFiles(e.dataTransfer.files);
      }}
    >
      {dragging && (
        <Empty className="pointer-events-none absolute inset-2 z-10 border bg-background/90" aria-hidden="true">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <IconUpload />
            </EmptyMedia>
            <EmptyTitle>Drop to add to {current?.name ?? 'your documents'}</EmptyTitle>
          </EmptyHeader>
        </Empty>
      )}
      <input
        ref={picker}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          if (e.target.files) addFiles(e.target.files);
          e.target.value = '';
        }}
      />
      <Page wide>
        <PageHeader
          title="Knowledge"
          subtitle={
            <>
              Documents your assistants can search and quote. Drop files anywhere on this page, then pick the folder in an assistant's
              settings (or just ask in Chat).
            </>
          }
          actions={
            <Button onClick={() => picker.current?.click()} disabled={adding > 0}>
              {adding > 0 ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
              Add files
            </Button>
          }
        />
        <ErrorNote error={list.error} />
        <div className="grid items-start gap-4 lg:grid-cols-[19rem_minmax(0,1fr)]">
          <div className="flex flex-col gap-3">
            <ItemGroup className="gap-2">
              {list.data?.map((c) => (
                <LinkItem key={c.name} href={`#/knowledge/${encodeURIComponent(c.name)}`} active={current?.name === c.name} title={c.name}>
                  <ItemDescription>
                    {c.documents} document{c.documents === 1 ? '' : 's'}
                    {dev && ` · ${c.chunks} chunks`}
                  </ItemDescription>
                  {dev && <ItemDescription className="font-mono">{c.embedding_model}</ItemDescription>}
                </LinkItem>
              ))}
            </ItemGroup>
            {list.data?.length === 0 && (
              <Empty className="border">
                <EmptyHeader>
                  <EmptyMedia variant="icon">
                    <FilesIcon />
                  </EmptyMedia>
                  <EmptyTitle>Drop files here</EmptyTitle>
                  <EmptyDescription>PDF, text, Markdown or HTML.</EmptyDescription>
                </EmptyHeader>
                <EmptyContent>
                  <Button variant="outline" onClick={() => picker.current?.click()}>
                    Choose files
                  </Button>
                </EmptyContent>
              </Empty>
            )}
            <Card>
              <CardContent>
                <Disclosure summary="New folder">
                  <NewCollection
                    onCreated={(name) => {
                      list.reload();
                      navigate(`/knowledge/${encodeURIComponent(name)}`);
                    }}
                  />
                </Disclosure>
              </CardContent>
            </Card>
          </div>
          {current ? (
            <CollectionView key={current.name} c={current} onChange={list.reload} />
          ) : (
            list.data && list.data.length > 0 && <EmptyState title="Pick a folder" />
          )}
        </div>
      </Page>
    </div>
  );
}

function NewCollection({ onCreated }: { onCreated: (name: string) => void }) {
  const [dev] = useDevMode();
  const [name, setName] = useState('');
  const [model, setModel] = useState('');
  const [description, setDescription] = useState('');
  const [busy, setBusy] = useState(false);
  async function create() {
    setBusy(true);
    try {
      const c = await api.createCollection({
        name: name.trim(),
        description: description.trim() || undefined,
        embedding_model: model.trim() || undefined,
      });
      setName('');
      setModel('');
      setDescription('');
      onCreated(c.name);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <FieldGroup className="pt-2">
      <TextField label="Name" placeholder="handbook" value={name} onChange={(e) => setName(e.target.value)} />
      <TextField label="Description" value={description} onChange={(e) => setDescription(e.target.value)} />
      {dev && (
        <TextField
          label="Embedding model"
          description="Leave empty for the configured default. Fixed for the collection's life."
          placeholder="openai/text-embedding-3-small"
          value={model}
          onChange={(e) => setModel(e.target.value)}
        />
      )}
      <Button className="w-fit" onClick={create} disabled={busy || !name.trim()}>
        {busy ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
        Create
      </Button>
    </FieldGroup>
  );
}

function CollectionView({ c, onChange }: { c: Collection; onChange: () => void }) {
  const [tab, setTab] = useState('documents');
  const docs = useLoad(() => api.documents(c.name), [c.name]);
  const refresh = () => {
    docs.reload();
    onChange();
  };

  async function remove() {
    try {
      await api.deleteCollection(c.name);
      navigate('/knowledge');
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">{c.name}</CardTitle>
          {c.description && <CardDescription>{c.description}</CardDescription>}
          <CardAction>
            <ConfirmAction
              title={`Delete ${c.name}?`}
              description={`Its ${c.documents} documents are deleted too. Assistants that search it lose it.`}
              action="Delete"
              destructive
              onConfirm={remove}
            >
              <Button variant="ghost" size="sm">
                <IconTrash data-icon="inline-start" />
                Delete
              </Button>
            </ConfirmAction>
          </CardAction>
        </CardHeader>
        <CardContent className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
          <Badge variant="outline" className="font-mono">
            {c.embedding_model}
          </Badge>
          {c.dimensions != null && <Badge variant="outline">{c.dimensions} dims</Badge>}
          <Badge variant="outline">
            Chunks of ~{c.chunk_tokens} tokens, {c.chunk_overlap} overlap
          </Badge>
          <span>Updated {ago(c.updated_at)}</span>
        </CardContent>
      </Card>
      <TabBar
        items={[
          { id: 'documents', label: 'Documents', badge: <Badge variant="secondary">{c.documents}</Badge> },
          { id: 'search', label: 'Search' },
          { id: 'table', label: 'Table' },
          { id: 'graph', label: 'Graph' },
        ]}
        value={tab}
        onValueChange={setTab}
        variant="underline"
        aria-label="Collection view"
      />
      {tab === 'documents' ? (
        <>
          <AddDocument collection={c.name} onAdded={refresh} />
          <ErrorNote error={docs.error} />
          {docs.data?.length === 0 ? (
            <EmptyState title="No documents">Add text, a file or a web page above.</EmptyState>
          ) : (
            <ItemGroup className="gap-2">
              {docs.data?.map((d) => (
                <DocumentRow key={d.id} collection={c.name} d={d} onDeleted={refresh} />
              ))}
            </ItemGroup>
          )}
        </>
      ) : tab === 'table' ? (
        <ExtractPanel collection={c.name} />
      ) : tab === 'graph' ? (
        <GraphPanel collection={c.name} />
      ) : (
        <SearchPanel collection={c.name} />
      )}
    </div>
  );
}

function AddDocument({ collection, onAdded }: { collection: string; onAdded: () => void }) {
  const [kind, setKind] = useState('file');
  const [text, setText] = useState('');
  const [title, setTitle] = useState('');
  const [source, setSource] = useState('');
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  async function add(body: Parameters<typeof api.addDocument>[1], label: string) {
    const r = await api.addDocument(collection, body);
    toast.success(r.unchanged ? `${label}: already up to date` : `${label}: ${r.document.chunks} chunks`);
  }

  async function submit() {
    setBusy(true);
    try {
      if (kind === 'text') {
        await add({ text, title: title.trim() || undefined, source: source.trim() || undefined }, title.trim() || 'Text');
        setText('');
        setTitle('');
        setSource('');
      } else if (kind === 'url') {
        await add({ url: url.trim() }, url.trim());
        setUrl('');
      }
      onAdded();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function upload(files: FileList | null) {
    if (!files?.length) return;
    setBusy(true);
    for (const f of Array.from(files)) {
      try {
        await add({ filename: f.name, content_type: f.type || undefined, content_base64: await fileToBase64(f) }, f.name);
      } catch (e) {
        toast.error(`${f.name}: ${(e as Error).message}`);
      }
    }
    setBusy(false);
    if (fileInput.current) fileInput.current.value = '';
    onAdded();
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Add documents</CardTitle>
        <CardAction>
          <TabBar
            items={[
              { id: 'file', label: 'Files' },
              { id: 'text', label: 'Text' },
              { id: 'url', label: 'Web page' },
            ]}
            value={kind}
            onValueChange={setKind}
            variant="pill"
            aria-label="Document source"
          />
        </CardAction>
      </CardHeader>
      <CardContent>
        {kind === 'file' && (
          <Field>
            <FieldLabel htmlFor="doc-files">{busy ? 'Adding…' : 'Choose files'}</FieldLabel>
            <Input
              id="doc-files"
              ref={fileInput}
              type="file"
              multiple
              accept=".txt,.md,.markdown,.html,.htm,.pdf,.csv,.json,.yaml,.yml,.rst"
              disabled={busy}
              onChange={(e) => upload(e.target.files)}
            />
            <FieldDescription>Text, Markdown, HTML or PDF. Uploading a file with the same name again replaces it.</FieldDescription>
          </Field>
        )}
        {kind === 'text' && (
          <FieldGroup>
            <div className="grid gap-4 sm:grid-cols-2">
              <TextField label="Title" value={title} onChange={(e) => setTitle(e.target.value)} />
              <TextField
                label="Source"
                description="Re-adding the same source replaces it."
                placeholder="faq/refunds"
                value={source}
                onChange={(e) => setSource(e.target.value)}
              />
            </div>
            <Textarea
              className="min-h-28"
              aria-label="Text"
              placeholder="Paste text or Markdown…"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
            <Button className="w-fit" onClick={submit} disabled={busy || !text.trim()}>
              {busy ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
              Add text
            </Button>
          </FieldGroup>
        )}
        {kind === 'url' && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              submit();
            }}
          >
            <InputGroup>
              <InputGroupInput
                aria-label="URL"
                placeholder="https://example.com/guide"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
              />
              <InputGroupAddon align="inline-end">
                <InputGroupButton type="submit" variant="default" disabled={busy || !/^https?:\/\//.test(url.trim())}>
                  {busy ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
                  Fetch and add
                </InputGroupButton>
              </InputGroupAddon>
            </InputGroup>
          </form>
        )}
      </CardContent>
    </Card>
  );
}

function DocumentRow({ collection, d, onDeleted }: { collection: string; d: KnowledgeDocument; onDeleted: () => void }) {
  const [open, setOpen] = useState(false);
  const view = useLoad(() => (open ? api.document(collection, d.id) : Promise.resolve(undefined)), [open, d.id]);
  async function remove() {
    try {
      await api.deleteDocument(collection, d.id);
      onDeleted();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }
  const format = (d.metadata as { format?: string } | null)?.format;
  return (
    <Collapsible open={open} onOpenChange={setOpen} asChild>
      <Item variant="outline" size="sm">
        <ItemContent className="min-w-0">
          <ItemTitle className="w-full">
            <CollapsibleTrigger asChild>
              <Button variant="ghost" size="sm" className="group/doc -ml-2 min-w-0 justify-start">
                <ChevronRightIcon data-icon="inline-start" className="transition-transform group-data-[state=open]/doc:rotate-90" />
                <span className="truncate">{d.title ?? d.source}</span>
              </Button>
            </CollapsibleTrigger>
            {format && <Badge variant="outline">{format}</Badge>}
          </ItemTitle>
          <ItemDescription className="truncate font-mono">{d.source}</ItemDescription>
        </ItemContent>
        <ItemActions>
          <span className="text-sm text-muted-foreground tabular-nums">
            {d.chunks} chunks · {ago(d.created_at)}
          </span>
          <ConfirmAction
            title={`Delete ${d.title ?? d.source}?`}
            description="Assistants can no longer find it."
            action="Delete"
            destructive
            onConfirm={remove}
          >
            <Button variant="ghost" size="icon-sm" aria-label={`Delete ${d.source}`}>
              <IconTrash />
            </Button>
          </ConfirmAction>
        </ItemActions>
        <CollapsibleContent className="basis-full">
          <ItemGroup className="gap-2 pt-2">
            {view.data?.chunks.map((c) => (
              <Item key={c.id} variant="muted" size="xs">
                <ItemContent>
                  <ItemDescription>
                    #{c.seq}
                    {(c.metadata as { heading?: string } | null)?.heading && ` · ${(c.metadata as { heading: string }).heading}`}
                  </ItemDescription>
                  <p className="text-sm whitespace-pre-wrap">{c.text}</p>
                </ItemContent>
              </Item>
            ))}
            {view.loading && <Spinner />}
          </ItemGroup>
        </CollapsibleContent>
      </Item>
    </Collapsible>
  );
}

const MODES: { id: SearchMode; label: string }[] = [
  { id: 'hybrid', label: 'Hybrid' },
  { id: 'vector', label: 'Meaning' },
  { id: 'text', label: 'Keywords' },
  { id: 'graph', label: 'With the graph' },
];

function SearchPanel({ collection }: { collection: string }) {
  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<SearchMode>('hybrid');
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [busy, setBusy] = useState(false);
  async function run(m = mode) {
    if (!query.trim()) return;
    setBusy(true);
    try {
      setHits(await api.search(collection, query, m));
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Card>
      <CardContent className="flex flex-col gap-4">
        <form
          className="flex flex-wrap items-end gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            run();
          }}
        >
          <InputGroup className="min-w-64 flex-1">
            <InputGroupInput
              aria-label="Search query"
              placeholder="What does an agent need to find?"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <InputGroupAddon align="inline-end">
              <InputGroupButton type="submit" variant="default" disabled={busy || !query.trim()}>
                {busy ? <Spinner data-icon="inline-start" /> : <IconSearch data-icon="inline-start" />}
                Search
              </InputGroupButton>
            </InputGroupAddon>
          </InputGroup>
          <ToggleField
            className="w-auto"
            aria-label="Search mode"
            options={MODES.map((m) => ({ value: m.id, label: m.label }))}
            value={mode}
            onValueChange={(m) => {
              setMode(m as SearchMode);
              run(m as SearchMode);
            }}
          />
        </form>
        {hits?.length === 0 && <EmptyState title="No results" icon={<SearchXIcon />} />}
        <ItemGroup className="gap-2">
          {hits?.map((h, i) => (
            <Item key={h.chunk_id} variant="outline">
              <ItemContent className="min-w-0">
                <ItemTitle className="flex-wrap">
                  <Badge variant="secondary">{i + 1}</Badge>
                  {h.title ?? h.source}
                  {h.heading && <span className="font-normal text-muted-foreground">› {h.heading}</span>}
                </ItemTitle>
                <p className="text-sm whitespace-pre-wrap">{h.text}</p>
                <ItemDescription className="font-mono">{h.source}</ItemDescription>
              </ItemContent>
              <ItemActions className="flex-col items-end">
                {h.vector_score != null && (
                  <Badge variant="outline" className="font-mono">
                    cos {h.vector_score.toFixed(3)}
                  </Badge>
                )}
                {h.text_score != null && (
                  <Badge variant="outline" className="font-mono">
                    bm25 {h.text_score.toFixed(2)}
                  </Badge>
                )}
              </ItemActions>
            </Item>
          ))}
        </ItemGroup>
      </CardContent>
    </Card>
  );
}
