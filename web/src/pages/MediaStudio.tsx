import { useState } from 'react';
import { ImageIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField, TabBar } from '@/components/fields';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter } from '@/components/ui/card';
import { Field, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Item, ItemContent, ItemDescription } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { Textarea } from '@/components/ui/textarea';
import { api, type StoredBlob } from '../api';
import { ConfirmAction, EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
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
    <Page>
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
    </Page>
  );
}

/** A model picker: the configured default, or a catalog model. */
function ModelPick({
  fallback,
  models,
  value,
  onChange,
}: {
  fallback: string | null;
  models: string[];
  value: string;
  onChange: (v: string) => void;
}) {
  const options = [
    { value: '', label: fallback ? `Default (${fallback})` : 'Default (none set)' },
    ...models.filter((m) => m !== fallback).map((m) => ({ value: m, label: m })),
  ];
  return <SelectField aria-label="Model" className="w-64" options={options} value={value} onValueChange={onChange} />;
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
    <Card>
      <CardContent className="flex flex-col gap-3">
        <Textarea
          placeholder="A lighthouse at dusk, watercolor, wide shot"
          aria-label="Prompt"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
        />
        <div className="flex flex-wrap items-center gap-2">
          <ModelPick fallback={fallback} models={models} value={model} onChange={setModel} />
          <SelectField
            aria-label="Size"
            className="w-36"
            options={['1024x1024', '1536x1024', '1024x1536', '512x512'].map((s) => ({ value: s, label: s }))}
            value={size}
            onValueChange={setSize}
          />
          <SelectField
            aria-label="How many"
            className="w-32"
            options={['1', '2', '3', '4'].map((x) => ({ value: x, label: `${x} image${x === '1' ? '' : 's'}` }))}
            value={n}
            onValueChange={setN}
          />
          <Button onClick={go} disabled={busy || !prompt.trim() || (!fallback && !model)}>
            {busy && <Spinner data-icon="inline-start" />}
            Generate
          </Button>
        </div>
        {made && (
          <>
            <CardDescription>{made.note}</CardDescription>
            <div className="flex flex-wrap gap-3">
              {made.blobs.map((b) => (
                <BlobView key={b.id} id={b.id} mediaType={b.media_type} />
              ))}
            </div>
          </>
        )}
      </CardContent>
    </Card>
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
    <Card>
      <CardContent className="flex flex-col gap-3">
        <Textarea placeholder="Something to say" aria-label="Text" value={text} onChange={(e) => setText(e.target.value)} />
        <div className="flex flex-wrap items-center gap-2">
          <ModelPick fallback={fallback} models={models} value={model} onChange={setModel} />
          <Input
            className="w-48"
            placeholder="Voice (optional)"
            aria-label="Voice"
            value={voice}
            onChange={(e) => setVoice(e.target.value)}
          />
          <Button onClick={go} disabled={busy || !text.trim() || (!fallback && !model)}>
            {busy && <Spinner data-icon="inline-start" />}
            Speak
          </Button>
        </div>
        {made && (
          <>
            <CardDescription>{made.note}</CardDescription>
            <BlobView id={made.blob.id} mediaType={made.blob.media_type} />
          </>
        )}
      </CardContent>
    </Card>
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
      const t = await api.transcribe({
        audio_base64: await blobToBase64(audio),
        media_type: audio.type || 'audio/webm',
        model: model || undefined,
      });
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
    <Card>
      <CardContent className="flex flex-col gap-3">
        <div className="flex flex-wrap items-end gap-2">
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
          <Field className="w-auto">
            <FieldLabel htmlFor="audio-file">Or an audio file</FieldLabel>
            <Input
              id="audio-file"
              type="file"
              accept="audio/*,video/webm"
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) transcribe(f);
                e.target.value = '';
              }}
            />
          </Field>
        </div>
        {busy && (
          <p className="flex items-center gap-2 text-muted-foreground">
            <Spinner /> Transcribing…
          </p>
        )}
        {result && (
          <Item variant="muted">
            <ItemContent>
              <p className="whitespace-pre-wrap">{result.text || '(no speech found)'}</p>
              <ItemDescription>{result.note}</ItemDescription>
            </ItemContent>
          </Item>
        )}
      </CardContent>
    </Card>
  );
}

function Library({ blobs, error, onChange }: { blobs: StoredBlob[]; error?: string; onChange: () => void }) {
  if (error) return <ErrorNote error={error} />;
  const shown = blobs.filter((b) => b.media_type.startsWith('image/') || b.media_type.startsWith('audio/'));
  if (!shown.length)
    return (
      <EmptyState title="Nothing here yet" icon={<ImageIcon />}>
        Images and audio made here or by assistants show up in this list.
      </EmptyState>
    );
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(min(15rem,100%),1fr))] gap-4">
      {shown.map((b) => {
        const meta = b.metadata as { prompt?: string; text?: string; model?: string };
        return (
          <Card key={b.id} size="sm">
            <CardContent className="flex flex-col gap-2">
              <BlobView id={b.id} mediaType={b.media_type} />
              <p className="line-clamp-2 text-sm">{meta.prompt ?? meta.text ?? b.name ?? b.media_type}</p>
              <CardDescription>
                {b.source ?? 'stored'} · {meta.model ?? b.media_type} · {ago(b.created_at)}
              </CardDescription>
            </CardContent>
            <CardFooter className="justify-between">
              {b.run_id ? (
                <Button variant="link" size="sm" className="px-0" asChild>
                  <a href={`#/activity/${b.run_id}`}>Open the run</a>
                </Button>
              ) : (
                <span />
              )}
              <ConfirmAction
                title="Delete this file?"
                description="It is removed from the library and from any chat that shows it."
                action="Delete"
                destructive
                onConfirm={async () => {
                  await api.deleteBlob(b.id).catch((e: Error) => toast.error(e.message));
                  onChange();
                }}
              >
                <Button variant="ghost" size="icon-sm" aria-label="Delete">
                  <IconTrash />
                </Button>
              </ConfirmAction>
            </CardFooter>
          </Card>
        );
      })}
    </div>
  );
}
