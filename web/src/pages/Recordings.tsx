import { useEffect, useMemo, useRef, useState } from 'react';
import { DownloadIcon, FileAudioIcon, FileVideoIcon, UploadIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField, TextField } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, LinkItem, Markdown, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Progress } from '@/components/ui/progress';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Textarea } from '@/components/ui/textarea';
import { api, type RecordingResult, type TranscriptSegment } from '../api';
import { IconTrash } from '../icons';
import { blobUrl } from '../Media';
import { navigate } from '../router';
import { ago, useLoad } from '../util';

const KINDS = [
  { value: 'meeting', label: 'Meeting notes', description: 'Speakers, summary, decisions and action items' },
  { value: 'audio', label: 'Call analysis', description: 'Who talked how much, the mood, key moments' },
  { value: 'video', label: 'Video summary', description: 'What is shown and said, in chapters' },
];

function clock(secs: number | null | undefined): string {
  const t = Math.max(0, Math.floor(secs ?? 0));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t / 60) % 60);
  const s = String(t % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

/** Recordings: meetings, calls and videos turned into notes and summaries. */
export function RecordingsPage({ selected }: { selected?: string }) {
  const list = useLoad(() => api.recordings(), []);
  const [adding, setAdding] = useState(false);
  const current = adding ? undefined : (selected ?? list.data?.[0]?.id);
  // Keep the list fresh while something is processing.
  const busy = list.data?.some((r) => r.status === 'processing');
  useEffect(() => {
    if (!busy) return;
    const t = setInterval(list.reload, 3000);
    return () => clearInterval(t);
  }, [busy, list.reload]);

  return (
    <Page wide>
      <PageHeader
        title="Recordings"
        subtitle="Drop in a meeting, call or video: get who said what, the decisions and action items, or a summary in chapters."
        actions={
          <Button onClick={() => setAdding(true)}>
            <UploadIcon data-icon="inline-start" />
            Add a recording
          </Button>
        }
      />
      <ErrorNote error={list.error} />
      <div className="grid items-start gap-4 lg:grid-cols-[16rem_minmax(0,1fr)]">
        <div className="flex flex-col gap-1">
          {list.data?.length === 0 && !adding && (
            <EmptyState title="No recordings yet">Add a meeting recording to get its notes.</EmptyState>
          )}
          {list.data?.map((r) => (
            <LinkItem
              key={r.id}
              href={`#/recordings/${encodeURIComponent(r.id)}`}
              active={current === r.id}
              title={r.title}
              onClick={() => setAdding(false)}
            >
              <span className="flex items-center gap-2 text-xs text-muted-foreground">
                {r.kind === 'video' ? <FileVideoIcon className="size-3" /> : <FileAudioIcon className="size-3" />}
                {r.status === 'processing' ? 'Processing…' : r.status === 'failed' ? 'Failed' : ago(r.created_at)}
              </span>
            </LinkItem>
          ))}
        </div>
        {adding || !current ? (
          adding || list.data?.length === 0 ? (
            <NewRecording
              onCreated={(id) => {
                setAdding(false);
                list.reload();
                navigate(`/recordings/${encodeURIComponent(id)}`);
              }}
              onCancel={list.data?.length ? () => setAdding(false) : undefined}
            />
          ) : null
        ) : (
          <RecordingView key={current} id={current} onChange={list.reload} />
        )}
      </div>
    </Page>
  );
}

function NewRecording({ onCreated, onCancel }: { onCreated: (id: string) => void; onCancel?: () => void }) {
  const templates = useLoad(() => api.notesTemplates(), []);
  const [file, setFile] = useState<File>();
  const [kind, setKind] = useState('meeting');
  const [template, setTemplate] = useState('general');
  const [title, setTitle] = useState('');
  const [context, setContext] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [dragging, setDragging] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  function pick(f: File | undefined) {
    if (!f) return;
    setFile(f);
    if (f.type.startsWith('video/')) setKind('video');
    if (!title) setTitle(f.name.replace(/\.[^.]+$/, ''));
  }

  async function submit() {
    if (!file) return;
    setBusy(true);
    setError(undefined);
    try {
      const blob = await api.uploadBlob(file, file.name);
      const rec = await api.createRecording({
        blob_id: blob.id,
        kind,
        title: title.trim() || undefined,
        options: {
          template: kind === 'meeting' ? template : undefined,
          context: context.trim() || undefined,
        },
      });
      onCreated(rec.id);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Add a recording</CardTitle>
        <CardDescription>Audio or video up to 500 MB. Long recordings take a minute or so per ten minutes.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <button
          type="button"
          onClick={() => input.current?.click()}
          onDragOver={(e) => {
            e.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragging(false);
            pick(e.dataTransfer.files[0]);
          }}
          className={`flex flex-col items-center gap-2 rounded-lg border border-dashed p-8 text-sm text-muted-foreground transition-colors ${
            dragging ? 'border-primary bg-muted' : 'hover:bg-muted/50'
          }`}
        >
          <UploadIcon className="size-6" />
          {file ? (
            <span className="text-foreground">
              {file.name} · {(file.size / 1024 / 1024).toFixed(1)} MB
            </span>
          ) : (
            <span>Drop a recording here, or choose one</span>
          )}
        </button>
        <input
          ref={input}
          type="file"
          accept="audio/*,video/*"
          className="hidden"
          aria-label="Recording file"
          onChange={(e) => pick(e.target.files?.[0])}
        />
        <div className="grid gap-4 md:grid-cols-2">
          <SelectField label="What to make" options={KINDS} value={kind} onValueChange={setKind} />
          {kind === 'meeting' && (
            <SelectField
              label="Notes template"
              options={(templates.data ?? [{ name: 'general', sections: [] }]).map((t) => ({
                value: t.name,
                label: t.name.replace(/_/g, ' '),
                description: t.sections.length ? `Adds ${t.sections.join(', ').replace(/_/g, ' ')}` : 'Summary, decisions, action items',
              }))}
              value={template}
              onValueChange={setTemplate}
            />
          )}
        </div>
        <TextField label="Title" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Weekly sync" />
        <div className="flex flex-col gap-2">
          <label className="text-sm font-medium" htmlFor="rec-context">
            {kind === 'video' ? 'What to look out for' : 'Agenda, attendees, names to spell right'}
          </label>
          <Textarea
            id="rec-context"
            value={context}
            onChange={(e) => setContext(e.target.value)}
            placeholder={kind === 'video' ? 'The product demo and pricing' : 'Attendees: Ana (PM), Ben (eng). Agenda: release date, login bug.'}
            rows={3}
          />
        </div>
        <ErrorNote error={error} />
        <div className="flex gap-2">
          <Button disabled={!file || busy} onClick={submit}>
            {busy && <Spinner data-icon="inline-start" />}
            {busy ? 'Uploading…' : 'Process'}
          </Button>
          {onCancel && (
            <Button variant="ghost" onClick={onCancel}>
              Cancel
            </Button>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function RecordingView({ id, onChange }: { id: string; onChange: () => void }) {
  const rec = useLoad(() => api.recording(id), [id]);
  const processing = rec.data?.status === 'processing';
  useEffect(() => {
    if (!processing) return;
    const t = setInterval(rec.reload, 2500);
    return () => clearInterval(t);
  }, [processing, rec.reload]);
  const player = useRef<HTMLMediaElement | null>(null);
  const seek = (secs: number) => {
    if (player.current) {
      player.current.currentTime = secs;
      void player.current.play().catch(() => {});
    }
  };
  if (rec.error) return <ErrorNote error={rec.error} />;
  const r = rec.data;
  if (!r) return <Spinner />;
  const result = r.result as RecordingResult | null | undefined;

  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle className="flex flex-wrap items-center gap-2">
            {r.title}
            <Badge variant="outline">{KINDS.find((k) => k.value === r.kind)?.label ?? r.kind}</Badge>
            {r.status === 'failed' && <Badge variant="destructive">Failed</Badge>}
          </CardTitle>
          <CardDescription>
            {ago(r.created_at)}
            {result?.transcript.duration_secs ? ` · ${clock(result.transcript.duration_secs)} long` : ''}
            {result?.insights?.language ? ` · ${result.insights.language}` : ''}
            {result ? ` · $${result.cost.toFixed(4)}` : ''}
          </CardDescription>
          <CardAction className="flex gap-2">
            {result && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  const a = document.createElement('a');
                  a.href = URL.createObjectURL(new Blob([result.markdown], { type: 'text/markdown' }));
                  a.download = `${r.title.replace(/[^\w.-]+/g, '-') || 'notes'}.md`;
                  a.click();
                }}
              >
                <DownloadIcon data-icon="inline-start" />
                Notes
              </Button>
            )}
            <ConfirmAction
              title={`Delete ${r.title}?`}
              description="Its notes and transcript go; the uploaded file stays under media."
              action="Delete"
              destructive
              onConfirm={() =>
                api.deleteRecording(r.id).then(
                  () => {
                    navigate('/recordings');
                    onChange();
                  },
                  (e: Error) => toast.error(e.message),
                )
              }
            >
              <Button variant="ghost" size="sm">
                <IconTrash data-icon="inline-start" />
                Delete
              </Button>
            </ConfirmAction>
          </CardAction>
        </CardHeader>
        {r.blob_id && (
          <CardContent>
            <Player blobId={r.blob_id} video={r.kind === 'video'} bind={(el) => (player.current = el)} />
          </CardContent>
        )}
      </Card>
      {processing && (
        <Card>
          <CardContent className="flex items-center gap-3 text-sm text-muted-foreground">
            <Spinner />
            Transcribing and writing {r.kind === 'video' ? 'the summary' : 'the notes'}…
          </CardContent>
        </Card>
      )}
      {r.status === 'failed' && <ErrorNote error={r.error ?? 'Processing failed'} />}
      {result && <ResultView result={result} seek={seek} />}
    </div>
  );
}

function Player({ blobId, video, bind }: { blobId: string; video: boolean; bind: (el: HTMLMediaElement | null) => void }) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    let live = true;
    blobUrl(blobId)
      .then((u) => live && setUrl(u))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [blobId]);
  if (!url) return null;
  return video ? (
    <video ref={bind} src={url} controls className="max-h-96 w-full rounded-lg bg-black" />
  ) : (
    <audio ref={bind} src={url} controls className="w-full" />
  );
}

function Time({ at, seek }: { at: number | null | undefined; seek: (s: number) => void }) {
  if (at == null) return null;
  return (
    <button type="button" onClick={() => seek(at)} className="font-mono text-xs text-muted-foreground tabular-nums hover:text-foreground hover:underline">
      {clock(at)}
    </button>
  );
}

function ResultView({ result, seek }: { result: RecordingResult; seek: (s: number) => void }) {
  const { notes, insights, video } = result;
  // Empty lists are left out of the JSON.
  const segments = result.transcript.segments ?? [];
  const actions = notes?.action_items ?? [];
  const topics = notes?.topics ?? [];
  const chapters = video?.chapters ?? [];
  const frames = video?.frames ?? [];
  const speakers = insights?.speakers ?? [];
  const moments = insights?.moments ?? [];
  return (
    <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
      <div className="flex flex-col gap-4">
        {notes && (
          <Card>
            <CardHeader>
              <CardTitle>{notes.title}</CardTitle>
              <CardDescription>Notes · {notes.template.replace(/_/g, ' ')}</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-5">
              <Markdown text={notes.summary} />
              {actions.length > 0 && (
                <section className="flex flex-col gap-2">
                  <h3 className="font-medium">Action items</h3>
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Task</TableHead>
                        <TableHead>Owner</TableHead>
                        <TableHead>Due</TableHead>
                        <TableHead className="w-16">When</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {actions.map((a, i) => (
                        <TableRow key={i}>
                          <TableCell className="whitespace-normal">{a.task}</TableCell>
                          <TableCell>{a.owner ?? '—'}</TableCell>
                          <TableCell>{a.due ?? '—'}</TableCell>
                          <TableCell>
                            <Time at={a.at} seek={seek} />
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </section>
              )}
              <ListSection title="Decisions" items={notes.decisions} />
              {Object.entries(notes.sections ?? {}).map(([k, v]) => (
                <section key={k} className="flex flex-col gap-2">
                  <h3 className="font-medium capitalize">{k.replace(/_/g, ' ')}</h3>
                  <Markdown text={v} />
                </section>
              ))}
              <ListSection title="Open questions" items={notes.open_questions} />
            </CardContent>
          </Card>
        )}
        {video && (
          <Card>
            <CardHeader>
              <CardTitle>{video.title}</CardTitle>
              <CardDescription>Video summary</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-5">
              <Markdown text={video.summary} />
              <ListSection title="Key points" items={video.key_points} />
              {chapters.length > 0 && (
                <section className="flex flex-col gap-3">
                  <h3 className="font-medium">Chapters</h3>
                  {chapters.map((c, i) => {
                    const frame = [...frames].reverse().find((f) => f.at <= c.start + 1) ?? frames[0];
                    return (
                      <div key={i} className="flex gap-3">
                        {frame?.blob_id && <Thumb id={frame.blob_id} onClick={() => seek(c.start)} />}
                        <div className="flex flex-col gap-0.5">
                          <span className="flex items-center gap-2 font-medium">
                            <Time at={c.start} seek={seek} />
                            {c.title}
                          </span>
                          {c.summary && <span className="text-sm text-muted-foreground">{c.summary}</span>}
                        </div>
                      </div>
                    );
                  })}
                </section>
              )}
            </CardContent>
          </Card>
        )}
        {segments.length > 0 && <TranscriptCard segments={segments} seek={seek} />}
      </div>
      <div className="flex flex-col gap-4">
        {insights && (speakers.length > 0 || insights.sentiment) && (
          <Card>
            <CardHeader>
              <CardTitle className="text-base">Speakers</CardTitle>
              {insights.sentiment && (
                <CardDescription>
                  Overall {insights.sentiment.label} ({insights.sentiment.score >= 0 ? '+' : ''}
                  {insights.sentiment.score.toFixed(2)}){insights.sentiment.reason ? `. ${insights.sentiment.reason}` : ''}
                </CardDescription>
              )}
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              {speakers.map((s) => (
                <div key={s.name} className="flex flex-col gap-1">
                  <div className="flex items-center justify-between gap-2 text-sm">
                    <span className="font-medium">{s.name}</span>
                    <span className="text-muted-foreground tabular-nums">
                      {Math.round(s.share * 100)}% · {s.turns} turns
                      {s.sentiment ? ` · ${s.sentiment.label}` : ''}
                    </span>
                  </div>
                  <Progress value={s.share * 100} aria-label={`${s.name}'s share of the talking`} />
                  {s.summary && <span className="text-xs text-muted-foreground">{s.summary}</span>}
                </div>
              ))}
            </CardContent>
          </Card>
        )}
        {moments.length > 0 && (
          <Card>
            <CardHeader>
              <CardTitle className="text-base">Moments</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-2 text-sm">
              {moments.map((m, i) => (
                <div key={i} className="flex gap-2">
                  <Time at={m.at} seek={seek} />
                  <span>{m.what}</span>
                </div>
              ))}
            </CardContent>
          </Card>
        )}
        {topics.length > 0 && (
          <Card>
            <CardHeader>
              <CardTitle className="text-base">Topics</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-2 text-sm">
              {topics.map((t, i) => (
                <div key={i} className="flex gap-2">
                  <Time at={t.start} seek={seek} />
                  <span>{t.title}</span>
                </div>
              ))}
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}

function ListSection({ title, items }: { title: string; items?: string[] }) {
  if (!items?.length) return null;
  return (
    <section className="flex flex-col gap-2">
      <h3 className="font-medium">{title}</h3>
      <ul className="list-disc space-y-1 pl-5 text-sm">
        {items.map((d, i) => (
          <li key={i}>{d}</li>
        ))}
      </ul>
    </section>
  );
}

function Thumb({ id, onClick }: { id: string; onClick: () => void }) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    blobUrl(id)
      .then(setUrl)
      .catch(() => {});
  }, [id]);
  return (
    <button type="button" onClick={onClick} className="w-28 shrink-0 overflow-hidden rounded border bg-muted" aria-label="Play from here">
      {url && <img src={url} alt="" className="block h-auto w-full" />}
    </button>
  );
}

const SPEAKER_COLORS = ['text-chart-1', 'text-chart-2', 'text-chart-3', 'text-chart-4', 'text-chart-5'];

function TranscriptCard({ segments, seek }: { segments: TranscriptSegment[]; seek: (s: number) => void }) {
  const [q, setQ] = useState('');
  const colors = useMemo(() => {
    const m = new Map<string, string>();
    for (const s of segments) if (s.speaker && !m.has(s.speaker)) m.set(s.speaker, SPEAKER_COLORS[m.size % SPEAKER_COLORS.length]);
    return m;
  }, [segments]);
  const shown = q ? segments.filter((s) => s.text.toLowerCase().includes(q.toLowerCase())) : segments;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">Transcript</CardTitle>
        <CardAction>
          <TextField aria-label="Search the transcript" placeholder="Search" value={q} onChange={(e) => setQ(e.target.value)} className="w-48" />
        </CardAction>
      </CardHeader>
      <CardContent className="flex max-h-[32rem] flex-col gap-2 overflow-auto text-sm">
        {shown.map((s, i) => (
          <div key={i} className="flex gap-3">
            <Time at={s.start} seek={seek} />
            <p>
              {s.speaker && <span className={`mr-1 font-medium ${colors.get(s.speaker)}`}>{s.speaker}:</span>}
              {s.text}
            </p>
          </div>
        ))}
        {shown.length === 0 && <span className="text-muted-foreground">Nothing matches.</span>}
      </CardContent>
    </Card>
  );
}
