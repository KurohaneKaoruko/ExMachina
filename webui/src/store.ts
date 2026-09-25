/** 全局状态（Zustand）+ WS 事件处理 */
import { create } from "zustand";
import type {
  AgentDefinition,
  ChatMessage,
  Session,
  SessionLedger,
  Statement,
  TaskGraph,
} from "./types";
import { api, type GatewayConfig, type GroupMeta, type RecallHit } from "./api";
import { tr } from "./i18n/core";

export interface TimelineItem {
  kind: "dispatch" | "sync" | "arbitration" | "memory" | "error";
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
  liveUnits: Record<string, string>;
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
  send: (text: string, images?: string[]) => Promise<void>;
  saveConfig: (body: Record<string, unknown>) => Promise<void>;
  handleEvent: (evt: WsEvent) => void;
  setWs: (ok: boolean) => void;
  bumpMemory: () => void;
}

let ws: WebSocket | null = null;
/** 连接代际号：每次 connectWs 递增。旧代际残留的 onclose/定时器一律失效，根治 A→B→A 快速切换的闭包竞态 */
let wsGen = 0;

function connectWs(get: () => ExmState): void {
  const sessionId = get().sessionId;
  if (!sessionId) return;
  const gen = ++wsGen;
  // 主动关闭旧连接：其 onclose 携带旧 gen，不会触发重连
  ws?.close();
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws?sessionId=${sessionId}&key=${encodeURIComponent(localStorage.getItem("exm.key") ?? "")}`);
  let reconnected = false; // 本代际是否经历过断连重连（成功后补偿拉取丢失的事件）
  ws.onopen = () => {
    if (gen !== wsGen) return;
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
    setTimeout(() => {
      if (gen !== wsGen || get().sessionId !== sessionId) return;
      connectWs(get);
    }, 2000);
  };
  ws.onmessage = (m) => {
    try {
      get().handleEvent(JSON.parse(String(m.data)) as WsEvent);
    } catch {
      /* 忽略坏帧 */
    }
  };
}

export const useExm = create<ExmState>((set, get) => ({
  sessions: [],
  sessionId: null,
  messages: [],
  graph: null,
  agents: [],
  ledger: null,
  config: null,
  running: false,
  wsConnected: false,
  liveOrch: "",
  liveUnits: {},
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
      liveUnits: {},
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
      liveUnits: {},
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
      liveUnits: {},
      timeline: [],
      running: false,
    });
    connectWs(get);
  },

  newSession: async () => {
    const s = await api.createSession(tr("session.new"));
    set({ sessions: [s, ...get().sessions] });
    await get().selectSession(s.id);
  },

  send: async (text, images) => {
    const id = get().sessionId;
    if (!id || !text.trim()) return;
    const label = images?.length ? tr("store.withImages", { text, n: images.length }) : text;
    set({
      running: true,
      liveOrch: "",
      liveUnits: {},
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
    await api.chat(id, text, images);
  },

  saveConfig: async (body) => {
    await api.putConfig(body);
    set({ config: await api.getConfig() });
  },

  setWs: (ok) => set({ wsConnected: ok }),
  bumpMemory: () => set({ memoryVersion: get().memoryVersion + 1 }),

  handleEvent: (evt) => {
    if (evt.sessionId !== get().sessionId) return;
    const p = evt.payload as Record<string, unknown>;
    switch (evt.type) {
      case "orchestrator.token":
        set({ liveOrch: get().liveOrch + String(p.delta ?? "") });
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
        const statements = (p.statements ?? []) as Statement[];
        const id = get().sessionId!;
        set({
          running: false,
          liveOrch: "",
          liveUnits: {},
          messages: [
            ...get().messages,
            {
              id: `fin-${Date.now()}`,
              sessionId: id,
              role: "orchestrator",
              agentId: typeof p.agentId === "string" ? p.agentId : "orchestrator",
              statements,
              createdAt: new Date().toISOString(),
            },
          ],
        });
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
