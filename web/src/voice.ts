import { voiceSocket } from './api';

/** What the voice chat is doing, for the UI. */
export type VoiceState = 'connecting' | 'listening' | 'hearing' | 'thinking' | 'speaking' | 'closed';

export interface VoiceHandlers {
  onState: (s: VoiceState) => void;
  /** The level of the microphone, 0 to 1, for a meter. */
  onLevel?: (level: number) => void;
  onTranscript: (text: string) => void;
  onReplyStarted: (runId?: string, sessionId?: string) => void;
  onDelta: (text: string) => void;
  onTool: (name: string) => void;
  onReplyDone: (e: { runId?: string; sessionId?: string; text?: string; status?: string; error?: string | null }) => void;
  onInterrupted: () => void;
  onError: (message: string) => void;
}

export interface VoiceStart {
  agent?: string;
  sessionId?: string;
  voice?: string;
}

/** How loud counts as speech (RMS of the signal). */
const SPEECH_LEVEL = 0.03;
/** Louder while the assistant talks, so its own voice from the speakers
 * doesn't interrupt it. */
const BARGE_IN_LEVEL = 0.08;
/** Speech this long starts a turn, silence this long ends it. */
const START_MS = 150;
const END_MS = 800;

/**
 * A voice conversation over `/v1/voice`: listens on the microphone, finds
 * where the user starts and stops talking, plays the reply as it arrives,
 * and stops it when the user talks over it.
 */
export class VoiceChat {
  private ws?: WebSocket;
  private stream?: MediaStream;
  private ctx?: AudioContext;
  private analyser?: AnalyserNode;
  private timer?: number;
  private recorder?: MediaRecorder;
  private mode: 'cascade' | 'realtime' = 'cascade';
  private playing: AudioBufferSourceNode[] = [];
  private playAt = 0;
  private aboveSince = 0;
  private belowSince = 0;
  private replying = false;
  private closed = false;
  private processor?: ScriptProcessorNode;
  private chain: Promise<void> = Promise.resolve();
  private playChain: Promise<void> = Promise.resolve();
  /** Bumped on each interruption, so clips still decoding are dropped. */
  private gen = 0;

  constructor(private h: VoiceHandlers) {}

  async start(opts: VoiceStart) {
    this.h.onState('connecting');
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
    });
    const ws = voiceSocket();
    this.ws = ws;
    ws.binaryType = 'arraybuffer';
    await new Promise<void>((resolve, reject) => {
      ws.onopen = () => resolve();
      ws.onerror = () => reject(new Error('could not open the voice connection'));
    });
    ws.onmessage = (e) => this.message(JSON.parse(String(e.data)));
    ws.onclose = () => this.stop();
    ws.send(JSON.stringify({ type: 'start', agent: opts.agent, session_id: opts.sessionId, voice: opts.voice }));
  }

  /** Type a turn instead of saying it. */
  say(text: string) {
    this.interrupt();
    this.ws?.send(JSON.stringify({ type: 'text', text }));
  }

  stop() {
    if (this.closed) return;
    this.closed = true;
    window.clearInterval(this.timer);
    this.stopPlayback();
    try {
      this.recorder?.state === 'recording' && this.recorder.stop();
    } catch {
      /* already stopped */
    }
    this.processor?.disconnect();
    this.stream?.getTracks().forEach((t) => t.stop());
    this.ctx?.close().catch(() => {});
    if (this.ws && this.ws.readyState <= WebSocket.OPEN) this.ws.close();
    this.h.onState('closed');
  }

  private message(m: Record<string, unknown>) {
    switch (m.type) {
      case 'ready':
        this.mode = m.mode === 'realtime' ? 'realtime' : 'cascade';
        void this.listen();
        break;
      case 'transcript':
        if (typeof m.text === 'string' && m.text.trim()) this.h.onTranscript(m.text);
        break;
      case 'reply_started':
        this.replying = true;
        this.h.onState('thinking');
        this.h.onReplyStarted(m.run_id as string | undefined, m.session_id as string | undefined);
        break;
      case 'delta':
        this.h.onDelta(String(m.text ?? ''));
        break;
      case 'tool':
        this.h.onTool(String(m.name ?? ''));
        break;
      case 'audio': {
        // Decoding is async: keep clips in the order they came.
        const [data, type, gen] = [String(m.data ?? ''), String(m.media_type ?? ''), this.gen];
        this.playChain = this.playChain.then(() => (gen === this.gen ? this.play(data, type) : undefined));
        break;
      }
      case 'speech_started':
        // The provider heard the user: stop talking over them.
        this.stopPlayback();
        this.h.onState('hearing');
        break;
      case 'reply_done':
        this.replying = false;
        this.h.onReplyDone({
          runId: m.run_id as string | undefined,
          sessionId: m.session_id as string | undefined,
          text: m.text as string | undefined,
          status: m.status as string | undefined,
          error: m.error as string | null | undefined,
        });
        this.afterPlayback();
        break;
      case 'interrupted':
        this.replying = false;
        this.h.onInterrupted();
        break;
      case 'error':
        this.h.onError(String(m.message ?? 'voice error'));
        break;
    }
  }

  private async listen() {
    const realtime = this.mode === 'realtime';
    this.ctx = new AudioContext(realtime ? { sampleRate: 24000 } : undefined);
    const src = this.ctx.createMediaStreamSource(this.stream!);
    this.analyser = this.ctx.createAnalyser();
    this.analyser.fftSize = 1024;
    src.connect(this.analyser);
    if (realtime) {
      // 24 kHz 16-bit mono PCM, straight to the provider, which finds the turns.
      this.processor = this.ctx.createScriptProcessor(2048, 1, 1);
      this.processor.onaudioprocess = (e) => {
        const f = e.inputBuffer.getChannelData(0);
        const pcm = new Int16Array(f.length);
        for (let i = 0; i < f.length; i++) pcm[i] = Math.max(-1, Math.min(1, f[i])) * 0x7fff;
        if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(pcm.buffer);
      };
      src.connect(this.processor);
      // A script processor only runs when connected to an output; keep it silent.
      const mute = this.ctx.createGain();
      mute.gain.value = 0;
      this.processor.connect(mute).connect(this.ctx.destination);
    }
    this.h.onState('listening');
    const buf = new Float32Array(this.analyser.fftSize);
    this.timer = window.setInterval(() => {
      this.analyser!.getFloatTimeDomainData(buf);
      let sum = 0;
      for (const v of buf) sum += v * v;
      const level = Math.sqrt(sum / buf.length);
      this.h.onLevel?.(Math.min(1, level * 8));
      if (!realtime) this.detect(level);
    }, 50);
  }

  /** Cascade mode: start and end turns from the level, and barge in. */
  private detect(level: number) {
    const now = performance.now();
    const speaking = this.playing.length > 0;
    const loud = level > (speaking ? BARGE_IN_LEVEL : SPEECH_LEVEL);
    const recording = this.recorder?.state === 'recording';
    if (loud) {
      this.belowSince = 0;
      if (!this.aboveSince) this.aboveSince = now;
      if (!recording && now - this.aboveSince > START_MS) {
        if (speaking || this.replying) this.interrupt();
        this.startTurn();
      }
    } else {
      this.aboveSince = 0;
      if (recording) {
        if (!this.belowSince) this.belowSince = now;
        if (now - this.belowSince > END_MS) this.endTurn();
      }
    }
  }

  private startTurn() {
    const type = ['audio/webm;codecs=opus', 'audio/webm', 'audio/ogg', 'audio/mp4'].find((t) => MediaRecorder.isTypeSupported(t));
    const r = new MediaRecorder(this.stream!, type ? { mimeType: type } : undefined);
    this.recorder = r;
    r.ondataavailable = (e) => {
      if (e.data.size) this.chain = this.chain.then(async () => this.ws?.send(await e.data.arrayBuffer()));
    };
    r.onstop = () => {
      this.chain = this.chain.then(() => {
        this.ws?.send(JSON.stringify({ type: 'commit', media_type: (r.mimeType || 'audio/webm').split(';')[0] }));
      });
    };
    r.start(250);
    this.h.onState('hearing');
  }

  private endTurn() {
    this.belowSince = 0;
    this.recorder?.stop();
    this.h.onState('thinking');
  }

  /** Stop the reply: what is playing, and (in cascade mode) the run. */
  interrupt() {
    this.stopPlayback();
    if (this.replying) this.ws?.send(JSON.stringify({ type: 'interrupt' }));
    this.replying = false;
  }

  private stopPlayback() {
    this.gen++;
    for (const s of this.playing) {
      try {
        s.stop();
      } catch {
        /* not started */
      }
    }
    this.playing = [];
    this.playAt = 0;
  }

  private async play(b64: string, mediaType: string) {
    if (!this.ctx || !b64) return;
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    let buffer: AudioBuffer;
    if (mediaType.startsWith('audio/pcm')) {
      const rate = Number(/rate=(\d+)/.exec(mediaType)?.[1] ?? 24000);
      const pcm = new Int16Array(bytes.buffer, 0, Math.floor(bytes.byteLength / 2));
      buffer = this.ctx.createBuffer(1, pcm.length, rate);
      const ch = buffer.getChannelData(0);
      for (let i = 0; i < pcm.length; i++) ch[i] = pcm[i] / 0x8000;
    } else {
      try {
        buffer = await this.ctx.decodeAudioData(bytes.buffer);
      } catch {
        return;
      }
    }
    const node = this.ctx.createBufferSource();
    node.buffer = buffer;
    node.connect(this.ctx.destination);
    const at = Math.max(this.ctx.currentTime, this.playAt);
    node.start(at);
    this.playAt = at + buffer.duration;
    this.playing.push(node);
    this.h.onState('speaking');
    node.onended = () => {
      this.playing = this.playing.filter((n) => n !== node);
      this.afterPlayback();
    };
  }

  private afterPlayback() {
    if (!this.playing.length && !this.replying && !this.closed && this.recorder?.state !== 'recording') this.h.onState('listening');
  }
}
