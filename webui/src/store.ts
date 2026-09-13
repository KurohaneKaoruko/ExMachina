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
  send: (text: string) => Promise<void>;
  saveConfig: (body: Record<string, unknown>) => Promise<void>;
  handleEvent: (evt: WsEvent) => void;
  setWs: (ok: boolean) => void;
  bumpMemory: () => void;
}

let ws: WebSocket | null = null;

function connectWs(get: () => ExmState): void {
  const sessionId = get().sessionId;
  if (!sessionId) return;
  ws?.close();
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws?sessionId=${sessionId}&key=${encodeURIComponent(localStorage.getItem("exm.key") ?? "")}`);
  ws.onopen = () => get().setWs(true);
  ws.onclose = () => {
    get().setWs(false);
    setTimeout(() => {
      if (get().sessionId === sessionId) connectWs(get);
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
    const s = await api.createSession("新会话");
    set({ sessions: [s, ...get().sessions] });
    await get().selectSession(s.id);
  },

  send: async (text) => {
    const id = get().sessionId;
    if (!id || !text.trim()) return;
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
          statements: [{ tag: "要求", text }],
          createdAt: new Date().toISOString(),
        },
      ],
    });
    await api.chat(id, text);
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
              text: `指挥体派发 → ${p.agentIdentifier}（${p.nodeId}）`,
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
              text: `${report.sourceAgent} 回流：${report.summary.slice(0, 80)}（置信度 ${report.confidence}）`,
            },
          ],
        });
        break;
      }
      case "memory.recall": {
        const hits = (p.hits ?? []) as RecallHit[];
        set({
          lastRecall: hits,
          timeline: [...get().timeline, { kind: "memory", text: `记忆召回 ${hits.length} 条` }],
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
              text: `记忆写入 ${entries.length} 条（固定记忆 ${p.pinned ?? 0} 条，基础记忆已更新）`,
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
            { kind: "arbitration", text: "冲突待裁决，已追加裁决节点" },
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
          timeline: [...get().timeline, { kind: "error", text: `运行失败：${String(p.message)}` }],
        });
        break;
    }
  },
}));
