import { useRef, useState } from 'react';
import { Button, Tabs, TextField, toast } from '@flame';
import { api, fileToBase64, type Collection, type KnowledgeDocument, type SearchHit, type SearchMode } from '../api';
import { Disclosure, Empty, ErrorNote, PageHeader } from '../components';
import { IconPlus, IconSearch, IconTrash } from '../icons';
import { navigate } from '../router';
import { ago, useLoad } from '../util';

export function Knowledge({ selected }: { selected?: string }) {
  const list = useLoad(() => api.collections(), []);
  const current = list.data?.find((c) => c.name === selected) ?? (selected ? undefined : list.data?.[0]);

  return (
    <div className="page">
      <PageHeader
        title="Knowledge"
        subtitle={
          <>
            Collections of documents agents search with <span className="mono">rag_search</span>. Give an agent{' '}
            <span className="mono">knowledge: [name]</span> to use one. Search combines meaning (embeddings) and keywords.
          </>
        }
      />
      <ErrorNote error={list.error} />
      <div className="agents-layout">
        <aside className="stack">
          <div className="agent-list">
            {list.data?.map((c) => (
              <a
                key={c.name}
                href={`#/knowledge/${encodeURIComponent(c.name)}`}
                className="agent-card"
                aria-current={current?.name === c.name ? 'page' : undefined}
                onClick={(e) => {
                  e.preventDefault();
                  navigate(`/knowledge/${encodeURIComponent(c.name)}`);
                }}
              >
                <strong>{c.name}</strong>
                <span className="muted small">
                  {c.documents} document{c.documents === 1 ? '' : 's'} · {c.chunks} chunks
                </span>
                <span className="muted small mono">{c.embedding_model}</span>
              </a>
            ))}
            {list.data?.length === 0 && <Empty title="No collections yet">Create one below, or run gai ingest ./docs -c handbook.</Empty>}
          </div>
          <section className="card">
            <Disclosure summary={<strong>New collection</strong>} open={list.data?.length === 0}>
              <NewCollection
                onCreated={(name) => {
                  list.reload();
                  navigate(`/knowledge/${encodeURIComponent(name)}`);
                }}
              />
            </Disclosure>
          </section>
        </aside>
        {current ? (
          <CollectionView key={current.name} c={current} onChange={list.reload} />
        ) : (
          list.data && list.data.length > 0 && <Empty title="Pick a collection" />
        )}
      </div>
    </div>
  );
}

function NewCollection({ onCreated }: { onCreated: (name: string) => void }) {
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
    <div className="stack" style={{ paddingTop: 8 }}>
      <TextField label="Name" placeholder="handbook" value={name} onChange={(e) => setName(e.target.value)} />
      <TextField label="Description" value={description} onChange={(e) => setDescription(e.target.value)} />
      <TextField
        label="Embedding model"
        description="Leave empty for the configured default. Fixed for the collection's life."
        placeholder="openai/text-embedding-3-small"
        value={model}
        onChange={(e) => setModel(e.target.value)}
      />
      <Button tone="accent" leading={<IconPlus />} loading={busy} disabled={!name.trim()} onClick={create}>
        Create
      </Button>
    </div>
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
    if (!confirm(`Delete ${c.name} and its ${c.documents} documents?`)) return;
    try {
      await api.deleteCollection(c.name);
      navigate('/knowledge');
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <section className="stack">
      <div className="card">
        <div className="row between">
          <div>
            <h2 style={{ margin: 0 }}>{c.name}</h2>
            {c.description && <p className="muted" style={{ margin: '4px 0 0' }}>{c.description}</p>}
          </div>
          <Button size="sm" variant="ghost" leading={<IconTrash />} onClick={remove}>
            Delete
          </Button>
        </div>
        <div className="row gap wrap small muted" style={{ marginTop: 10 }}>
          <span className="chip mono">{c.embedding_model}</span>
          {c.dimensions != null && <span className="chip">{c.dimensions} dims</span>}
          <span className="chip">
            chunks of ~{c.chunk_tokens} tokens, {c.chunk_overlap} overlap
          </span>
          <span>updated {ago(c.updated_at)}</span>
        </div>
      </div>
      <Tabs
        items={[
          { id: 'documents', label: 'Documents', badge: c.documents },
          { id: 'search', label: 'Search' },
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
          <div className="card flush">
            {docs.data?.map((d) => (
              <DocumentRow key={d.id} collection={c.name} d={d} onDeleted={refresh} />
            ))}
            {docs.data?.length === 0 && <Empty title="No documents">Add text, a file or a web page above.</Empty>}
          </div>
        </>
      ) : (
        <SearchPanel collection={c.name} />
      )}
    </section>
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
    <section className="card stack">
      <div className="row between">
        <h3 style={{ margin: 0 }}>Add documents</h3>
        <Tabs
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
      </div>
      {kind === 'file' && (
        <label className="dropzone">
          <input ref={fileInput} type="file" multiple accept=".txt,.md,.markdown,.html,.htm,.pdf,.csv,.json,.yaml,.yml,.rst" onChange={(e) => upload(e.target.files)} />
          <strong>{busy ? 'Adding…' : 'Choose files'}</strong>
          <span className="muted small">Text, Markdown, HTML or PDF. Uploading a file with the same name again replaces it.</span>
        </label>
      )}
      {kind === 'text' && (
        <>
          <div className="mcp-form" style={{ paddingTop: 0 }}>
            <TextField label="Title" value={title} onChange={(e) => setTitle(e.target.value)} />
            <TextField label="Source" description="Re-adding the same source replaces it." placeholder="faq/refunds" value={source} onChange={(e) => setSource(e.target.value)} />
          </div>
          <textarea className="code-editor small-editor" aria-label="Text" placeholder="Paste text or Markdown…" value={text} onChange={(e) => setText(e.target.value)} />
          <Button tone="accent" leading={<IconPlus />} loading={busy} disabled={!text.trim()} onClick={submit}>
            Add text
          </Button>
        </>
      )}
      {kind === 'url' && (
        <div className="row gap">
          <input className="plain-input" aria-label="URL" placeholder="https://example.com/guide" value={url} onChange={(e) => setUrl(e.target.value)} />
          <Button tone="accent" leading={<IconPlus />} loading={busy} disabled={!/^https?:\/\//.test(url.trim())} onClick={submit}>
            Fetch and add
          </Button>
        </div>
      )}
    </section>
  );
}

function DocumentRow({ collection, d, onDeleted }: { collection: string; d: KnowledgeDocument; onDeleted: () => void }) {
  const [open, setOpen] = useState(false);
  const view = useLoad(() => (open ? api.document(collection, d.id) : Promise.resolve(undefined)), [open, d.id]);
  async function remove() {
    if (!confirm(`Delete ${d.title ?? d.source}?`)) return;
    try {
      await api.deleteDocument(collection, d.id);
      onDeleted();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }
  const format = (d.metadata as { format?: string } | null)?.format;
  return (
    <div className="doc-row">
      <div className="doc-head">
        <button type="button" className="disclosure-head" onClick={() => setOpen(!open)} aria-expanded={open}>
          <span className="doc-title">{d.title ?? d.source}</span>
        </button>
        <span className="muted small mono doc-source">{d.source}</span>
        {format && <span className="chip small">{format}</span>}
        <span className="muted small num">{d.chunks} chunks</span>
        <span className="muted small">{ago(d.created_at)}</span>
        <button type="button" className="icon-btn" aria-label={`Delete ${d.source}`} onClick={remove}>
          <IconTrash />
        </button>
      </div>
      {open && (
        <div className="chunks">
          {view.data?.chunks.map((c) => (
            <div key={c.id} className="chunk">
              <div className="muted small">
                #{c.seq}
                {(c.metadata as { heading?: string } | null)?.heading && ` · ${(c.metadata as { heading: string }).heading}`}
              </div>
              <div className="pre small">{c.text}</div>
            </div>
          ))}
          {view.loading && <span className="muted small">Loading…</span>}
        </div>
      )}
    </div>
  );
}

const MODES: { id: SearchMode; label: string }[] = [
  { id: 'hybrid', label: 'Hybrid' },
  { id: 'vector', label: 'Meaning' },
  { id: 'text', label: 'Keywords' },
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
    <section className="card stack">
      <form
        className="row gap"
        onSubmit={(e) => {
          e.preventDefault();
          run();
        }}
      >
        <input className="plain-input" aria-label="Search query" placeholder="What does an agent need to find?" value={query} onChange={(e) => setQuery(e.target.value)} />
        <Tabs
          items={MODES}
          value={mode}
          onValueChange={(m) => {
            setMode(m as SearchMode);
            run(m as SearchMode);
          }}
          variant="pill"
          aria-label="Search mode"
        />
        <Button type="submit" tone="accent" leading={<IconSearch />} loading={busy} disabled={!query.trim()}>
          Search
        </Button>
      </form>
      {hits?.length === 0 && <Empty title="No results" />}
      <ol className="hits">
        {hits?.map((h, i) => (
          <li key={h.chunk_id} className="hit">
            <div className="row gap wrap">
              <span className="chip">{i + 1}</span>
              <strong>{h.title ?? h.source}</strong>
              {h.heading && <span className="muted small">› {h.heading}</span>}
              <span className="spacer" />
              {h.vector_score != null && <span className="chip small mono">cos {h.vector_score.toFixed(3)}</span>}
              {h.text_score != null && <span className="chip small mono">bm25 {h.text_score.toFixed(2)}</span>}
            </div>
            <div className="pre small">{h.text}</div>
            <div className="muted small mono">{h.source}</div>
          </li>
        ))}
      </ol>
    </section>
  );
}
