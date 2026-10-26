# Voice, meetings and video

gai can hold a spoken conversation with any agent, and turn recordings of
meetings, calls and videos into notes: who said what, the decisions and
action items, the mood, or a summary in chapters.

## Models

All of this uses the `[media]` models in `gai.toml` (or an agent's own
`media`), and the default model (or the agent's) to write:

```toml
[media]
transcription = "openai/gpt-4o-transcribe-diarize"  # labels speakers; whisper-1 gives timed segments
speech = "openai/gpt-4o-mini-tts"
voice = "alloy"
vision = "openai/gpt-5"           # describes video frames; the writing model when not set
realtime = "openai/gpt-realtime"  # optional: speech-to-speech for live voice
```

Speakers come from the transcription model when it can tell them apart
(`gpt-4o-transcribe-diarize`, ElevenLabs `scribe_v1`). With any other
model, the writing model works them out from the conversation: names when
people say them, otherwise Speaker 1, Speaker 2.

Long recordings and video need **ffmpeg** (included in the Docker image;
`apt install ffmpeg` or `brew install ffmpeg` elsewhere): it takes the
audio out of video and cuts recordings into ten-minute pieces, which are
transcribed in turn and joined with their times. Without it, gai takes
audio up to 25 MB.

## Talking to an agent

Press **Talk** next to the message box in the chat: speak, pause, and the
agent answers aloud. Talk over it to interrupt; it stops and listens.
Typed messages in the same chat are answered aloud too.

Each pause ends a turn: the audio is transcribed, the agent runs (tools,
approvals and the conversation work as in a typed chat), and its answer is
spoken one sentence at a time, starting before the whole answer is
written.

An agent with `media.realtime` talks through the provider's
speech-to-speech session instead: lower latency, and the provider hears
when you start talking. Its tools that need no approval run in gai.

### The WebSocket

`GET /v1/voice` upgrades to a WebSocket (browsers pass their API key as the
subprotocol `bearer.<key>`, next to `gai`). JSON messages, plus audio as
binary frames:

| Client sends | |
| --- | --- |
| `{"type": "start", "agent": "...", "session_id": "...", "voice": "..."}` | first; all optional |
| binary frames | the current utterance's audio (any format the transcriber takes) |
| `{"type": "commit", "media_type": "audio/webm"}` | the utterance is over: answer it |
| `{"type": "text", "text": "..."}` | a typed turn |
| `{"type": "interrupt"}` | stop the reply (barge-in) |

| Server sends | |
| --- | --- |
| `ready` | with `mode` (`cascade` or `realtime`) |
| `transcript` | what the user said |
| `reply_started` | with `run_id` and `session_id` |
| `delta` | the answer's text as it is written |
| `audio` | a spoken sentence: `data` (base64), `media_type`, `text`, `seq` |
| `tool` | a tool is running |
| `reply_done` | with the run's `status`, `text` and `cost` |
| `interrupted`, `error` | |

In `realtime` mode the client streams 24 kHz 16-bit mono PCM instead of
committing turns, `audio` is the same PCM, and `speech_started` says the
user started talking (stop playback).

## Recordings

Under **Knowledge › Recordings**, drop in a recording and pick what to make:

- **Meeting notes**: a summary, decisions, action items with owners and
  due dates, open questions, topics with times, and the speakers' share of
  the talking. Templates add sections: `standup` (each person's update,
  blockers), `one_on_one`, `sales_call` (needs, objections, budget and
  timeline), `interview`, `retro`, `lecture`.
- **Call analysis**: who talked how much, the language, the overall and
  each speaker's sentiment, and the key moments.
- **Video summary**: frames sampled through the video are described by
  the vision model, and with what is said, summarized in chapters with
  key points.

Add the agenda, attendees or names to spell right: they help both the
transcription and the notes. Times in the notes jump the player there.
Everything is kept (with the full transcript) and can be downloaded as
Markdown.

From the command line:

```bash
gai notes standup.m4a --template standup --context "Ana, Ben, Chen"
gai notes demo.mp4 --out demo.md
gai transcribe call.mp3 --speakers
```

Agents get the same with the tools `meeting_notes`, `analyze_audio` and
`summarize_video` (on a blob or a workspace file); the **Meeting notes**
template uses them.

### API

| | |
| --- | --- |
| `POST /v1/blobs?name=...` | upload the recording (up to 500 MB) |
| `POST /v1/recordings` | `{"blob_id": "...", "kind": "meeting", "title": "...", "options": {"template": "standup", "context": "..."}}`; processed in the background |
| `GET /v1/recordings`, `GET /v1/recordings/{id}` | `status` is `processing`, `done` or `failed`; `result` has the transcript, `insights`, `notes` or `video`, and `markdown` |
| `GET /v1/recordings/templates` | the notes templates |
| `POST /v1/audio/transcriptions` | `"speakers": true` returns timed `segments` with speakers |
