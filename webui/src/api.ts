/** REST 客户端 —— 契约见 docs/04 §7 */
import type { AgentDefinition, ChatMessage, EvidenceItem, Session, TaskGraph } from "./types";
import { tr } from "./i18n/core";

/** 密钥掩码哨兵（与后端 llm_admin::KEY_MASK / 通道脱敏同一契约）：提交时收到此值 = 沿用服务端旧值 */
export const KEY_MASK = "***已配置***";

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
    throw new Error(tr("api.unauthorized"));
  }
  if (!resp.ok && resp.status !== 202) {
    const body = await resp.text();
    throw new Error(`${init?.method ?? "GET"} ${path} -> ${resp.status} ${body.slice(0, 200)}`);
  }
  return (await resp.json()) as T;
}

export interface GatewayConfig {
  llm: { baseUrl: string; apiKey: string; model: string };
  maxConcurrency: number;
  /** 会话 token 预算（估算；0 = 不限） */
  maxSessionTokens: number;
  memory: {
    enabled: boolean;
    recallLimit: number;
    halfLifeDays: number;
    /** memory.md 字数上限（超限触发 AI 自主压缩） */
    mdMaxChars?: number;
    /** 语义检索目标："档案ID" 或 "档案ID/模型名"；空 = 仅词项召回 */
    semanticModel: string;
  };
  security: {
    execApproval: string;
    execAllowlist: string;
    /** 后台访问密钥（服务端掩码回显；空 = 免鉴权） */
    authKey?: string;
    /** 终端命令超时（秒）；后台任务不受此限 */
    terminalTimeoutSecs?: number;
    /** 工具结果落盘阈值（字符） */
    toolOutputSpillChars?: number;
  };
  automation: {
    heartbeatEnabled: boolean;
    heartbeatIntervalMinutes: number;
    heartbeatPrompt: string;
    autoAdapt: boolean;
    /** 子个体单次派发的最大工具步数 */
    unitMaxSteps?: number;
  };
  /** 联网搜索后端（web_search 工具；未配置则不下发该工具） */
  search?: { provider: string; endpoint: string; apiKey: string; maxResults: number };
  /** 沙箱执行（环境净化 / bwrap / strict 闸门） */
  sandbox?: { mode: string; allowNetwork: boolean; useBwrap: boolean; memoryMb: number; maxProcesses: number };
  /** 浏览器自动化（browser 工具） */
  browser?: { executable: string; headless: boolean; timeoutSecs: number; maxChars: number };
  /** 生命周期钩子 */
  hooks?: { preTool: string[]; postTool: string[]; onRunEnd: string[] };
  /** 工具面（声明式自定义工具） */
  tools?: { custom: Array<Record<string, unknown>> };
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

/** 组级能力模型覆盖（"档案ID" 或 "档案ID/模型名"）：每组可用不同模型栈；缺省 = 跟随全局槽位 */
export interface GroupCapabilities {
  speech?: string;
  transcribe?: string;
  visionRelay?: string;
  embedding?: string;
}

export interface GroupMeta {
  id: string;
  name: string;
  description: string;
  primary?: string;
  workspace?: string;
  /** 组默认模型（"档案ID" 或 "档案ID/模型名"；空 = 跟随全局生效档案） */
  model?: string;
  /** 组级能力模型覆盖；缺省字段跟随「模型设置」页的全局槽位 */
  capabilities?: GroupCapabilities | null;
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
  /** 平台扩展配置：qqbot → appId/appSecret/sandbox；napcat → url/token */
  config?: Record<string, string>;
  /** 会话白名单（Telegram chat id / QQ openid / OneBot 群号或 QQ 号；空 = 不限） */
  allowedChats?: string[];
  createdAt: string;
}

/** 通道运行状态（内置适配器上报；桥接类型无运行时不产生条目） */
export interface ChannelStatus {
  state: "ok" | "error";
  detail: string;
  at: string;
}

export interface LlmProfile {
  id: string;
  name: string;
  baseUrl: string;
  apiFormat?: string;
  apiKey: string;
  /** 多 Key 池（掩码行 = 沿用旧池同位键） */
  apiKeys?: string[];
  /** 该提供商的默认模型名（模型清单里设为默认的那一个） */
  model: string;
  /** 模型清单（模型名 + 视觉/语音能力开关）；空 = 未标记（能力未知，输入直通） */
  models: ProfileModel[];
  /** 失败回退：下一个档案 id（请求失败且未发出内容时切换） */
  fallback?: string | null;
}

/** 模型条目：视觉/语音开关决定多模态路由（不支持时走视觉转述/语音转述） */
export interface ProfileModel {
  model: string;
  vision: boolean;
  audio: boolean;
}

/** 能力模型槽位："档案ID" 或 "档案ID/模型名"；空 = 全局档案默认（TTS tts-1 / STT whisper-1 / 仅词项召回） */
export interface LlmCapabilities {
  speech: string;
  transcribe: string;
  visionRelay: string;
  embedding: string;
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
  transcribe: async (blob: Blob) => {
    const form = new FormData();
    form.append("audio", blob, "audio.webm");
    const key = localStorage.getItem("exm.key") ?? "";
    const resp = await fetch("/api/voice/transcribe", {
      method: "POST",
      headers: key ? { "X-Auth-Key": key } : {},
      body: form,
    });
    const data = (await resp.json()) as { text?: string; error?: string };
    if (!resp.ok || data.error) throw new Error(data.error ?? tr("api.transcribeFailed", { status: resp.status }));
    return data.text ?? "";
  },
  speak: async (text: string) => {
    const key = localStorage.getItem("exm.key") ?? "";
    const resp = await fetch("/api/voice/speak", {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(key ? { "X-Auth-Key": key } : {}) },
      body: JSON.stringify({ text }),
    });
    if (!resp.ok) {
      const err = (await resp.json().catch(() => ({}))) as { error?: string };
      throw new Error(err.error ?? tr("api.synthFailed", { status: resp.status }));
    }
    return resp.blob();
  },
  sessionTokens: (id: string) =>
    req<{ estimate: number; budget: number; unlimited: boolean }>(`/sessions/${id}/tokens`),
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
  renameSession: (id: string, title: string) =>
    req<{ ok: boolean; title: string }>(`/sessions/${id}/title`, {
      method: "PUT",
      body: JSON.stringify({ title }),
    }),
  deleteSession: (id: string) => req<{ ok: boolean }>(`/sessions/${id}`, { method: "DELETE" }),
  messages: (id: string) => req<ChatMessage[]>(`/sessions/${id}/messages`),
  chat: (id: string, text: string, images?: string[]) =>
    req<{ accepted: boolean }>(`/sessions/${id}/chat`, {
      method: "POST",
      body: JSON.stringify(images?.length ? { text, images } : { text }),
    }),
  agents: () => req<AgentDefinition[]>("/agents"),
  groups: () => req<{ active: string; groups: GroupMeta[] }>("/groups"),
  createGroup: (body: { name: string; id?: string; description?: string }) =>
    req<GroupMeta>("/groups", { method: "POST", body: JSON.stringify(body) }),
  switchGroup: (id: string) =>
    req<{ ok: boolean }>("/groups/active", { method: "PUT", body: JSON.stringify({ id }) }),
  /** 设置组主智能体 */
  setGroupPrimary: (gid: string, identifier: string) =>
    req<{ ok: boolean }>(`/groups/${gid}/primary`, { method: "POST", body: JSON.stringify({ identifier }) }),
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
  removeGroupAgent: (gid: string, identifier: string) =>
    req<{ ok: boolean }>(`/groups/${gid}/agents/${identifier}`, { method: "DELETE" }),
  getPersona: (identifier: string) => req<PersonaInfo>(`/agents/${identifier}/persona`),
  putPersona: (identifier: string, persona: string) =>
    req<{ ok: boolean }>(`/agents/${identifier}/persona`, {
      method: "PUT",
      body: JSON.stringify({ persona }),
    }),
  resetPersona: (identifier: string) =>
    req<{ ok: boolean }>(`/agents/${identifier}/persona`, { method: "DELETE" }),
  /** 单体智能体人设（agents/singles/personas/<id>.md，与组内个体同语义） */
  singlePersona: (id: string) => req<PersonaInfo>(`/singles/${id}/persona`),
  singlesSetPersona: (id: string, persona: string) =>
    req<{ ok: boolean }>(`/singles/${id}/persona`, { method: "PUT", body: JSON.stringify({ persona }) }),
  singlesResetPersona: (id: string) =>
    req<{ ok: boolean }>(`/singles/${id}/persona`, { method: "DELETE" }),
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
  /** 更新个体可编辑字段（name/domain/description/capabilities/tools/modelHint；identifier/tier/promptFile 不可改） */
  updateAgent: (
    identifier: string,
    body: { name?: string; domain?: string; description?: string; capabilities?: string[]; tools?: string[]; modelHint?: string },
  ) => req<AgentDefinition>(`/agents/${identifier}`, { method: "PUT", body: JSON.stringify(body) }),
  /** 更新单体智能体可编辑字段（同 updateAgent 字段集） */
  updateSingle: (
    id: string,
    body: { name?: string; domain?: string; description?: string; capabilities?: string[]; tools?: string[]; modelHint?: string },
  ) => req<AgentDefinition>(`/singles/${id}`, { method: "PUT", body: JSON.stringify(body) }),
  setGroupModel: (gid: string, model: string) =>
    req<{ ok: boolean }>(`/groups/${gid}/model`, {
      method: "PUT",
      body: JSON.stringify({ model }),
    }),
  setGroupCapabilities: (gid: string, body: GroupCapabilities) =>
    req<{ ok: boolean; capabilities?: GroupCapabilities | null }>(`/groups/${gid}/capabilities`, {
      method: "PUT",
      body: JSON.stringify(body),
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
  channelStatus: () => req<Record<string, ChannelStatus>>("/channels/status"),
  createChannel: (body: {
    id: string;
    platform?: string;
    group?: string;
    account?: string;
    secret?: string;
    replyWebhook?: string;
    token?: string;
    enabled?: boolean;
    allowedChats?: string[];
    config?: Record<string, string>;
  }) => req<Channel>("/channels", { method: "POST", body: JSON.stringify(body) }),
  updateChannel: (id: string, body: { enabled?: boolean; group?: string; token?: string; secret?: string; replyWebhook?: string; account?: string; allowedChats?: string[]; config?: Record<string, string> }) =>
    req<Channel>(`/channels/${id}`, { method: "PUT", body: JSON.stringify(body) }),
  deleteChannel: (id: string) => req<{ ok: boolean }>(`/channels/${id}`, { method: "DELETE" }),

  llmProfiles: () => req<LlmProfilesInfo>("/llm/profiles"),
  saveLlmProfile: (body: { id?: string; name?: string; baseUrl?: string; apiFormat?: string; apiKey?: string; apiKeys?: (string | { keep: number })[]; model?: string; models?: ProfileModel[]; fallback?: string }) =>
    req<{ ok: boolean; id: string; mock?: boolean }>("/llm/profiles", { method: "POST", body: JSON.stringify(body) }),
  deleteLlmProfile: (id: string) => req<{ ok: boolean }>(`/llm/profiles/${id}`, { method: "DELETE" }),
  activateLlmProfile: (id: string) =>
    req<{ ok: boolean; active: string; mock: boolean }>("/llm/active", { method: "PUT", body: JSON.stringify({ id }) }),
  testLlmProfile: (id?: string) =>
    req<{ ok: boolean; mock?: boolean; configured?: boolean; status?: number; snippet?: string; message?: string; error?: string }>("/llm/test", {
      method: "POST",
      body: JSON.stringify(id ? { id } : {}),
    }),
  llmCapabilities: () => req<LlmCapabilities>("/llm/capabilities"),
  saveLlmCapabilities: (body: Partial<LlmCapabilities>) =>
    req<LlmCapabilities & { ok: boolean }>("/llm/capabilities", { method: "PUT", body: JSON.stringify(body) }),
  setGroupWorkspace: (gid: string, workspace: string) =>
    req<{ ok: boolean }>(`/groups/${gid}/workspace`, { method: "PUT", body: JSON.stringify({ workspace }) }),
};
