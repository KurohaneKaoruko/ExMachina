/** REST 客户端 —— 契约见 docs/04 §7 */
import type { AgentDefinition, ChatMessage, EvidenceItem, Session, TaskGraph } from "./types";

function authHeaders(): Record<string, string> {
  const key = localStorage.getItem("exm.key") ?? "";
  return key ? { "X-Auth-Key": key } : {};
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(`/api${path}`, {
    headers: { "Content-Type": "application/json", ...authHeaders() },
    ...init,
  });
  if (resp.status === 401) {
    // 密钥失效：清除并回到登录门（登录门自身的 verify 不触发重载，交由调用方处理）
    if (path !== "/auth/verify") {
      localStorage.removeItem("exm.key");
      window.location.reload();
    }
    throw new Error("401 需要访问密钥");
  }
  if (!resp.ok && resp.status !== 202) {
    const body = await resp.text();
    throw new Error(`${init?.method ?? "GET"} ${path} -> ${resp.status} ${body.slice(0, 200)}`);
  }
  return (await resp.json()) as T;
}

export interface GatewayConfig {
  llm: { baseUrl: string; apiKey: string; orchModel: string; unitModel: string };
  maxConcurrency: number;
  mock: boolean;
}

/** 配置 Schema（后端 config_schema() 输出，设置页据此渲染 ⇒ 新增配置项零 UI 改动） */
export interface ConfigSchemaField {
  key: string;
  label: string;
  kind: "string" | "password" | "number" | "boolean";
  default: string;
  required: boolean;
  help?: string;
  min?: number;
  max?: number;
}

export interface ConfigSchemaGroup {
  key: string;
  label: string;
  fields: ConfigSchemaField[];
}

export interface ConfigSchema {
  configVersion: number;
  groups: ConfigSchemaGroup[];
}

export interface MemoryEntry {
  id: string;
  kind: string;
  scope: string;
  sessionId?: string;
  agentId?: string;
  title: string;
  body: string;
  tags: string[];
  importance: number;
  confidence: number;
  pinned: boolean;
  sourceRef?: string;
  createdAt: string;
  updatedAt: string;
  accessCount: number;
}

export interface PersonaInfo {
  identifier: string;
  persona: string;
  custom: boolean;
  default: string;
}

export interface RecallHit {
  entry: MemoryEntry;
  score: number;
  reasons: string[];
}

export interface AgentStat {
  agentId: string;
  runs: number;
  done: number;
  blocked: number;
  failed: number;
  avgConfidence: number;
  updatedAt: string;
}

export interface GroupMeta {
  id: string;
  name: string;
  description: string;
  primary?: string;
  workspace?: string;
  /** 组默认模型（"档案ID" 或 "档案ID/模型名"；空 = 跟随全局生效档案） */
  model?: string;
  builtin: boolean;
  createdAt: string;
}

export interface SkillDef {
  id: string;
  name: string;
  description: string;
  triggers: string[];
  instructions: string;
  agents: string[];
  createdAt: string;
}

export interface CronJob {
  id: string;
  name: string;
  prompt: string;
  cron?: string;
  at?: string;
  group?: string;
  sessionTitle?: string;
  enabled: boolean;
  lastRunAt?: string;
  lastStatus?: string;
  createdAt: string;
}

export interface CronRun {
  id: string;
  jobId: string;
  jobName: string;
  sessionId: string;
  startedAt: string;
  finishedAt: string;
  status: string;
  summary: string;
}

export interface ApprovalRequest {
  id: string;
  sessionId: string;
  nodeId?: string;
  agentId: string;
  command: string;
  status: string;
  result?: string;
  createdAt: string;
  decidedAt?: string;
}

export interface Channel {
  id: string;
  type: string;
  enabled: boolean;
  group?: string;
  account?: string;
  secret?: string;
  replyWebhook?: string;
  token?: string;
  createdAt: string;
}

export interface LlmProfile {
  id: string;
  name: string;
  baseUrl: string;
  apiFormat?: string;
  apiKey: string;
  /** 多 Key 池（掩码行 = 沿用旧池同位键） */
  apiKeys?: string[];
  orchModel: string;
  unitModel: string;
  /** 失败回退：下一个档案 id（请求失败且未发出内容时切换） */
  fallback?: string | null;
  /** 嵌入模型（混合记忆检索；空 = 不提供嵌入） */
  embedModel?: string | null;
}

export interface LlmProfilesInfo {
  active: string;
  mock: boolean;
  profiles: LlmProfile[];
}

export interface GroupOverview {
  group: GroupMeta | null;
  agents: number;
  playbooks: number;
  sessions: number;
  memory: { group: number; shared: number };
  agentStats: AgentStat[];
}

export interface StoredEvent {
  id?: string;
  topic?: string;
  from?: string;
  to?: string;
  type?: string;
  kind?: string;
  sessionId?: string;
  payload?: unknown;
  createdAt?: string;
}

export interface MemoryStats {
  memory: {
    total: number;
    pinned: number;
    individual: number;
    shared: number;
    terms: number;
    byKind: Record<string, number>;
    dbPath: string;
  };
  agentStats: AgentStat[];
}

export const api = {
  verifyAuth: (key: string) =>
    req<{ ok: boolean; required?: boolean; error?: string }>("/auth/verify", {
      method: "POST",
      body: JSON.stringify({ key }),
    }),
  health: () => req<{ ok: boolean; mock: boolean; agents: number }>("/health"),
  listSessions: () => req<Session[]>("/sessions"),
  createSession: (title: string) =>
    req<Session>("/sessions", { method: "POST", body: JSON.stringify({ title }) }),
  getSession: (id: string) => req<Session>(`/sessions/${id}`),
  deleteSession: (id: string) => req<{ ok: boolean }>(`/sessions/${id}`, { method: "DELETE" }),
  messages: (id: string) => req<ChatMessage[]>(`/sessions/${id}/messages`),
  chat: (id: string, text: string) =>
    req<{ accepted: boolean }>(`/sessions/${id}/chat`, {
      method: "POST",
      body: JSON.stringify({ text }),
    }),
  agents: () => req<AgentDefinition[]>("/agents"),
  groups: () => req<{ active: string; groups: GroupMeta[] }>("/groups"),
  createGroup: (body: { name: string; id?: string; description?: string }) =>
    req<GroupMeta>("/groups", { method: "POST", body: JSON.stringify(body) }),
  switchGroup: (id: string) =>
    req<{ ok: boolean }>("/groups/active", { method: "PUT", body: JSON.stringify({ id }) }),
  deleteGroup: (id: string) => req<{ ok: boolean }>(`/groups/${id}`, { method: "DELETE" }),
  createAgent: (body: {
    name: string;
    identifier: string;
    description: string;
    domain?: string;
    tier?: string;
    prompt?: string;
    group?: string;
  }) => req<AgentDefinition>("/agents", { method: "POST", body: JSON.stringify(body) }),
  removeAgent: (identifier: string) =>
    req<{ ok: boolean }>(`/agents/${identifier}`, { method: "DELETE" }),
  getPersona: (identifier: string) => req<PersonaInfo>(`/agents/${identifier}/persona`),
  putPersona: (identifier: string, persona: string) =>
    req<{ ok: boolean }>(`/agents/${identifier}/persona`, {
      method: "PUT",
      body: JSON.stringify({ persona }),
    }),
  resetPersona: (identifier: string) =>
    req<{ ok: boolean }>(`/agents/${identifier}/persona`, { method: "DELETE" }),
  graph: (id: string) => req<TaskGraph>(`/sessions/${id}/graph`),
  evidence: (id: string) => req<EvidenceItem[]>(`/sessions/${id}/evidence`),

  getConfig: () => req<GatewayConfig>("/config"),
  putConfig: (body: Record<string, unknown>) =>
    req<{ ok: boolean; mock: boolean }>("/config", { method: "PUT", body: JSON.stringify(body) }),
  configSchema: () => req<ConfigSchema>("/config/schema"),

  listMemory: (opts?: { kind?: string; agent?: string; limit?: number }) => {
    const p = new URLSearchParams();
    if (opts?.kind) p.set("kind", opts.kind);
    if (opts?.agent) p.set("agent", opts.agent);
    p.set("limit", String(opts?.limit ?? 50));
    return req<MemoryEntry[]>(`/memory?${p.toString()}`);
  },
  searchMemory: (query: string, limit = 8, agent?: string) =>
    req<{ query: string; hits: RecallHit[] }>("/memory/search", {
      method: "POST",
      body: JSON.stringify({ query, limit, agent }),
    }),
  memoryStats: () => req<MemoryStats>("/memory/stats"),
  addMemory: (body: {
    kind?: string;
    title: string;
    body: string;
    tags?: string[];
    pin?: boolean;
    importance?: number;
    agentId?: string;
  }) => req<MemoryEntry>("/memory", { method: "POST", body: JSON.stringify(body) }),
  pinMemory: (id: string, pinned: boolean) =>
    req<{ ok: boolean }>(`/memory/${id}/pin`, { method: "POST", body: JSON.stringify({ pinned }) }),
  forgetMemory: (id: string) => req<{ ok: boolean }>(`/memory/${id}`, { method: "DELETE" }),
  reindexMemory: () => req<{ ok: boolean }>("/memory/reindex", { method: "POST" }),
  decayMemory: () => req<{ ok: boolean }>("/memory/decay", { method: "POST" }),
  renderMemory: () => req<{ ok: boolean }>("/memory/render", { method: "POST" }),

  groupAgents: (gid: string) => req<AgentDefinition[]>(`/groups/${gid}/agents`),
  getTarget: () => req<{ mode: "group" | "single"; id: string; name?: string; primary?: string }>("/target"),
  setTarget: (mode: "group" | "single", id?: string) =>
    req<{ ok: boolean }>("/target", { method: "PUT", body: JSON.stringify({ mode, id }) }),
  listSingles: () => req<{ active?: string; singles: { identifier: string; name: string; description: string; domain: string; modelHint?: string | null }[] }>("/singles"),
  setAgentModel: (identifier: string, model: string) =>
    req<{ ok: boolean }>(`/agents/${identifier}/model`, {
      method: "PUT",
      body: JSON.stringify({ model }),
    }),
  setGroupModel: (gid: string, model: string) =>
    req<{ ok: boolean }>(`/groups/${gid}/model`, {
      method: "PUT",
      body: JSON.stringify({ model }),
    }),
  createSingle: (body: { name: string; identifier: string; description: string; domain?: string; prompt?: string }) =>
    req<AgentDefinition>("/singles", { method: "POST", body: JSON.stringify(body) }),
  deleteSingle: (id: string) => req<{ ok: boolean }>(`/singles/${id}`, { method: "DELETE" }),
  groupOverview: (gid: string) => req<GroupOverview>(`/groups/${gid}/overview`),
  sessionEvents: (id: string, limit = 200) =>
    req<StoredEvent[]>(`/sessions/${id}/events?limit=${limit}`),

  skills: () => req<SkillDef[]>("/skills"),
  createSkill: (body: {
    id: string;
    name: string;
    description?: string;
    triggers?: string[];
    instructions: string;
    agents?: string[];
  }) => req<SkillDef>("/skills", { method: "POST", body: JSON.stringify(body) }),
  deleteSkill: (id: string) => req<{ ok: boolean }>(`/skills/${id}`, { method: "DELETE" }),

  listCron: () => req<CronJob[]>("/cron"),
  createCron: (body: {
    name: string;
    prompt: string;
    cron?: string;
    at?: string;
    group?: string;
    sessionTitle?: string;
  }) => req<CronJob>("/cron", { method: "POST", body: JSON.stringify(body) }),
  updateCron: (id: string, body: { enabled?: boolean }) =>
    req<CronJob>(`/cron/${id}`, { method: "PUT", body: JSON.stringify(body) }),
  deleteCron: (id: string) => req<{ ok: boolean }>(`/cron/${id}`, { method: "DELETE" }),
  runCron: (id: string) =>
    req<CronRun>(`/cron/${id}/run`, { method: "POST", body: JSON.stringify({}) }),
  cronRuns: (job?: string, limit = 20) => {
    const p = new URLSearchParams();
    if (job) p.set("job", job);
    p.set("limit", String(limit));
    return req<CronRun[]>(`/cron/runs?${p.toString()}`);
  },

  listApprovals: (status?: string, limit = 50) => {
    const p = new URLSearchParams();
    if (status) p.set("status", status);
    p.set("limit", String(limit));
    return req<ApprovalRequest[]>(`/approvals?${p.toString()}`);
  },
  decideApproval: (id: string, approve: boolean) =>
    req<ApprovalRequest>(`/approvals/${id}/${approve ? "approve" : "deny"}`, {
      method: "POST",
      body: JSON.stringify({}),
    }),

  listChannels: () => req<Channel[]>("/channels"),
  createChannel: (body: {
    id: string;
    platform?: string;
    group?: string;
    account?: string;
    secret?: string;
    replyWebhook?: string;
    token?: string;
    enabled?: boolean;
  }) => req<Channel>("/channels", { method: "POST", body: JSON.stringify(body) }),
  updateChannel: (id: string, body: { enabled?: boolean; group?: string; token?: string; secret?: string; replyWebhook?: string; account?: string }) =>
    req<Channel>(`/channels/${id}`, { method: "PUT", body: JSON.stringify(body) }),
  deleteChannel: (id: string) => req<{ ok: boolean }>(`/channels/${id}`, { method: "DELETE" }),

  llmProfiles: () => req<LlmProfilesInfo>("/llm/profiles"),
  saveLlmProfile: (body: { id?: string; name?: string; baseUrl?: string; apiFormat?: string; apiKey?: string; apiKeys?: string[]; orchModel?: string; unitModel?: string; fallback?: string; embedModel?: string }) =>
    req<{ ok: boolean; id: string; mock?: boolean }>("/llm/profiles", { method: "POST", body: JSON.stringify(body) }),
  deleteLlmProfile: (id: string) => req<{ ok: boolean }>(`/llm/profiles/${id}`, { method: "DELETE" }),
  activateLlmProfile: (id: string) =>
    req<{ ok: boolean; active: string; mock: boolean }>("/llm/active", { method: "PUT", body: JSON.stringify({ id }) }),
  testLlmProfile: (id?: string) =>
    req<{ ok: boolean; mock?: boolean; status?: number; snippet?: string; message?: string; error?: string }>("/llm/test", {
      method: "POST",
      body: JSON.stringify(id ? { id } : {}),
    }),
  setGroupWorkspace: (gid: string, workspace: string) =>
    req<{ ok: boolean }>(`/groups/${gid}/workspace`, { method: "PUT", body: JSON.stringify({ workspace }) }),
};
