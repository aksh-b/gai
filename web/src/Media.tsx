import { useEffect, useRef, useState } from 'react';
import { Skeleton } from '@/components/ui/skeleton';
import { api, type ContentBlock, type MediaSource } from './api';

/** Object URLs for stored blobs, fetched once per page load. */
const urls = new Map<string, Promise<string>>();

export function blobUrl(id: string): Promise<string> {
  let p = urls.get(id);
  if (!p) {
    p = api.blobBytes(id).then((b) => URL.createObjectURL(b));
    p.catch(() => urls.delete(id));
    urls.set(id, p);
  }
  return p;
}

/** A URL a browser can show for a media source. */
export function useMediaUrl(source: MediaSource | undefined): string | undefined {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    if (!source) return;
    if (source.kind === 'url') setUrl(source.url);
    else if (source.kind === 'base64') setUrl(`data:${source.media_type};base64,${source.data}`);
    else {
      let live = true;
      blobUrl(source.id)
        .then((u) => live && setUrl(u))
        .catch(() => {});
      return () => {
        live = false;
      };
    }
  }, [source?.kind === 'blob' ? source.id : source?.kind === 'url' ? source.url : source]);
  return url;
}

/** Image and audio blocks out of a list of content blocks. */
export function mediaBlocks(blocks: ContentBlock[] | undefined): ContentBlock[] {
  return (blocks ?? []).filter((b) => b.type === 'image' || b.type === 'audio');
}

/** An image or audio block, shown inline. */
export function MediaView({ block }: { block: ContentBlock }) {
  const source = block.type === 'image' || block.type === 'audio' ? block.source : undefined;
  const url = useMediaUrl(source);
  if (!source) return null;
  const id = source.kind === 'blob' ? source.id : undefined;
  if (!url) return <Skeleton className={block.type === 'audio' ? 'h-9 w-full max-w-sm' : 'aspect-square w-full max-w-sm'} />;
  if (block.type === 'audio') return <audio className="h-9 w-full max-w-sm" controls src={url} aria-label={id ?? 'Audio'} />;
  return (
    <a href={url} target="_blank" rel="noreferrer" title={id} className="block w-fit max-w-sm overflow-hidden rounded-lg border">
      <img src={url} alt={id ?? 'Image'} className="block h-auto w-full" />
    </a>
  );
}

/** A stored blob by id (from tool output or the media page). */
export function BlobView({ id, mediaType }: { id: string; mediaType: string }) {
  const block: ContentBlock = mediaType.startsWith('audio/')
    ? { type: 'audio', source: { kind: 'blob', id, media_type: mediaType } }
    : { type: 'image', source: { kind: 'blob', id, media_type: mediaType } };
  return <MediaView block={block} />;
}

export function blobToBase64(b: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(',')[1] ?? '');
    r.onerror = () => reject(r.error);
    r.readAsDataURL(b);
  });
}

/**
 * Record from the microphone. `start` asks for permission the first time;
 * `stop` resolves with the recording.
 */
export function useRecorder() {
  const rec = useRef<MediaRecorder | null>(null);
  const chunks = useRef<Blob[]>([]);
  const [recording, setRecording] = useState(false);
  const [seconds, setSeconds] = useState(0);

  useEffect(() => {
    if (!recording) return;
    const t0 = Date.now();
    const t = setInterval(() => setSeconds(Math.floor((Date.now() - t0) / 1000)), 250);
    return () => clearInterval(t);
  }, [recording]);

  async function start() {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const type = ['audio/webm;codecs=opus', 'audio/webm', 'audio/ogg', 'audio/mp4'].find((t) => MediaRecorder.isTypeSupported(t));
    const r = new MediaRecorder(stream, type ? { mimeType: type } : undefined);
    chunks.current = [];
    r.ondataavailable = (e) => e.data.size && chunks.current.push(e.data);
    r.start();
    rec.current = r;
    setSeconds(0);
    setRecording(true);
  }

  function stop(): Promise<Blob> {
    return new Promise((resolve) => {
      const r = rec.current;
      if (!r) return resolve(new Blob());
      r.onstop = () => {
        r.stream.getTracks().forEach((t) => t.stop());
        setRecording(false);
        rec.current = null;
        resolve(new Blob(chunks.current, { type: r.mimeType || 'audio/webm' }));
      };
      r.stop();
    });
  }

  return { recording, seconds, start, stop, supported: typeof MediaRecorder !== 'undefined' };
}
