// Plain-language names for tools, models and providers, in one place so the
// whole dashboard says things the same way. Raw names stay available in
// tooltips and in developer details.

const TOOLS: Record<string, { label: string; verb: (i: Record<string, unknown>) => string }> = {
  calculator: { label: 'Calculator', verb: (i) => `Calculate ${str(i.expression)}` },
  current_time: { label: 'Current time', verb: () => 'Check the time' },
  fs_read: { label: 'Read a file', verb: (i) => `Read ${str(i.path, 'a file')}` },
  fs_write: { label: 'Write a file', verb: (i) => `Write ${str(i.path, 'a file')}` },
  fs_edit: { label: 'Edit a file', verb: (i) => `Edit ${str(i.path, 'a file')}` },
  fs_list: { label: 'List files', verb: (i) => `List files in ${str(i.path, 'the workspace')}` },
  http_fetch: { label: 'Open a web page', verb: (i) => `Open ${str(i.url, 'a web page')}` },
  shell: { label: 'Run a command', verb: (i) => `Run ${str(i.command, 'a command')}` },
  code_exec: {
    label: 'Run code',
    verb: (i) => `Run ${({ node: 'JavaScript', javascript: 'JavaScript', js: 'JavaScript', bash: 'a script', sh: 'a script', shell: 'a script' } as Record<string, string>)[String(i.language)] ?? 'Python'} code`,
  },
  browser_navigate: { label: 'Browse to a page', verb: (i) => `Open ${str(i.url, 'a web page')} in the browser` },
  browser_read: { label: 'Read the page', verb: () => 'Read the page' },
  browser_click: { label: 'Click on the page', verb: (i) => `Click ${target(i)}` },
  browser_type: { label: 'Type on the page', verb: (i) => `Type ${str(i.text)} into ${target(i)}` },
  browser_submit: { label: 'Submit a form', verb: (i) => `Submit the form at ${target(i)}` },
  browser_extract: { label: 'Collect from the page', verb: (i) => `Collect ${str(i.selector, 'items')} from the page` },
  browser_screenshot: { label: 'Screenshot the page', verb: () => 'Take a screenshot of the page' },
  rag_search: { label: 'Search your documents', verb: (i) => `Search documents for ${str(i.query)}` },
  memory_search: { label: 'Search memories', verb: (i) => `Search memories for ${str(i.query)}` },
  remember: { label: 'Remember something', verb: () => 'Remember this' },
  memory_append: { label: 'Add to memory', verb: () => 'Update memory' },
  memory_replace: { label: 'Change memory', verb: () => 'Update memory' },
  memory_rethink: { label: 'Rewrite memory', verb: () => 'Rewrite memory' },
  load_skill: { label: 'Use a skill', verb: (i) => `Use the ${str(i.name, 'skill')} skill` },
  image_gen: { label: 'Make an image', verb: (i) => `Make an image of ${str(i.prompt)}` },
  speak: { label: 'Speak text aloud', verb: () => 'Turn text into speech' },
  transcribe: { label: 'Transcribe audio', verb: () => 'Transcribe audio' },
  meeting_notes: { label: 'Take meeting notes', verb: (i) => `Take notes on ${str(i.title ?? i.path, 'the meeting')}` },
  analyze_audio: { label: 'Analyse a recording', verb: (i) => `Analyse ${str(i.title ?? i.path, 'the recording')}` },
  summarize_video: { label: 'Summarize a video', verb: (i) => `Summarize ${str(i.title ?? i.path, 'the video')}` },
};

function str(v: unknown, fallback = ''): string {
  if (typeof v !== 'string' || !v.trim()) return fallback;
  const s = v.trim().replace(/\s+/g, ' ');
  return s.length > 70 ? `“${s.slice(0, 69)}…”` : `“${s}”`;
}

/** The element a browser tool acts on: `[3]` or its selector. */
const target = (i: Record<string, unknown>) =>
  i.ref != null && String(i.ref).trim() ? `element [${String(i.ref).replace(/[[\]]/g, '')}]` : str(i.selector, 'an element');

const words = (s: string) => {
  const t = s.replace(/[_-]+/g, ' ').trim();
  return t.charAt(0).toUpperCase() + t.slice(1);
};

/** A tool's name for people: `mcp__atlassian__create_issue` → "Atlassian · Create issue". */
export function toolLabel(name: string): string {
  if (TOOLS[name]) return TOOLS[name].label;
  if (name.startsWith('mcp__') && name.endsWith('__*')) return `Everything from ${words(name.slice(5, -3))}`;
  const mcp = /^mcp__(.+?)__(.+)$/.exec(name);
  if (mcp) return `${words(mcp[1])} · ${words(mcp[2])}`;
  const ask = /^ask_(.+)$/.exec(name);
  if (ask) return `Ask ${words(ask[1])}`;
  const a2a = /^a2a__(.+)$/.exec(name);
  if (a2a) return `Ask ${words(a2a[1])} (remote)`;
  if (name.endsWith('*')) return `All “${name.slice(0, -1)}” tools`;
  return words(name);
}

/** What a tool call does, as a short sentence: `Read “poem.txt”`. */
export function toolSentence(name: string, input: unknown): string {
  const i = (input && typeof input === 'object' ? input : {}) as Record<string, unknown>;
  if (TOOLS[name]) return TOOLS[name].verb(i);
  const ask = /^ask_(.+)$/.exec(name);
  if (ask) return `Ask ${words(ask[1])}: ${str(i.task)}`;
  return toolLabel(name);
}

const LOCAL = new Set(['ollama', 'lmstudio', 'vllm', 'llamacpp']);

const PROVIDERS: Record<string, string> = {
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  gemini: 'Google',
  openrouter: 'OpenRouter',
  ollama: 'this computer',
  groq: 'Groq',
  mistral: 'Mistral',
  deepseek: 'DeepSeek',
  together: 'Together',
  xai: 'xAI',
  elevenlabs: 'ElevenLabs',
};

export const providerLabel = (p: string) => PROVIDERS[p] ?? words(p);
export const isLocalProvider = (p: string) => LOCAL.has(p);

/** `claude-sonnet-5-5` → "Claude Sonnet 5.5", `gemma3:1b` → "Gemma3 1B". */
function prettyModel(m: string): string {
  const base = m.split('/').pop() ?? m;
  const [name, tag] = base.split(':');
  const parts = name.split('-');
  const out: string[] = [];
  for (const p of parts) {
    const prev = out[out.length - 1];
    if (/^\d+$/.test(p) && prev && /\d$/.test(prev)) out[out.length - 1] = `${prev}.${p}`;
    else out.push(/^\d/.test(p) ? p : p.charAt(0).toUpperCase() + p.slice(1));
  }
  const t = tag && tag !== 'latest' ? ` ${tag.toUpperCase()}` : '';
  return `${out.join(' ')}${t}`;
}

/** A model address for people: `router/auto` → "Automatic", `mock/echo` → "Demo (not a real AI)". */
export function modelLabel(id: string | null | undefined): string {
  if (!id) return 'No model yet';
  if (id === 'router/auto') return 'Automatic (picks a model for each message)';
  if (id.startsWith('router/')) return `Automatic: ${words(id.slice(7))}`;
  if (id.startsWith('mock/')) return 'Demo (not a real AI)';
  const slash = id.indexOf('/');
  if (slash < 0) return prettyModel(id);
  const provider = id.slice(0, slash);
  const model = id.slice(slash + 1);
  const where = LOCAL.has(provider) ? 'on this computer' : providerLabel(provider);
  return `${prettyModel(model)} · ${where}`;
}

/** Run states in everyday words. */
export const statusLabel: Record<string, string> = {
  queued: 'waiting to start',
  running: 'working',
  awaiting_approval: 'needs your OK',
  completed: 'done',
  failed: 'failed',
  cancelled: 'stopped',
  budget_exceeded: 'over budget',
};
