/** 全局状态（Zustand）+ WS 事件处理 */
import { create } from "zustand";
import { message } from "antd";
import type {
  AgentDefinition,
  ApprovalItem,
  ChatMessage,
  Session,
  SessionLedger,
  Statement,
  TaskGraph,
  ToolCallItem,
} from "./types";
import { api, serverBase, type GatewayConfig, type GroupMeta, type RecallHit } from "./api";
import { tr } from "./i18n/core";

export interface TimelineItem {
  kind: "dispatch" | "sync" | "arbitration" | "memory" | "error" | "cron";
  agentId?: string;
  nodeId?: string;
  text: string;
}

interface WsEvent {
  type: string;
  sessionId: string;
  payload: unknown;
}

interface ExmState {
  sessions: Session[];
  sessionId: string | null;
  messages: ChatMessage[];
  graph: TaskGraph | null;
  agents: AgentDefinition[];
  ledger: SessionLedger | null;
  config: GatewayConfig | null;
  running: boolean;
  wsConnected: boolean;
  liveOrch: string;
  /** 指挥体思维链（分轨流式，不与回答混流） */
  liveThinking: string;
  liveUnits: Record<string, string>;
  /** 子个体思维链 */
  liveUnitThinking: Record<string, string>;
  /** 当前轮工具执行轨迹（编程软件式过程透明；run.finished 后并入最终消息） */
  runToolCalls: ToolCallItem[];
  /** 待人工审批单（聊天内联裁决） */
  approvals: ApprovalItem[];
  timeline: TimelineItem[];
  lastRecall: RecallHit[];
  memoryVersion: number;
  groups: GroupMeta[];
  activeGroup: string;

  init: () => Promise<void>;
  loadGroups: () => Promise<void>;
  switchGroup: (id: string) => Promise<void>;
  setTarget: (mode: "group" | "single", id?: string) => Promise<void>;
  createGroup: (body: { name: string; id?: string; description?: string }) => Promise<void>;
  refreshAgents: () => Promise<void>;
  selectSession: (id: string) => Promise<void>;
  newSession: () => Promise<void>;
  send: (text: string, images?: string[], mode?: string | null) => Promise<void>;
  /** 会话列表已完成首次加载（骨架屏判定） */
  sessionsLoaded: boolean;
  /** 会话输入草稿（内存态；sessionStorage 镜像持久化） */
  drafts: Record<string, string>;
  setDraft: (sessionId: string, text: string) => void;
  /** 新消息落会话列表时刷新该会话的末条预览（chat-ergonomics） */
  updateSessionPreview: (sessionId: string, preview: string) => void;
  saveConfig: (body: Record<string, unknown>) => Promise<void>;
  decideApproval: (id: string, approve: boolean) => Promise<void>;
  handleEvent: (evt: WsEvent) => void;
  setWs: (ok: boolean) => void;
  bumpMemory: () => void;
  /** 跨视图导航请求（审批→会话跳转等）：App 订阅消费 */
  navRequest: { view: string; sessionId?: string; nonce: number } | null;
  requestNav: (view: string, sessionId?: string) => void;
  markSessionsLoaded: () => void;
}

/** 会话输入草稿（chat-ergonomics）：内存 + sessionStorage 双写；发送成功即清除 */
const draftKey = (id: string) => `exm.draft.${id}`;

/** 浏览器通知偏好（web-notifications）：localStorage 持久化 */
export interface NotifyPrefs {
  enabled: boolean;
  approval: boolean;
  roundDone: boolean;
  cronDone: boolean;
}

export function loadNotifyPrefs(): NotifyPrefs {
  const def: NotifyPrefs = { enabled: false, approval: true, roundDone: false, cronDone: true };
  try {
    const raw = localStorage.getItem("exm.notify");
    return raw ? { ...def, ...JSON.parse(raw) } : def;
  } catch {
    return def;
  }
}

export function saveNotifyPrefs(prefs: NotifyPrefs): void {
  try { localStorage.setItem("exm.notify", JSON.stringify(prefs)); } catch { /* 忽略 */ }
}

/** 触发系统通知（授权 + 偏好由调用方判断），点击聚焦并跳转会话 */
function fireNotification(title: string, body: string, sessionId?: string): void {
  try {
    const n = new Notification(title, { body: body.slice(0, 200), tag: sessionId ?? title });
    n.onclick = () => {
      window.focus();
      if (sessionId) useExm.getState().requestNav("chat", sessionId);
    };
  } catch { /* 通知不可用时静默 */ }
}

/** 是否允许对某会话弹通知：已授权 + 页面隐藏 或 事件非当前会话 */
function fireNotificationLocal(title: string, body: string, sessionId?: string): void {
  try {
    const n = new Notification(title, { body: body.slice(0, 200), tag: sessionId ?? title });
    n.onclick = () => {
      window.focus();
      if (sessionId) useExm.getState().requestNav("chat", sessionId);
    };
  } catch { /* 忽略 */ }
}

function shouldNotifySession(sessionId?: string): boolean {
  if (document.visibilityState !== "hidden" && sessionId && sessionId === useExm.getState().sessionId) return false;
  return true;
}

let ws: WebSocket | null = null;
/** 连接代际号：每次 connectWs 递增。旧代际残留的 onclose/定时器一律失效，根治 A→B→A 快速切换的闭包竞态 */
let wsGen = 0;
/** 连续失败退避（2s → 5s → 10s → 30s 封顶）：成功 open 即归零 */
let wsBackoff = 0;
/** 心跳看门狗：超过该时长未收到任何帧（含协议 Pong）即判死重连 */
const WS_STALE_MS = 45000;
let wsLastFrame = 0;
let wsWatch: ReturnType<typeof setInterval> | null = null;

function connectWs(get: () => ExmState): void {
  const sessionId = get().sessionId;
  if (!sessionId) return;
  const gen = ++wsGen;
  // 主动关闭旧连接：其 onclose 携带旧 gen，不会触发重连
  ws?.close();
  const base = serverBase();
  const proto = base.startsWith("https") || (base === "" && location.protocol === "https:") ? "wss" : "ws";
  const host = base ? base.replace(/^https?:\/\//, "") : location.host;
  ws = new WebSocket(`${proto}://${host}/ws?sessionId=${sessionId}&key=${encodeURIComponent(localStorage.getItem("exm.key") ?? "")}`);
  let reconnected = false; // 本代际是否经历过断连重连（成功后补偿拉取丢失的事件）
  wsLastFrame = Date.now();
  ws.onopen = () => {
    if (gen !== wsGen) return;
    wsBackoff = 0; // 连上即归零退避
    get().setWs(true);
    if (!reconnected) return;
    // 断连补偿：重拉消息与任务图恢复丢失事件；图无在途节点则解除 running 卡死
    void (async () => {
      try {
        const [messages, graph] = await Promise.all([api.messages(sessionId), api.graph(sessionId)]);
        if (gen !== wsGen || useExm.getState().sessionId !== sessionId) return;
        const pending = (graph?.nodes ?? []).some((n) =>
          ["running", "dispatched", "syncing"].includes(String((n as { status?: string }).status ?? "")),
        );
        useExm.setState({ messages, graph: (graph?.nodes?.length ?? 0) > 0 ? graph : null, running: pending });
      } catch {
        /* 补偿失败静默：下轮断连重连时再次补偿 */
      }
    })();
  };
  ws.onclose = () => {
    if (gen !== wsGen) return; // 旧代际：静默退出，不碰当前连接、不重连
    get().setWs(false);
    reconnected = true;
    const delay = [2000, 5000, 10000, 30000][Math.min(wsBackoff, 3)];
    wsBackoff += 1;
    setTimeout(() => {
      if (gen !== wsGen || get().sessionId !== sessionId) return;
      connectWs(get);
    }, delay);
  };
  ws.onmessage = (m) => {
    wsLastFrame = Date.now();
    try {
      get().handleEvent(JSON.parse(String(m.data)) as WsEvent);
    } catch {
      /* 忽略坏帧 */
    }
  };
  // 心跳看门狗（单例）：空闲超时强制重连（浏览器对协议 Pong 不暴露事件，以「收帧时刻」为准——
  // 服务端 25s 一跳且事件流通常活跃，45s 静默基本等于链路已死）
  if (wsWatch === null) {
    wsWatch = setInterval(() => {
      const cur = ws;
      if (!cur || cur.readyState !== WebSocket.OPEN) return;
      if (Date.now() - wsLastFrame > WS_STALE_MS) {
        cur.close(); // 触发 onclose → 指数退避重连
      }
    }, 10000);
  }
}

// 网络恢复 / 页面回前台：立即重连（不等退避计时器）
function reconnectNow(): void {
  const cur = ws;
  if (cur && cur.readyState === WebSocket.OPEN) return;
  wsBackoff = 0;
  cur?.close(); // 触发 onclose → 立即档退避
  // onclose 携带旧代际时不会自愈（例如从未连上过），此处直接补一跳
  setTimeout(() => {
    if (!ws || ws.readyState === WebSocket.CLOSED) wsBackoff = 0;
  }, 0);
}

if (typeof window !== "undefined") {
  window.addEventListener("online", reconnectNow);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") reconnectNow();
  });
}

export const useExm = create<ExmState>((set, get) => ({
  sessions: [],
  sessionsLoaded: false,
  sessionId: null,
  messages: [],
  graph: null,
  agents: [],
  ledger: null,
  config: null,
  running: false,
  wsConnected: false,
  liveOrch: "",
  liveThinking: "",
  liveUnits: {},
  liveUnitThinking: {},
  runToolCalls: [],
  approvals: [],
  timeline: [],
  lastRecall: [],
  memoryVersion: 0,
  groups: [],
  activeGroup: "default",

  init: async () => {
    const [sessions, agents, config] = await Promise.all([
      api.listSessions(),
      api.agents(),
      api.getConfig(),
    ]);
    set({ sessions, agents, config });
    get().markSessionsLoaded();
    await get().loadGroups();
    if (sessions.length > 0) {
      await get().selectSession(sessions[0].id);
    } else {
      await get().newSession();
    }
  },

  loadGroups: async () => {
    const { active, groups } = await api.groups();
    set({ activeGroup: active, groups });
  },

  setTarget: async (mode, id) => {
    await api.setTarget(mode, id);
    const agents = await api.agents();
    const sessions = await api.listSessions();
    set({
      agents,
      sessions,
      graph: null,
      timeline: [],
      liveOrch: "",
      liveThinking: "",
      liveUnits: {},
      liveUnitThinking: {},
      runToolCalls: [],
      approvals: [],
      messages: [],
    });
    if (sessions.length > 0) {
      await get().selectSession(sessions[0].id);
    } else {
      await get().newSession();
    }
  },

  switchGroup: async (id) => {
    await api.switchGroup(id);
    const agents = await api.agents();
    // 组是交互对象：切组即切换会话上下文（会话归属组），自动选中该组最近会话
    const sessions = await api.listSessions();
    set({
      activeGroup: id,
      agents,
      sessions,
      graph: null,
      timeline: [],
      liveOrch: "",
      liveThinking: "",
      liveUnits: {},
      liveUnitThinking: {},
      runToolCalls: [],
      approvals: [],
      messages: [],
    });
    await get().loadGroups();
    if (sessions.length > 0) {
      await get().selectSession(sessions[0].id);
    } else {
      await get().newSession();
    }
  },

  createGroup: async (body) => {
    await api.createGroup(body);
    await get().loadGroups();
  },

  refreshAgents: async () => {
    set({ agents: await api.agents() });
  },

  selectSession: async (id) => {
    const [messages, graph, session] = await Promise.all([
      api.messages(id),
      api.graph(id),
      api.getSession(id),
    ]);
    set({
      sessionId: id,
      messages,
      graph: graph.nodes?.length ? graph : null,
      ledger: session.ledger,
      liveOrch: "",
      liveThinking: "",
      liveUnits: {},
      liveUnitThinking: {},
      runToolCalls: [],
      approvals: [],
      running: false,
    });
    connectWs(get);
  },

  newSession: async () => {
    const s = await api.createSession(tr("session.new"));
    set({ sessions: [s, ...get().sessions] });
    await get().selectSession(s.id);
  },

  send: async (text, images, sendMode) => {
    const id = get().sessionId;
    if (!id || !text.trim()) return;
    const label = images?.length ? tr("store.withImages", { text, n: images.length }) : text;
    set({
      running: true,
      liveOrch: "",
      liveThinking: "",
      liveUnits: {},
      liveUnitThinking: {},
      runToolCalls: [],
      approvals: [],
      timeline: [],
      lastRecall: [],
      messages: [
        ...get().messages,
        {
          id: `local-${Date.now()}`,
          sessionId: id,
          role: "user",
          statements: [{ tag: "要求", text: label }],
          createdAt: new Date().toISOString(),
        },
      ],
    });
    try {
      await api.chat(id, text, images, sendMode ?? undefined);
    } finally {
      // 草稿已发出：清除（chat-ergonomics）
      try {
        sessionStorage.removeItem(draftKey(id));
      } catch { /* 隐私模式忽略 */ }
      set((s) => ({ drafts: { ...s.drafts, [id]: "" } }));
    }
    get().updateSessionPreview(id, label);
  },

  saveConfig: async (body) => {
    await api.putConfig(body);
    set({ config: await api.getConfig() });
  },

  setWs: (ok) => set({ wsConnected: ok }),
  bumpMemory: () => set({ memoryVersion: get().memoryVersion + 1 }),

  decideApproval: async (id, approve) => {
    try {
      await api.decideApproval(id, approve);
      // 乐观移除（approval.resolved 事件到达时幂等）
      set({ approvals: get().approvals.filter((a) => a.approvalId !== id) });
    } catch (e) {
      message.error(String(e));
    }
  },

  navRequest: null,
  requestNav: (view, sessionId) => set({ navRequest: { view, sessionId, nonce: Date.now() } }),
  markSessionsLoaded: () => set({ sessionsLoaded: true }),

  drafts: (() => {
    try { return JSON.parse(sessionStorage.getItem("exm.drafts") ?? "{}"); } catch { return {}; }
  })(),
  setDraft: (sessionId, text) => {
    set((s) => ({ drafts: { ...s.drafts, [sessionId]: text } }));
    try { sessionStorage.setItem("exm.drafts", JSON.stringify({ ...useExm.getState().drafts, [sessionId]: text })); } catch { /* 隐私模式忽略 */ }
  },
  updateSessionPreview: (sessionId, preview) => {
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === sessionId ? { ...x, lastMessagePreview: preview, lastActiveAt: new Date().toISOString() } : x)) }));
  },

  handleEvent: (evt) => {
    if (evt.sessionId !== get().sessionId) return;
    const p = evt.payload as Record<string, unknown>;
    switch (evt.type) {
      case "orchestrator.token":
        set({ liveOrch: get().liveOrch + String(p.delta ?? "") });
        break;
      case "orchestrator.thinking":
        set({ liveThinking: get().liveThinking + String(p.delta ?? "") });
        break;
      case "unit.thinking": {
        const agent = String(p.agentId ?? "");
        set({
          liveUnitThinking: {
            ...get().liveUnitThinking,
            [agent]: (get().liveUnitThinking[agent] ?? "") + String(p.delta ?? ""),
          },
        });
        break;
      }
      case "tool.call": {
        const call: ToolCallItem = {
          callId: String(p.callId),
          agentId: String(p.agentId ?? ""),
          tool: String(p.tool ?? ""),
          args: (p.args ?? {}) as Record<string, unknown>,
          status: "running",
        };
        set({ runToolCalls: [...get().runToolCalls.filter((c) => c.callId !== call.callId), call] });
        break;
      }
      case "tool.result": {
        const callId = String(p.callId);
        set({
          runToolCalls: get().runToolCalls.map((c) =>
            c.callId === callId
              ? {
                  ...c,
                  status: p.ok ? "ok" : "error",
                  durationMs: Number(p.durationMs ?? 0),
                  summary: String(p.summary ?? "").slice(0, 400),
                  images: (p.images ?? []) as string[],
                }
              : c,
          ),
        });
        break;
      }
      case "approval.required": {
        const prefs = loadNotifyPrefs();
        if (prefs.enabled && prefs.approval && document.visibilityState === "hidden" && typeof Notification !== "undefined" && Notification.permission === "granted") {
          fireNotification(
            "审批请求待处理",
            String(p.command ?? p.agentId ?? ""),
            evt.sessionId || undefined,
          );
        }
        set({
          approvals: [
            ...get().approvals.filter((a) => a.approvalId !== String(p.approvalId)),
            {
              approvalId: String(p.approvalId),
              agentId: String(p.agentId ?? ""),
              command: String(p.command ?? ""),
            },
          ],
        });
        break;
      }
      case "approval.resolved":
        set({ approvals: get().approvals.filter((a) => a.approvalId !== String(p.approvalId)) });
        break;
      case "unit.token": {
        const agent = String(p.agentId ?? "");
        set({
          liveUnits: {
            ...get().liveUnits,
            [agent]: (get().liveUnits[agent] ?? "") + String(p.delta ?? ""),
          },
        });
        break;
      }
      case "graph.updated":
        set({ graph: p as unknown as TaskGraph });
        break;
      case "dispatch.sent":
        set({
          timeline: [
            ...get().timeline,
            {
              kind: "dispatch",
              agentId: String(p.agentIdentifier),
              nodeId: String(p.nodeId),
              text: tr("store.tl.dispatch", { agent: String(p.agentIdentifier), node: String(p.nodeId) }),
            },
          ],
        });
        break;
      case "sync.received": {
        const report = p.report as {
          sourceAgent: string;
          taskNodeId: string;
          summary: string;
          confidence: number;
        };
        const units = { ...get().liveUnits };
        delete units[report.sourceAgent];
        set({
          liveUnits: units,
          timeline: [
            ...get().timeline,
            {
              kind: "sync",
              agentId: report.sourceAgent,
              nodeId: report.taskNodeId,
              text: tr("store.tl.sync", { agent: report.sourceAgent, summary: report.summary.slice(0, 80), conf: report.confidence }),
            },
          ],
        });
        break;
      }
      case "memory.recall": {
        const hits = (p.hits ?? []) as RecallHit[];
        set({
          lastRecall: hits,
          timeline: [...get().timeline, { kind: "memory", text: tr("store.tl.memory", { n: hits.length }) }],
        });
        break;
      }
      case "memory.written": {
        const entries = (p.entries ?? []) as unknown[];
        set({
          timeline: [
            ...get().timeline,
            {
              kind: "memory",
              text: tr("store.tl.memoryWrite", { n: entries.length, pinned: Number(p.pinned ?? 0) }),
            },
          ],
        });
        get().bumpMemory();
        break;
      }
      case "arbitration.required":
        set({
          timeline: [
            ...get().timeline,
            { kind: "arbitration", text: tr("store.tl.arbitration") },
          ],
        });
        break;
      case "ledger.updated":
        set({ ledger: p as unknown as SessionLedger });
        break;
      case "run.finished": {
        // 浏览器通知（web-notifications）：非当前会话或页面隐藏时提醒
        {
          const prefs = loadNotifyPrefs();
          if (prefs.enabled && prefs.roundDone && typeof Notification !== "undefined" && Notification.permission === "granted" && shouldNotifySession(evt.sessionId)) {
            const first = ((p.statements ?? []) as Array<{ text?: string }>)[0];
            fireNotificationLocal("轮次完成", String(first?.text ?? "已完成"), evt.sessionId);
          }
        }
        const statements = (p.statements ?? []) as Statement[];
        const id = get().sessionId!;
        // 工具轨迹与思维链随最终消息留存（收起栏可随时展开回看，不再「突然消失」）
        const toolCalls = get().runToolCalls;
        const thinking = get().liveThinking;
        set({
          running: false,
          liveOrch: "",
          liveThinking: "",
          liveUnits: {},
          liveUnitThinking: {},
          runToolCalls: [],
          messages: [
            ...get().messages,
            {
              id: `fin-${Date.now()}`,
              sessionId: id,
              role: "orchestrator",
              agentId: typeof p.agentId === "string" ? p.agentId : "orchestrator",
              statements,
              toolCalls: toolCalls.length > 0 ? toolCalls : undefined,
              thinking: thinking.trim() ? thinking : undefined,
              createdAt: new Date().toISOString(),
            },
          ],
        });
        const finText = statements.map((st) => st.text).join(" ");
        if (finText.trim()) get().updateSessionPreview(id, finText);
        break;
      }
      case "cron.finished": {
        const prefs = loadNotifyPrefs();
        if (prefs.enabled && prefs.cronDone && typeof Notification !== "undefined" && Notification.permission === "granted") {
          const name = String(p.jobName ?? "");
          const status = String(p.status ?? "");
          fireNotificationLocal(`定时任务${status === "done" ? "完成" : "失败"}：${name}`, String(p.summary ?? status), undefined);
        }
        set({ timeline: [...get().timeline, { kind: "cron", text: tr("store.tl.cron", { name: String(p.jobName ?? ""), status: String(p.status ?? "") }) }] });
        break;
      }
      case "run.error":
        set({
          running: false,
          timeline: [...get().timeline, { kind: "error", text: tr("store.tl.error", { msg: String(p.message) }) }],
        });
        break;
    }
  },
}));
