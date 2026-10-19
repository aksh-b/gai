import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/spinner';
import { SelectField, TabBar } from '@/components/fields';
import { toast } from 'sonner';
import { api, type StoredBlob } from '../api';
import { Empty, ErrorNote, PageHeader } from '../components';
import { IconMic, IconTrash } from '../icons';
import { BlobView, blobToBase64, useRecorder } from '../Media';
import { ago, money, useLoad } from '../util';

/** Try the media models, and browse what is stored. */
export function MediaStudio() {
  const defaults = useLoad(() => api.mediaDefaults(), []);
  const models = useLoad(() => api.models(), []);
  const library = useLoad(() => api.blobs({ limit: 60 }), []);
  const [tab, setTab] = useState('image');

  const byKind = (kind: string) => (models.data ?? []).filter((m) => m.kind === kind && m.key.status !== 'missing').map((m) => m.id);
  const d = defaults.data;

  return (
    <div className="page">
      <PageHeader
        title="Voice & images"
        subtitle="Make images, turn text into speech and speech into text. Assistants can do the same when you give them these abilities."
      />
      <ErrorNote error={defaults.error} />
      <TabBar
        items={[
          { id: 'image', label: 'Images' },
          { id: 'speech', label: 'Speech' },
          { id: 'transcribe', label: 'Transcribe' },
          { id: 'library', label: 'Library' },
        ]}
        value={tab}
        onValueChange={(t) => {
          setTab(t);
          if (t === 'library') library.reload();
        }}
        variant="underline"
        aria-label="Media"
      />
      {tab === 'image' && <ImagePanel fallback={d?.image ?? null} models={byKind('image')} onMade={library.reload} />}
      {tab === 'speech' && <SpeechPanel fallback={d?.speech ?? null} models={byKind('speech')} onMade={library.reload} />}
      {tab === 'transcribe' && <TranscribePanel fallback={d?.transcription ?? null} models={byKind('transcription')} />}
      {tab === 'library' && <Library blobs={library.data ?? []} error={library.error} onChange={library.reload} />}
    </div>
  );
}

/** A model picker: the configured default, or a catalog model. */
function ModelPick({ fallback, models, value, onChange }: { fallback: string | null; models: string[]; value: string; onChange: (v: string) => void }) {
  const options = [
    { value: '', label: fallback ? `Default (${fallback})` : 'Default (none set)' },
    ...models.filter((m) => m !== fallback).map((m) => ({ value: m, label: m })),
  ];
  return <SelectField aria-label="Model" options={options} value={value} onValueChange={onChange} />;
}

function ImagePanel({ fallback, models, onMade }: { fallback: string | null; models: string[]; onMade: () => void }) {
  const [prompt, setPrompt] = useState('');
  const [model, setModel] = useState('');
  const [size, setSize] = useState('1024x1024');
  const [n, setN] = useState('1');
  const [busy, setBusy] = useState(false);
  const [made, setMade] = useState<{ blobs: StoredBlob[]; note: string } | null>(null);

  async function go() {
    setBusy(true);
    try {
      const r = await api.images({ prompt, model: model || undefined, size, n: Number(n) });
      setMade({ blobs: r.blobs, note: `${r.model}${r.cost ? ` · ${money(r.cost)}` : ''}` });
      onMade();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card stack">
      <textarea className="code-editor short" placeholder="A lighthouse at dusk, watercolor, wide shot" aria-label="Prompt" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
      <div className="row gap wrap">
        <ModelPick fallback={fallback} models={models} value={model} onChange={setModel} />
        <SelectField
          aria-label="Size"
          options={['1024x1024', '1536x1024', '1024x1536', '512x512'].map((s) => ({ value: s, label: s }))}
          value={size}
          onValueChange={setSize}
        />
        <SelectField aria-label="How many" options={['1', '2', '3', '4'].map((x) => ({ value: x, label: `${x} image${x === '1' ? '' : 's'}` }))} value={n} onValueChange={setN} />
        <Button onClick={go} disabled={busy || !prompt.trim() || (!fallback && !model)}>
          {busy && <Spinner data-icon="inline-start" />}
          Generate
        </Button>
      </div>
      {made && (
        <>
          <p className="muted small">{made.note}</p>
          <div className="media-grid">
            {made.blobs.map((b) => (
              <BlobView key={b.id} id={b.id} mediaType={b.media_type} />
            ))}
          </div>
        </>
      )}
    </section>
  );
}

function SpeechPanel({ fallback, models, onMade }: { fallback: string | null; models: string[]; onMade: () => void }) {
  const [text, setText] = useState('');
  const [model, setModel] = useState('');
  const [voice, setVoice] = useState('');
  const [busy, setBusy] = useState(false);
  const [made, setMade] = useState<{ blob: StoredBlob; note: string } | null>(null);

  async function go() {
    setBusy(true);
    try {
      const r = await api.speak({ text, model: model || undefined, voice: voice || undefined });
      setMade({ blob: r.blobs[0], note: `${r.model}${r.cost ? ` · ${money(r.cost)}` : ''}` });
      onMade();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card stack">
      <textarea className="code-editor short" placeholder="Something to say" aria-label="Text" value={text} onChange={(e) => setText(e.target.value)} />
      <div className="row gap wrap">
        <ModelPick fallback={fallback} models={models} value={model} onChange={setModel} />
        <input className="plain-input narrow" placeholder="Voice (optional)" aria-label="Voice" value={voice} onChange={(e) => setVoice(e.target.value)} />
        <Button onClick={go} disabled={busy || !text.trim() || (!fallback && !model)}>
          {busy && <Spinner data-icon="inline-start" />}
          Speak
        </Button>
      </div>
      {made && (
        <>
          <p className="muted small">{made.note}</p>
          <BlobView id={made.blob.id} mediaType={made.blob.media_type} />
        </>
      )}
    </section>
  );
}

function TranscribePanel({ fallback, models }: { fallback: string | null; models: string[] }) {
  const [model, setModel] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; note: string } | null>(null);
  const recorder = useRecorder();

  async function transcribe(audio: Blob) {
    setBusy(true);
    try {
      const t = await api.transcribe({ audio_base64: await blobToBase64(audio), media_type: audio.type || 'audio/webm', model: model || undefined });
      setResult({
        text: t.text,
        note: `${t.model}${t.duration_secs ? ` · ${t.duration_secs.toFixed(1)}s` : ''}${t.cost ? ` · ${money(t.cost)}` : ''}`,
      });
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card stack">
      <div className="row gap wrap">
        <ModelPick fallback={fallback} models={models} value={model} onChange={setModel} />
        {recorder.supported && (
          <Button
            variant={recorder.recording ? 'destructive' : 'outline'}
            onClick={async () => {
              if (recorder.recording) transcribe(await recorder.stop());
              else recorder.start().catch((e: Error) => toast.error(`Microphone: ${e.message}`));
            }}
            disabled={!fallback && !model}
          >
            <IconMic data-icon="inline-start" />
            {recorder.recording ? `Stop (${recorder.seconds}s)` : 'Record'}
          </Button>
        )}
        <label className="file-pick">
          <input
            type="file"
            accept="audio/*,video/webm"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) transcribe(f);
              e.target.value = '';
            }}
          />
          <span>…or choose an audio file</span>
        </label>
      </div>
      {busy && <p className="muted">Transcribing…</p>}
      {result && (
        <>
          <p className="muted small">{result.note}</p>
          <blockquote className="transcript-out">{result.text || '(no speech found)'}</blockquote>
        </>
      )}
    </section>
  );
}

function Library({ blobs, error, onChange }: { blobs: StoredBlob[]; error?: string; onChange: () => void }) {
  if (error) return <ErrorNote error={error} />;
  const shown = blobs.filter((b) => b.media_type.startsWith('image/') || b.media_type.startsWith('audio/'));
  if (!shown.length) return <Empty title="Nothing here yet">Images and audio made here or by assistants show up in this list.</Empty>;
  return (
    <div className="media-library">
      {shown.map((b) => {
        const meta = b.metadata as { prompt?: string; text?: string; model?: string };
        return (
          <figure key={b.id} className="card media-card">
            <BlobView id={b.id} mediaType={b.media_type} />
            <figcaption className="stack tight">
              <span className="small clamp">{meta.prompt ?? meta.text ?? b.name ?? b.media_type}</span>
              <span className="muted small">
                {b.source ?? 'stored'} · {meta.model ?? b.media_type} · {ago(b.created_at)}
              </span>
              <div className="row between">
                {b.run_id ? (
                  <a className="small" href={`#/activity/${b.run_id}`}>
                    run
                  </a>
                ) : (
                  <span />
                )}
                <button
                  type="button"
                  className="icon-btn"
                  aria-label="Delete"
                  onClick={async () => {
                    await api.deleteBlob(b.id).catch((e: Error) => toast.error(e.message));
                    onChange();
                  }}
                >
                  <IconTrash />
                </button>
              </div>
            </figcaption>
          </figure>
        );
      })}
    </div>
  );
}
