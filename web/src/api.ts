// Typed client for the gai HTTP API.
//
// Shapes that map one-to-one onto the server's come from api.gen.ts, which
// `npm run gen:api` generates from /openapi.json; when the Rust API changes,
// regenerating makes this file (and every page) type-check against it.

import type { components } from './api.gen';

type Schemas = components['schemas'];
export type Session = Schemas['Session'];
export type Step = Schemas['Step'];
export type UsageTotal = Schemas['UsageTotal'];
export type ApiKey = Schemas['ApiKey'];
export type Org = Schemas['Org'];
export type OrgView = Schemas['OrgView'];
export type Membership = Schemas['Membership'];
export type Team = Schemas['Team'];
export type OrgRole = Schemas['Role'];
export type Owner = Schemas['Owner'];
export type Me = Schemas['Me'];
export type AuthConfig = Schemas['AuthConfig'];
export type AgentStats = Schemas['AgentStats'];
export type MessageHit = Schemas['MessageHit'];
export type Change = Schemas['Change'];
export type Extracted = Schemas['Extracted'];
export type DataConnection = Schemas['DataConnection'];
export type DataSchema = Schemas['Schema'];
export type QueryAnswer = Schemas['QueryAnswer'];
export type DashboardView = Schemas['DashboardView'];
export type DashboardPanel = Schemas['Panel'];
export type PanelResult = Schemas['PanelResult'];
export type DashboardResults = Schemas['DashboardResults'];
export type GraphView = Schemas['GraphView'];
export type GraphNodeView = Schemas['GraphNodeView'];
export type GraphBuild = Schemas['GraphBuild'];
export type AgentSummary = Schemas['AgentSummary'];
export type PendingApproval = Schemas['PendingApproval'];
export type McpServerInfo = Schemas['McpServerInfo'];
export type McpServerConfig = Schemas['McpServerConfig'];
export type Collection = Schemas['Collection'];
export type KnowledgeDocument = Schemas['Document'];
export type DocumentView = Schemas['DocumentView'];
export type SearchHit = Schemas['SearchHit'];
export type SearchMode = Schemas['SearchMode'];
export type Ingested = Schemas['Ingested'];
export type MemoryBlock = Schemas['MemoryBlock'];
export type BlockRevision = Schemas['BlockRevision'];
export type SkillView = Schemas['SkillView'];
export type SkillDetail = Schemas['SkillDetail'];
export type GraphNode = Schemas['GraphNode'];
export type WorkflowEntry = Schemas['WorkflowEntry'];
export type WorkflowView = Schemas['WorkflowView'];
export type WorkflowSpec = Schemas['WorkflowSpec'];
export type BundleReport = Schemas['ImportReport'];
export type Feedback = Schemas['Feedback'];
export type FeedbackSummary = Schemas['FeedbackSummary'];
export type EvalSet = Schemas['EvalSet'];
export type EvalSetSummary = Schemas['EvalSetSummary'];
export type EvalSetView = Schemas['EvalSetView'];
export type EvalRun = Schemas['EvalRun'];
export type EvalResult = Schemas['EvalResult'];
export type EvalReport = Schemas['EvalReport'];
export type EvalSummary = Schemas['Summary'];
export type EvalCompare = Schemas['CompareView'];
export type StartEval = Schemas['StartEval'];
export type BundleExport = Schemas['ExportOptions'];
export type Validation = Schemas['Validation'];
export type Trace = Schemas['Trace'];
export type WorkflowVersion = Schemas['WorkflowVersion'];
export type StoredBlob = Schemas['Blob'];
export type MediaResult = Schemas['MediaResult'];
export type TranscribeResult = Schemas['TranscribeResult'];
export type MediaDefaults = Schemas['MediaDefaults'];
export type TriggerView = Schemas['TriggerView'];
export type TriggerEvent = Schemas['TriggerEvent'];
export type Fired = Schemas['Fired'];
export type PersonaSpec = Schemas['PersonaSpec'];
export type PersonaSummary = Schemas['PersonaSummary'];
export type PersonaEntry = Schemas['PersonaEntry'];
export type PersonaVersion = Schemas['PersonaVersion'];
export type PersonaDefaults = Schemas['PersonaDefaultsView'];
export type Placement = Schemas['Placement'];

export type Role = 'user' | 'assistant';

export type MediaSource =
  | { kind: 'url'; url: string }
  | { kind: 'base64'; media_type: string; data: string }
  | { kind: 'blob'; id: string; media_type: string };

export type ContentBlock =
  | { type: 'text'; text: string }
  | { type: 'image'; source: MediaSource }
  | { type: 'audio'; source: MediaSource }
  | { type: 'tool_use'; id: string; name: string; input: unknown }
  | { type: 'tool_result'; tool_use_id: string; content: ContentBlock[]; is_error?: boolean }
  | { type: 'thinking'; text: string; signature?: string; origin?: string }
  | { type: 'redacted_thinking'; data: string; origin?: string };

export interface Message {
  role: Role;
  content: ContentBlock[];
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
}

export type RunStatus =
  | 'queued'
  | 'running'
  | 'awaiting_approval'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'budget_exceeded';

export interface Run {
  id: string;
  session_id: string;
  agent: string;
  agent_version: number | null;
  parent_run_id: string | null;
  status: RunStatus;
  input: ContentBlock[] | Record<string, unknown>;
  output: unknown;
  error: string | null;
  usage: Usage;
  cost: number;
  steps: number;
  metadata: {
    model?: string;
    spec?: AgentSpec;
    /** The persona the run spoke as (lists leave out its spec). */
    persona?: { name: string; version?: number; placement: Placement; spec?: PersonaSpec };
    [k: string]: unknown;
  };
  created_at: string;
  updated_at: string;
  finished_at: string | null;
  active?: boolean;
}



export interface StoredMessage {
  seq: number;
  run_id: string | null;
  message: Message;
  created_at: string;
}

export type ToolPolicy = 'auto' | 'approve' | 'deny';

export interface AgentSpec {
  name: string;
  description?: string;
  model?: string;
  system?: string;
  /** Persona it speaks as unless a chat picks another; `none` for none. */
  persona?: string;
  persona_placement?: Placement;
  tools?: string[];
  tool_policy?: Record<string, ToolPolicy>;
  reasoning?: { effort: string; budget_tokens?: number; expose?: boolean };
  max_tokens?: number;
  temperature?: number;
  limits?: { max_steps?: number; max_tokens?: number; max_cost_usd?: number; timeout_secs?: number };
  safety?: Safety;
  [k: string]: unknown;
}

export type Safety = Schemas['Safety'];
export type AuditEntry = Schemas['AuditEntry'];
export type Deleted = Schemas['Deleted'];


export interface AgentVersion {
  name: string;
  version: number;
  spec: AgentSpec;
  note: string | null;
  created_at: string;
}

export interface ModelInfo {
  id: string;
  provider: string;
  model: string;
  kind: 'chat' | 'embedding';
  capabilities: {
    tools: boolean;
    vision: boolean;
    reasoning: boolean;
    json_schema: boolean;
    prompt_caching: boolean;
    context_window?: number;
    max_output_tokens?: number;
  };
  pricing?: { input: number; output: number; cache_read?: number };
}

export type KeyState = { status: 'present' } | { status: 'not_needed' } | { status: 'missing'; env: string };

export interface CatalogEntry extends ModelInfo {
  key: KeyState;
}

export interface Provider {
  name: string;
  dialect: string;
  base_url: string | null;
  key: KeyState;
  models: string[];
}

export interface ToolInfo {
  spec: { name: string; description: string; input_schema: unknown };
  policy: ToolPolicy;
}

export interface ToolOutput {
  content: ContentBlock[];
  is_error: boolean;
}

/** One run event; `type` selects the other fields. */
export type RunEvent =
  | { type: 'run_started'; agent: string; agent_version: number | null; model: string; session_id: string }
  | { type: 'step_started'; step: number }
  | { type: 'step_replayed'; step: number; from_run_id: string }
  | { type: 'model_routed'; step: number; model: string; reason: string }
  | { type: 'text_delta'; text: string }
  | { type: 'reasoning_delta'; text: string }
  | { type: 'assistant_message'; message: Message }
  | { type: 'tool_call'; id: string; name: string; input: unknown; policy: ToolPolicy }
  | { type: 'approval_required'; id: string; name: string; input: unknown }
  | { type: 'approval_resolved'; id: string; approved: boolean; reason?: string }
  | { type: 'tool_result'; id: string; name: string; output: ToolOutput; duration_ms: number }
  | { type: 'usage'; step: number; model: string; usage: Usage; cost: number | null; total_usage: Usage; total_cost: number }
  | { type: 'compacted'; summarized_messages: number; summary_chars: number }
  | { type: 'tool_results_cleared'; cleared: number }
  | { type: 'warning'; message: string }
  | { type: 'workflow_started'; workflow: string; version: number | null; inputs: Record<string, unknown> }
  | { type: 'node_started'; node: string; kind: string }
  | { type: 'node_finished'; node: string; status: 'completed' | 'failed' | 'skipped'; output?: unknown; error?: string; duration_ms: number }
  | {
      type: 'run_finished';
      status: RunStatus;
      output?: unknown;
      error?: string;
      usage: Usage;
      cost: number;
      steps: number;
    };

export type EventEnvelope = RunEvent & { run_id: string; seq: number; ts: string };

/** Two versions of an agent or workflow and a unified diff between them. */
export interface VersionDiff {
  name: string;
  from: number;
  to: number;
  from_yaml: string;
  to_yaml: string;
  diff: string;
}

export interface StepEdit {
  step: number;
  content?: ContentBlock[];
  text?: string;
  tool_results?: Record<string, ToolOutput>;
}

export interface ReplayOverrides {
  model?: string;
  system_prompt?: string;
  tools?: string[];
  agent_version?: number;
  auto_approve?: boolean;
  step_edits?: StepEdit[];
}

/** `metadata.fork` on a run made by replaying another. */
export interface ForkInfo {
  run_id: string;
  from_step: number;
  overrides: ReplayOverrides;
}


export interface CloudProvider {
  name: string;
  title: string;
  description: string;
  key_url: string;
  connected: boolean;
  saved_key: string | null;
  suggested_model: string | null;
}

export interface OllamaStatus {
  reachable: boolean;
  base_url: string | null;
  models: string[];
}

export interface SetupStatus {
  needs_key: boolean;
  can_claim: boolean;
  has_chat_model: boolean;
  default_model: string | null;
  default_model_source: 'settings' | 'config' | 'detected' | null;
  ollama: OllamaStatus;
  providers: CloudProvider[];
}

export interface LibraryModel {
  name: string;
  description: string;
  size: string;
  tools: boolean;
  installed: boolean;
}

export interface PullProgress {
  model: string;
  state: 'pulling' | 'done' | 'failed';
  status: string;
  completed: number;
  total: number;
  error: string | null;
}

export interface Template {
  id: string;
  kind: 'agent' | 'workflow' | 'persona';
  title: string;
  description: string;
  needs: string[];
  yaml: string;
  installed: boolean;
}

export type Scope = Schemas['Scope'];

/** An organization's branding and limits (its `settings`). */
export interface OrgSettings {
  branding?: { product_name?: string | null; logo_url?: string | null; accent?: string | null };
  limits?: { budget_usd_per_month?: number | null; requests_per_minute?: number | null };
}



export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

const KEY_STORAGE = 'gai.apiKey';
const ORG_STORAGE = 'gai.org';

/** The organization a signed-in user works in (a key has its own). */
export function getOrg(): string | null {
  try {
    return localStorage.getItem(ORG_STORAGE);
  } catch {
    return null;
  }
}

export function setOrg(slug: string | null) {
  try {
    if (slug) localStorage.setItem(ORG_STORAGE, slug);
    else localStorage.removeItem(ORG_STORAGE);
  } catch {
    /* storage unavailable */
  }
}

/** The API key this browser uses, if any. */
export function getApiKey(): string | null {
  try {
    return localStorage.getItem(KEY_STORAGE);
  } catch {
    return null;
  }
}

export function setApiKey(key: string | null) {
  try {
    if (key) localStorage.setItem(KEY_STORAGE, key.trim());
    else localStorage.removeItem(KEY_STORAGE);
  } catch {
    /* storage unavailable: the key lasts for this page only */
  }
}

function headers(json: boolean): HeadersInit {
  const h: Record<string, string> = {};
  if (json) h['content-type'] = 'application/json';
  const key = getApiKey();
  if (key) h.authorization = `Bearer ${key}`;
  // A key belongs to one organization; a sign-in picks one.
  const org = getOrg();
  if (!key && org) h['x-gai-org'] = org;
  return h;
}

/** Tell the app the server wants a (different) key. */
function unauthorized(res: Response) {
  if (res.status === 401) window.dispatchEvent(new CustomEvent('gai:unauthorized'));
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: headers(body !== undefined),
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  unauthorized(res);
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const err = await res.json();
      message = err?.error?.message ?? message;
    } catch {
      /* not JSON */
    }
    throw new ApiError(res.status, message);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

const get = <T,>(path: string) => request<T>('GET', path);

/** A request whose answer is a file. */
async function download(method: string, path: string, body?: unknown): Promise<Blob> {
  const res = await fetch(path, { method, headers: headers(body !== undefined), body: body === undefined ? undefined : JSON.stringify(body) });
  unauthorized(res);
  if (!res.ok) throw new ApiError(res.status, `${res.status} ${res.statusText}`);
  return res.blob();
}

function versionPair(from?: number, to?: number) {
  const p = new URLSearchParams();
  if (from !== undefined) p.set('from', String(from));
  if (to !== undefined) p.set('to', String(to));
  return p.toString();
}

export const api = {
  setupStatus: () => get<SetupStatus>('/v1/setup/status'),
  claim: () => request<{ key: ApiKey; secret: string }>('POST', '/v1/setup/claim'),
  settings: () => get<{ default_model: string | null; default_model_source: SetupStatus['default_model_source'] }>('/v1/settings'),
  setDefaultModel: (default_model: string | null) =>
    request<{ default_model: string | null; default_model_source: SetupStatus['default_model_source'] }>('PUT', '/v1/settings', {
      default_model,
    }),
  setProviderKey: (name: string, key: string) =>
    request<{ provider: string; connected: boolean; saved_key: string | null }>('PUT', `/v1/providers/${encodeURIComponent(name)}/key`, { key }),
  forgetProviderKey: (name: string) =>
    request<{ provider: string; connected: boolean; saved_key: string | null }>('DELETE', `/v1/providers/${encodeURIComponent(name)}/key`),
  testProvider: (name: string, model?: string) =>
    request<{ ok: boolean; model: string; message: string }>('POST', `/v1/providers/${encodeURIComponent(name)}/test`, { model }),
  ollamaLibrary: () => get<{ status: OllamaStatus; models: LibraryModel[] }>('/v1/providers/ollama/library'),
  ollamaPull: (model: string) => request<PullProgress>('POST', '/v1/providers/ollama/pull', { model }),
  ollamaPulls: () => get<PullProgress[]>('/v1/providers/ollama/pulls'),
  templates: () => get<Template[]>('/v1/templates'),
  installTemplate: (id: string) =>
    request<{ kind: string; name: string; also: string[] }>('POST', `/v1/templates/${encodeURIComponent(id)}/install`),
  health: () => get<{ status: string; version: string; default_model: string | null; auth: 'open' | 'keys' }>('/v1/health'),
  me: () => get<Me>('/v1/me'),
  authConfig: () => get<AuthConfig>('/v1/auth/config'),
  logout: () => request<{ ok: boolean }>('POST', '/v1/auth/logout'),
  orgs: () => get<OrgView[]>('/v1/orgs'),
  createOrg: (slug: string, name: string, owner_email?: string) => request<Org>('POST', '/v1/orgs', { slug, name, owner_email }),
  updateOrg: (slug: string, body: { name?: string; settings?: OrgSettings }) => request<Org>('PATCH', `/v1/orgs/${encodeURIComponent(slug)}`, body),
  deleteOrg: (slug: string) => request<{ ok: boolean }>('DELETE', `/v1/orgs/${encodeURIComponent(slug)}`),
  members: () => get<Membership[]>('/v1/members'),
  setMember: (who: string, role: OrgRole) => request<Membership>('PUT', `/v1/members/${encodeURIComponent(who)}`, { role }),
  removeMember: (who: string) => request<{ ok: boolean }>('DELETE', `/v1/members/${encodeURIComponent(who)}`),
  teams: () => get<Team[]>('/v1/teams'),
  createTeam: (name: string) => request<Team>('POST', '/v1/teams', { name }),
  deleteTeam: (team: string) => request<{ ok: boolean }>('DELETE', `/v1/teams/${encodeURIComponent(team)}`),
  addTeamMember: (team: string, who: string) => request<Team>('PUT', `/v1/teams/${encodeURIComponent(team)}/members/${encodeURIComponent(who)}`),
  removeTeamMember: (team: string, who: string) =>
    request<Team>('DELETE', `/v1/teams/${encodeURIComponent(team)}/members/${encodeURIComponent(who)}`),
  owners: () => get<Owner[]>('/v1/owners'),
  setOwner: (kind: string, name: string, team: string | null) =>
    request<unknown>('PUT', `/v1/owners/${encodeURIComponent(kind)}/${encodeURIComponent(name)}`, { team }),
  agentStats: (since?: string) => get<AgentStats[]>(`/v1/analytics/agents${since ? `?since=${encodeURIComponent(since)}` : ''}`),
  searchSessions: (q: string, agent?: string) =>
    get<MessageHit[]>(`/v1/sessions/search?q=${encodeURIComponent(q)}${agent ? `&agent=${encodeURIComponent(agent)}` : ''}`),
  changes: (target: string) => get<Change[]>(`/v1/changes?target=${encodeURIComponent(target)}`),
  secrets: () => get<string[]>('/v1/secrets'),
  putSecret: (name: string, value: string) => request<{ ok: boolean }>('PUT', `/v1/secrets/${encodeURIComponent(name)}`, { value }),
  deleteSecret: (name: string) => request<{ ok: boolean }>('DELETE', `/v1/secrets/${encodeURIComponent(name)}`),
  keys: () => get<ApiKey[]>('/v1/keys'),
  createKey: (name: string, scopes: Scope[], budget_usd?: number, team?: string) =>
    request<{ key: ApiKey; secret: string }>('POST', '/v1/keys', { name, scopes, budget_usd, team }),
  revokeKey: (id: string) => request<void>('DELETE', `/v1/keys/${id}`),
  models: () => get<CatalogEntry[]>('/v1/models'),
  providers: () => get<Provider[]>('/v1/providers'),
  tools: () => get<ToolInfo[]>('/v1/tools'),
  agents: () => get<{ summary: AgentSummary; spec: AgentSpec | null }[]>('/v1/agents'),
  defaultAgent: () => get<AgentSpec>('/v1/agents/default'),
  agentVersions: (name: string) => get<AgentVersion[]>(`/v1/agents/${encodeURIComponent(name)}/versions`),
  saveAgent: (spec: AgentSpec, note?: string) =>
    request<{ name: string; version: number }>(
      'POST',
      `/v1/agents${note ? `?note=${encodeURIComponent(note)}` : ''}`,
      spec,
    ),
  deleteAgent: (name: string) => request<void>('DELETE', `/v1/agents/${encodeURIComponent(name)}`),
  agentDiff: (name: string, from?: number, to?: number) =>
    get<VersionDiff>(`/v1/agents/${encodeURIComponent(name)}/diff?${versionPair(from, to)}`),
  rollbackAgent: (name: string, version: number) =>
    request<{ name: string; version: number }>('POST', `/v1/agents/${encodeURIComponent(name)}/rollback`, { version }),
  setAgentLabel: (name: string, label: string, version: number | null) =>
    version === null
      ? request<unknown>('DELETE', `/v1/agents/${encodeURIComponent(name)}/labels/${encodeURIComponent(label)}`)
      : request<unknown>('POST', `/v1/agents/${encodeURIComponent(name)}/labels/${encodeURIComponent(label)}`, { version }),
  personas: () => get<PersonaEntry[]>('/v1/personas'),
  savePersona: (spec: PersonaSpec, note?: string) =>
    request<{ name: string; version: number }>('POST', `/v1/personas${note ? `?note=${encodeURIComponent(note)}` : ''}`, spec),
  deletePersona: (name: string) => request<void>('DELETE', `/v1/personas/${encodeURIComponent(name)}`),
  personaVersions: (name: string) => get<PersonaVersion[]>(`/v1/personas/${encodeURIComponent(name)}/versions`),
  personaDiff: (name: string, from?: number, to?: number) =>
    get<VersionDiff>(`/v1/personas/${encodeURIComponent(name)}/diff?${versionPair(from, to)}`),
  rollbackPersona: (name: string, version: number) =>
    request<{ name: string; version: number }>('POST', `/v1/personas/${encodeURIComponent(name)}/rollback`, { version }),
  setPersonaLabel: (name: string, label: string, version: number | null) =>
    version === null
      ? request<unknown>('DELETE', `/v1/personas/${encodeURIComponent(name)}/labels/${encodeURIComponent(label)}`)
      : request<unknown>('POST', `/v1/personas/${encodeURIComponent(name)}/labels/${encodeURIComponent(label)}`, { version }),
  renderPersona: (spec: PersonaSpec) => request<{ text: string }>('POST', '/v1/personas/render', spec),
  personaDefaults: () => get<PersonaDefaults>('/v1/personas/defaults'),
  setPersonaDefaults: (body: { persona: string | null; placement?: Placement } | { reset: true }) =>
    request<PersonaDefaults>('PUT', '/v1/personas/defaults', body),
  sessions: (limit = 100) => get<Session[]>(`/v1/sessions?limit=${limit}`),
  messages: (id: string) => get<StoredMessage[]>(`/v1/sessions/${id}/messages`),
  deleteSession: (id: string) => request<void>('DELETE', `/v1/sessions/${id}`),
  runs: (q: { session_id?: string; status?: string; fork_of?: string; limit?: number } = {}) => {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(q)) if (v !== undefined) p.set(k, String(v));
    return get<Run[]>(`/v1/runs?${p}`);
  },
  run: (id: string) => get<Run>(`/v1/runs/${id}`),
  steps: (id: string) => get<Step[]>(`/v1/runs/${id}/steps`),
  cancel: (id: string) => request<unknown>('POST', `/v1/runs/${id}/cancel`),
  approvals: () => get<PendingApproval[]>('/v1/approvals'),
  decide: (runId: string, callId: string, approved: boolean, reason?: string) =>
    request<unknown>('POST', `/v1/runs/${runId}/approvals/${encodeURIComponent(callId)}`, { approved, reason }),
  usage: () => get<UsageTotal[]>('/v1/usage'),
  replay: (id: string, from_step: number, overrides: ReplayOverrides) =>
    request<{ run_id: string; session_id: string }>('POST', `/v1/runs/${id}/replay`, { from_step, overrides, mode: 'background' }),
  resume: (id: string) => request<{ run_id: string; session_id: string }>('POST', `/v1/runs/${id}/resume`),
  decideWith: (runId: string, callId: string, input: unknown) =>
    request<unknown>('POST', `/v1/runs/${runId}/approvals/${encodeURIComponent(callId)}`, { approved: true, input }),
  mcp: () => get<McpServerInfo[]>('/v1/mcp'),
  addMcp: (name: string, config: McpServerConfig) => request<McpServerInfo>('POST', '/v1/mcp', { name, config }),
  removeMcp: (name: string) => request<void>('DELETE', `/v1/mcp/${encodeURIComponent(name)}`),
  reconnectMcp: (name: string) => request<McpServerInfo>('POST', `/v1/mcp/${encodeURIComponent(name)}/reconnect`),
  hooks: () => get<string[]>('/v1/hooks'),
  mcpSignIn: (name: string) =>
    request<{ auth_url: string }>('POST', `/v1/mcp/${encodeURIComponent(name)}/sign-in`, {
      redirect_uri: `${location.origin}/v1/mcp/oauth/callback`,
    }),
  mcpSignOut: (name: string) => request<McpServerInfo>('DELETE', `/v1/mcp/${encodeURIComponent(name)}/sign-in`),

  collections: (all = false) => get<Collection[]>(`/v1/collections${all ? '?all=true' : ''}`),
  createCollection: (body: { name: string; description?: string; embedding_model?: string; chunk_tokens?: number }) =>
    request<Collection>('POST', '/v1/collections', body),
  deleteCollection: (name: string) => request<void>('DELETE', `/v1/collections/${encodeURIComponent(name)}`),
  documents: (name: string) => get<KnowledgeDocument[]>(`/v1/collections/${encodeURIComponent(name)}/documents?limit=500`),
  document: (name: string, id: string) => get<DocumentView>(`/v1/collections/${encodeURIComponent(name)}/documents/${id}`),
  addDocument: (
    name: string,
    body: { source?: string; title?: string; text?: string; url?: string; content_base64?: string; filename?: string; content_type?: string },
  ) => request<Ingested>('POST', `/v1/collections/${encodeURIComponent(name)}/documents`, body),
  deleteDocument: (name: string, id: string) =>
    request<void>('DELETE', `/v1/collections/${encodeURIComponent(name)}/documents/${id}`),
  dashboards: () => get<DashboardView[]>('/v1/dashboards'),
  saveDashboard: (yaml: string) => request<DashboardView>('POST', '/v1/dashboards', { yaml }),
  deleteDashboard: (name: string) => request<{ ok: boolean }>('DELETE', `/v1/dashboards/${encodeURIComponent(name)}`),
  refreshDashboard: (name: string) => request<DashboardResults>('POST', `/v1/dashboards/${encodeURIComponent(name)}/refresh`),
  connections: () => get<DataConnection[]>('/v1/connections'),
  saveConnection: (yaml: string) => request<DataConnection>('POST', '/v1/connections', { yaml }),
  deleteConnection: (name: string) => request<{ ok: boolean }>('DELETE', `/v1/connections/${encodeURIComponent(name)}`),
  connectionSchema: (name: string, refresh = false) =>
    get<DataSchema>(`/v1/connections/${encodeURIComponent(name)}/schema${refresh ? '?refresh=true' : ''}`),
  query: (name: string, sql: string, dry_run = false) =>
    request<QueryAnswer>('POST', `/v1/connections/${encodeURIComponent(name)}/query`, { sql, dry_run }),
  extract: (name: string, fields: Record<string, string>) =>
    request<Extracted>('POST', `/v1/collections/${encodeURIComponent(name)}/extract`, { fields }),
  extractCsv: (name: string, fields: Record<string, string>) =>
    download('POST', `/v1/collections/${encodeURIComponent(name)}/extract?format=csv`, { fields }),
  graph: (name: string, q?: string) =>
    get<GraphView>(`/v1/collections/${encodeURIComponent(name)}/graph${q ? `?q=${encodeURIComponent(q)}` : ''}`),
  graphNode: (name: string, id: number) => get<GraphNodeView>(`/v1/collections/${encodeURIComponent(name)}/graph/nodes/${id}`),
  buildGraph: (name: string) => request<GraphBuild | null>('POST', `/v1/collections/${encodeURIComponent(name)}/graph?wait=true`, {}),
  search: (name: string, query: string, mode: SearchMode, k = 8) =>
    request<SearchHit[]>('POST', `/v1/collections/${encodeURIComponent(name)}/search`, { query, mode, k }),

  blocks: (scope?: string) => get<MemoryBlock[]>(`/v1/memory/blocks${scope ? `?scope=${encodeURIComponent(scope)}` : ''}`),
  createBlock: (body: { scope: string; label: string; value: string; limit: number; description?: string; read_only?: boolean }) =>
    request<MemoryBlock>('POST', '/v1/memory/blocks', body),
  editBlock: (id: string, body: { value?: string; description?: string; limit?: number; read_only?: boolean }) =>
    request<MemoryBlock>('PATCH', `/v1/memory/blocks/${id}`, body),
  deleteBlock: (id: string) => request<void>('DELETE', `/v1/memory/blocks/${id}`),
  blockHistory: (id: string) => get<BlockRevision[]>(`/v1/memory/blocks/${id}/history`),

  skills: () => get<SkillView[]>('/v1/skills'),
  skill: (name: string) => get<SkillDetail>(`/v1/skills/${encodeURIComponent(name)}`),
  saveSkill: (content: string, files: Record<string, string> = {}) =>
    request<{ name: string; version: number }>('POST', '/v1/skills', { content, files }),
  deleteSkill: (name: string) => request<void>('DELETE', `/v1/skills/${encodeURIComponent(name)}`),

  workflows: () => get<WorkflowEntry[]>('/v1/workflows'),
  workflow: (name: string) => get<WorkflowView>(`/v1/workflows/${encodeURIComponent(name)}`),
  saveWorkflow: (spec: WorkflowSpec, note?: string) =>
    request<{ name: string; version: number }>('POST', `/v1/workflows${note ? `?note=${encodeURIComponent(note)}` : ''}`, spec),
  deleteWorkflow: (name: string) => request<void>('DELETE', `/v1/workflows/${encodeURIComponent(name)}`),
  workflowVersions: (name: string) => get<WorkflowVersion[]>(`/v1/workflows/${encodeURIComponent(name)}/versions`),
  workflowDiff: (name: string, from?: number, to?: number) =>
    get<VersionDiff>(`/v1/workflows/${encodeURIComponent(name)}/diff?${versionPair(from, to)}`),
  rollbackWorkflow: (name: string, version: number) =>
    request<{ name: string; version: number }>('POST', `/v1/workflows/${encodeURIComponent(name)}/rollback`, { version }),
  setWorkflowLabel: (name: string, label: string, version: number | null) =>
    version === null
      ? request<unknown>('DELETE', `/v1/workflows/${encodeURIComponent(name)}/labels/${encodeURIComponent(label)}`)
      : request<unknown>('POST', `/v1/workflows/${encodeURIComponent(name)}/labels/${encodeURIComponent(label)}`, { version }),
  validateWorkflow: (spec: unknown) => request<Validation>('POST', '/v1/workflows/validate', spec),
  runWorkflow: (name: string, inputs: Record<string, unknown>) =>
    request<{ run_id: string; session_id: string }>('POST', `/v1/workflows/${encodeURIComponent(name)}/runs`, {
      inputs,
      mode: 'background',
    }),
  workflowRuns: (name: string) => get<Run[]>(`/v1/runs?agent=${encodeURIComponent(`workflow:${name}`)}&limit=20`),
  trace: (id: string) => get<Trace>(`/v1/runs/${id}/trace`),

  triggers: () => get<TriggerView[]>('/v1/triggers'),
  createTrigger: (spec: unknown) => request<{ trigger: TriggerView; secret: string | null }>('POST', '/v1/triggers', spec),
  updateTrigger: (id: string, spec: unknown) => request<TriggerView>('PUT', `/v1/triggers/${encodeURIComponent(id)}`, spec),
  deleteTrigger: (id: string) => request<void>('DELETE', `/v1/triggers/${encodeURIComponent(id)}`),
  enableTrigger: (id: string) => request<TriggerView>('POST', `/v1/triggers/${encodeURIComponent(id)}/enable`),
  disableTrigger: (id: string) => request<TriggerView>('POST', `/v1/triggers/${encodeURIComponent(id)}/disable`),
  fireTrigger: (id: string) => request<Fired>('POST', `/v1/triggers/${encodeURIComponent(id)}/fire`),
  triggerSecret: (id: string) => request<{ secret: string }>('POST', `/v1/triggers/${encodeURIComponent(id)}/secret`),
  previewTrigger: (spec: unknown) =>
    request<{ ok: boolean; error?: string | null; schedule_text?: string | null; next: string[] }>('POST', '/v1/triggers/preview', spec),
  triggerEvents: (id: string) => get<TriggerEvent[]>(`/v1/triggers/${encodeURIComponent(id)}/events?limit=30`),

  mediaDefaults: () => get<MediaDefaults>('/v1/media'),
  blobs: (q: { run_id?: string; limit?: number } = {}) => {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(q)) if (v !== undefined) p.set(k, String(v));
    return get<StoredBlob[]>(`/v1/blobs?${p}`);
  },
  deleteBlob: (id: string) => request<void>('DELETE', `/v1/blobs/${id}`),
  /** A blob's bytes (fetched with the API key, so it can be shown). */
  blobBytes: async (id: string): Promise<Blob> => {
    const res = await fetch(`/v1/blobs/${id}`, { headers: headers(false) });
    if (!res.ok) throw new ApiError(res.status, `blob ${id}: HTTP ${res.status}`);
    return res.blob();
  },
  uploadBlob: async (file: Blob, name?: string): Promise<StoredBlob> => {
    const res = await fetch(`/v1/blobs${name ? `?name=${encodeURIComponent(name)}` : ''}`, {
      method: 'POST',
      headers: { ...(headers(false) as Record<string, string>), 'content-type': file.type || 'application/octet-stream' },
      body: file,
    });
    if (!res.ok) throw new ApiError(res.status, await res.text());
    return res.json();
  },
  audit: (q: { actor?: string; target?: string; limit?: number } = {}) => {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(q)) if (v) p.set(k, String(v));
    return get<AuditEntry[]>(`/v1/audit?${p}`);
  },
  /** The audit log as CSV, fetched with the key so it can be saved. */
  auditCsv: async (q: { actor?: string; target?: string } = {}): Promise<Blob> => {
    const p = new URLSearchParams({ format: 'csv', limit: '10000' });
    for (const [k, v] of Object.entries(q)) if (v) p.set(k, String(v));
    const res = await fetch(`/v1/audit?${p}`, { headers: headers(false) });
    unauthorized(res);
    if (!res.ok) throw new ApiError(res.status, `HTTP ${res.status}`);
    return res.blob();
  },
  forgetUser: (id: string) => request<Deleted>('DELETE', `/v1/users/${encodeURIComponent(id)}`),
  sweepRetention: () => request<{ agents: (Deleted & { agent: string })[] }>('POST', '/v1/retention/sweep'),
  rate: (runId: string, rating: 1 | -1, comment?: string, tags: string[] = []) =>
    request<Feedback>('POST', `/v1/runs/${runId}/feedback`, { rating, comment, tags }),
  unrate: (runId: string) => request<{ ok: boolean }>('DELETE', `/v1/runs/${runId}/feedback`),
  runFeedback: (runId: string) => get<Feedback[]>(`/v1/runs/${runId}/feedback`),
  feedback: (q: { agent?: string; rating?: 1 | -1; limit?: number } = {}) => {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(q)) if (v !== undefined) p.set(k, String(v));
    return get<Feedback[]>(`/v1/feedback?${p}`);
  },
  feedbackSummary: (agent?: string) => get<FeedbackSummary[]>(`/v1/feedback/summary${agent ? `?agent=${encodeURIComponent(agent)}` : ''}`),
  evalSets: () => get<EvalSetSummary[]>('/v1/evals'),
  evalSet: (name: string) => get<EvalSetView>(`/v1/evals/${encodeURIComponent(name)}`),
  saveEvalSet: (yaml: string, note?: string) =>
    request<{ name: string; version: number }>('POST', `/v1/evals${note ? `?note=${encodeURIComponent(note)}` : ''}`, { yaml }),
  deleteEvalSet: (name: string) => request<{ ok: boolean }>('DELETE', `/v1/evals/${encodeURIComponent(name)}`),
  startEval: (name: string, body: Partial<StartEval>) =>
    request<{ eval_run_ids: string[] }>('POST', `/v1/evals/${encodeURIComponent(name)}/runs`, body),
  evalRuns: (set?: string) => get<EvalRun[]>(`/v1/eval-runs${set ? `?set=${encodeURIComponent(set)}` : ''}`),
  evalReport: (id: string) => get<EvalReport>(`/v1/eval-runs/${id}`),
  deleteEvalRun: (id: string) => request<{ ok: boolean }>('DELETE', `/v1/eval-runs/${id}`),
  compareEvals: (baseline: string, candidate: string) =>
    get<EvalCompare>(`/v1/eval-runs/compare?baseline=${baseline}&candidate=${candidate}`),
  /** A .gaibundle (zip) of the chosen items and what they use. */
  exportBundle: async (opts: Partial<BundleExport>): Promise<Blob> => {
    const res = await fetch('/v1/bundles/export', { method: 'POST', headers: headers(true), body: JSON.stringify(opts) });
    unauthorized(res);
    if (!res.ok) throw new ApiError(res.status, (await res.json().catch(() => null))?.error?.message ?? `HTTP ${res.status}`);
    return res.blob();
  },
  /** What a bundle holds and what this server is missing, or (with `install`) import it. */
  bundle: async (file: Blob, install?: { skip_existing?: boolean }): Promise<BundleReport> => {
    const path = install ? `/v1/bundles/import${install.skip_existing ? '?skip_existing=true' : ''}` : '/v1/bundles/inspect';
    const res = await fetch(path, {
      method: 'POST',
      headers: { ...(headers(false) as Record<string, string>), 'content-type': 'application/zip' },
      body: file,
    });
    unauthorized(res);
    if (!res.ok) throw new ApiError(res.status, (await res.json().catch(() => null))?.error?.message ?? `HTTP ${res.status}`);
    return res.json();
  },
  transcribe: (body: { audio_base64?: string; media_type?: string; blob_id?: string; model?: string; language?: string }) =>
    request<TranscribeResult>('POST', '/v1/audio/transcriptions', body),
  speak: (body: { text: string; model?: string; voice?: string; format?: string }) =>
    request<MediaResult>('POST', '/v1/audio/speech', body),
  images: (body: { prompt: string; model?: string; size?: string; n?: number; quality?: string }) =>
    request<MediaResult>('POST', '/v1/images/generations', body),
};

/** A file's contents as base64, for uploads. */
export function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ''));
    r.onerror = () => reject(r.error ?? new Error('could not read the file'));
    r.readAsDataURL(file);
  });
}

export interface StartRun {
  agent?: string | AgentSpec;
  input: string | ContentBlock[];
  session_id?: string;
  model?: string;
  /** A stored persona, an unsaved one to try, or `none`. */
  persona?: string | PersonaSpec;
  auto_approve?: boolean;
}

/** Parse a `text/event-stream` body, calling `onEvent` for each event. */
async function readSse(res: Response, onEvent: (e: EventEnvelope) => void) {
  unauthorized(res);
  if (!res.ok || !res.body) {
    let message = `${res.status} ${res.statusText}`;
    try {
      message = (await res.json())?.error?.message ?? message;
    } catch {
      /* not JSON */
    }
    throw new ApiError(res.status, message);
  }
  const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
  let buf = '';
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += value;
    let cut: number;
    while ((cut = buf.indexOf('\n\n')) >= 0) {
      const chunk = buf.slice(0, cut);
      buf = buf.slice(cut + 2);
      const data = chunk
        .split('\n')
        .filter((l) => l.startsWith('data:'))
        .map((l) => l.slice(5).replace(/^ /, ''))
        .join('\n');
      if (data) onEvent(JSON.parse(data) as EventEnvelope);
    }
  }
}

/** Start a run and stream its events until it finishes. */
export async function streamRun(body: StartRun, onEvent: (e: EventEnvelope) => void, signal?: AbortSignal) {
  const res = await fetch('/v1/runs', {
    method: 'POST',
    headers: headers(true),
    body: JSON.stringify({ ...body, mode: 'stream' }),
    signal,
  });
  await readSse(res, onEvent);
}

/** Stored then live events of a run. */
export async function followRun(id: string, onEvent: (e: EventEnvelope) => void, signal?: AbortSignal) {
  const res = await fetch(`/v1/runs/${id}/events`, { signal, headers: headers(false) });
  await readSse(res, onEvent);
}
