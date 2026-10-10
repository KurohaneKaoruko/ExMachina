/** EXMACHINA 桌面对话界面 —— 对话优先的桌面壳 UI（无构建，经 frontendDist 嵌入）
 *
 * 与网关的契约（与 WebUI 同一口径）：
 * - REST：`${gw}/api/...`，鉴权头 `X-Auth-Key`（密钥由壳注入）
 * - WS：`${gw}/ws?sessionId=<id>&key=<key>`，事件 {type, sessionId, payload}
 * - 发言：POST /api/sessions/:id/chat {text} → 事件流 → run.finished 收束
 *
 * 网关地址来源：壳经 initialization_script 注入 `window.__EXM_GATEWAY__`；
 * 浏览器直开调试时可用 `?gw=http://127.0.0.1:<port>` 覆盖或同源兜底。
 */
"use strict";

// ────────────────────────────── 网关寻址 ──────────────────────────────

const GW = (() => {
  const injected = window.__EXM_GATEWAY__ ?? {};
  const qp = new URLSearchParams(location.search).get("gw");
  const url = (qp || injected.url || (location.protocol.startsWith("http") ? location.origin : "http://127.0.0.1:58501")).replace(/\/+$/, "");
  return { url, key: injected.key ?? "" };
})();

async function req(path, init) {
  const headers = { "Content-Type": "application/json" };
  if (GW.key) headers["X-Auth-Key"] = GW.key;
  const resp = await fetch(`${GW.url}/api${path}`, { headers, ...init });
  if (!resp.ok && resp.status !== 202) {
    const body = await resp.text().catch(() => "");
    let msg = body.slice(0, 200);
    try { msg = JSON.parse(body).error ?? msg; } catch { /* 非 JSON 原样截断 */ }
    throw new Error(`${init?.method ?? "GET"} ${path} → ${resp.status} ${msg}`);
  }
  return resp.json();
}

const api = {
  health: () => req("/health"),
  config: () => req("/config"),
  target: () => req("/target"),
  sessions: () => req("/sessions"),
  createSession: (title) => req("/sessions", { method: "POST", body: JSON.stringify({ title }) }),
  deleteSession: (id) => req(`/sessions/${id}`, { method: "DELETE" }),
  messages: (id) => req(`/sessions/${id}/messages`),
  chat: (id, text, images, agent) => req(`/sessions/${id}/chat`, { method: "POST", body: JSON.stringify({ text, ...(images?.length ? { images } : {}), ...(agent ? { agent } : {}) }) }),
  stop: (id) => req(`/sessions/${id}/stop`, { method: "POST", body: JSON.stringify({}) }),
  graph: (id) => req(`/sessions/${id}/graph`),
  decideApproval: (id, approve) => req(`/approvals/${id}/${approve ? "approve" : "deny"}`, { method: "POST", body: JSON.stringify({}) }),
  // 输入区上下文芯片：组 / 智能体 / 模型 / 工作目录
  groups: () => req("/groups"),
  singles: () => req("/singles"),
  llmProfiles: () => req("/llm/profiles"),
  setTarget: (mode, id) => req("/target", { method: "PUT", body: JSON.stringify({ mode, ...(id ? { id } : {}) }) }),
  setGroupWorkspace: (gid, workspace) => req(`/groups/${gid}/workspace`, { method: "PUT", body: JSON.stringify({ workspace }) }),
  setGroupModel: (gid, model) => req(`/groups/${gid}/model`, { method: "PUT", body: JSON.stringify({ model }) }),
  setSingleModel: (id, modelHint) => req(`/singles/${id}`, { method: "PUT", body: JSON.stringify({ modelHint }) }),
  // 模型思考强度（reasoning effort）：进程级，对指挥体与子个体的全部 LLM 调用生效
  getEffort: () => req("/llm/effort"),
  setEffort: (effort) => req("/llm/effort", { method: "PUT", body: JSON.stringify({ effort }) }),
  // 工作目录（干活的项目）：智能体与智能体组通用
  getWorkspace: () => req("/workspace"),
  setWorkspace: (path) => req("/workspace", { method: "PUT", body: JSON.stringify({ path }) }),
  // 会话事件流（子个体详情：派发单/回执等存储事件）
  sessionEvents: (id, limit = 300) => req(`/sessions/${id}/events?limit=${limit}`),
  // 会话增强：详情（含台账 ledger）/ 重命名 / 分叉 / 撤销 / 归档 / 轮次快照
  sessionDetail: (id) => req(`/sessions/${id}`),
  renameSession: (id, title) => req(`/sessions/${id}/title`, { method: "PUT", body: JSON.stringify({ title }) }),
  forkSession: (id, turn) => req(`/sessions/${id}/fork`, { method: "POST", body: JSON.stringify({ turn }) }),
  undoSession: (id, turn, restoreFiles) => req(`/sessions/${id}/undo`, { method: "POST", body: JSON.stringify({ turn, restoreFiles }) }),
  archive: (id) => req(`/sessions/${id}/archive`),
  snapshots: (id) => req(`/sessions/${id}/snapshots`),
  // 审批清单（status: pending|approved|denied|executed|failed；空串 = 全部，此时不带参数以免后端按空串过滤）
  approvals: (status, limit = 50) => req(`/approvals?${status ? `status=${encodeURIComponent(status)}&` : ""}limit=${limit}`),
  // 编码页：工作区文件树 / 文本读写 / 变更清单 / Git 概览与操作
  fsList: (path) => req(`/fs/list?path=${encodeURIComponent(path)}`),
  fsFile: (path) => req(`/fs/file?path=${encodeURIComponent(path)}`),
  fsSave: (path, content) => req("/fs/file", { method: "PUT", body: JSON.stringify({ path, content }) }),
  workspaceChanges: () => req("/workspace/changes"),
  gitOverview: () => req("/git/overview"),
  gitOp: (payload) => req("/git/op", { method: "POST", body: JSON.stringify(payload) }),
};

// ────────────────────────────── 全局状态 ──────────────────────────────

const S = {
  sessions: [],
  sessionId: null,
  messages: [],        // 已收束消息 {id, role, agentId?, statements[{tag,text}], toolCalls?, thinking?, createdAt}
  running: false,
  unitView: null,      // 当前子代理会话视图（null = 指挥体主会话）
  activeProfileId: null, // 生效模型档案 id
  connected: false,
  target: null,        // {mode, id, name?, primary?}
  config: null,
  drill: null,         // 右栏下钻的子个体标识（null = 任务总览）；@see renderRight
  dispatches: [],      // 派发单缓存（存储事件 type=dispatch，按 to=个体 过滤）
  // 输入区上下文芯片：智能体组 / 智能体 / 模型档案 / 思考强度 / 附件
  groups: [],          // GroupMeta[]（项目口径：组 = 项目，workspace 即项目目录）
  singles: [],
  profiles: [],        // LlmProfile[]（模型档案）
  effort: (() => { try { return localStorage.getItem("exm.effort") ?? "default"; } catch { return "default"; } })(),
  // default=端点默认 | low | medium | high（模型推理预算，全局生效，本地记忆 + 启动时重设）
  images: [],          // data URL 附件
  view: "chat",        // chat | settings（设置视图：模型/组/通道/定时/安全，直连环口 API）
  settingsTab: "model",
  graph: null,         // TaskGraph（任务派发图：指挥体 → 子个体）
  reports: [],         // 子个体回执 {agent, summary, confidence, nodeId}
  workspace: "",       // 当前生效工作目录（干活的项目；空 = 全局根）
  rightOpen: (() => { try { return localStorage.getItem("exm.rightOpen") !== "0"; } catch { return true; } })(),
  // 本轮实时区（run.finished 后清空并并入最终消息）
  live: { orch: "", thinking: "", units: {}, unitThinking: {}, toolCalls: [], activity: [], approvals: [] },
  approvalsPending: 0, // 待处理审批数（侧栏角标；通道等跨会话审批也要亮）
  // 编码页状态：树按目录懒加载（"" = 工作区根）；file 为编辑器当前内容（dirty 才可保存）
  code: {
    tree: { "": { open: true, loaded: false, entries: [] } },
    file: null,          // {path, content, size, truncated, binary, readonly, dirty}（content 兼作未保存标记的基线）
    changes: null,       // /workspace/changes 原样数据
    overview: null,      // /git/overview 原样数据
    rightTab: "changes", // changes | git
    changeMap: new Map(),// path → 变更字母（文件树标记）
  },
};

// ────────────────────────────── SFX：界面音效（Web Audio API 程序化合成） ──────────────────────────────
// 零音频文件 / 零外部依赖 / 零网络请求：振荡器 + 包络现场合成。主增益 0.12，宁轻勿吵。
// 静音口径：localStorage exm.sfx（默认开）；prefers-reduced-motion 用户默认静音（与动效减免一致）。
// 纪律：hover 不发声；splash 期间无循环音，仅网关就绪淡出时一声 connect；批量表单渲染只建按钮不发声。

const sfx = (() => {
  let ctx = null;
  let master = null;
  const reduced = Boolean(window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches);
  const enabled = () => {
    try { const v = localStorage.getItem("exm.sfx"); return v == null ? !reduced : v === "1"; }
    catch { return !reduced; }
  };
  let on = enabled();

  // 共享 AudioContext 懒初始化；自动播放策略下挂起则尝试 resume（首次用户手势后生效，未就绪即静默丢弃）
  function ready() {
    if (!on) return null;
    if (!ctx) {
      const AC = window.AudioContext ?? window.webkitAudioContext;
      if (!AC) return null;
      ctx = new AC();
      master = ctx.createGain();
      master.gain.value = 0.12; // 主增益纪律：0.08~0.15
      master.connect(ctx.destination);
    }
    if (ctx.state === "suspended") void ctx.resume();
    return ctx.state === "running" ? ctx : null;
  }

  // 单音：type 波形，f0→f1 频率滑移（f1=0 不滑），at 相对起始秒，dur 时长，vol 峰值（乘主增益）
  function tone({ type = "sine", f0, f1 = 0, at = 0, dur = 0.06, vol = 0.6 }) {
    const c = ready();
    if (!c) return;
    const t0 = c.currentTime + at;
    const osc = c.createOscillator();
    const g = c.createGain();
    osc.type = type;
    osc.frequency.setValueAtTime(f0, t0);
    if (f1) osc.frequency.exponentialRampToValueAtTime(Math.max(f1, 1), t0 + dur);
    g.gain.setValueAtTime(0.0001, t0);
    g.gain.exponentialRampToValueAtTime(vol, t0 + 0.005);  // 5ms 快起音：清脆不拖
    g.gain.exponentialRampToValueAtTime(0.0001, t0 + dur); // 指数快衰减：滴一声即收
    osc.connect(g).connect(master);
    osc.start(t0);
    osc.stop(t0 + dur + 0.02);
  }

  // 音效清单（合成参数即音色，可微调）
  const sounds = {
    // click 清脆滴：方波 880→660Hz，40ms 快衰减
    click: () => tone({ type: "square", f0: 880, f1: 660, dur: 0.04, vol: 0.5 }),
    // send 发射感：正弦 520→880Hz 上滑，90ms
    send: () => tone({ f0: 520, f1: 880, dur: 0.09, vol: 0.7 }),
    // success 确认：双音 660+990Hz 顺序，120ms
    success: () => { tone({ f0: 660, dur: 0.06 }); tone({ f0: 990, at: 0.06, dur: 0.06 }); },
    // error 低哑否定：锯齿 220→180Hz，150ms
    error: () => tone({ type: "sawtooth", f0: 220, f1: 180, dur: 0.15, vol: 0.55 }),
    // notify 注意：三连短音 880/1100/880
    notify: () => { tone({ f0: 880, dur: 0.05 }); tone({ f0: 1100, at: 0.07, dur: 0.05 }); tone({ f0: 880, at: 0.14, dur: 0.06 }); },
    // connect 启动完成：正弦 300→1200Hz 上扫 250ms + 包络淡出
    connect: () => tone({ f0: 300, f1: 1200, dur: 0.25, vol: 0.65 }),
  };

  return {
    play(name) { if (!on) return; try { sounds[name]?.(); } catch { /* 音频不可用不干扰功能 */ } },
    toggle() {
      on = !on;
      try { localStorage.setItem("exm.sfx", on ? "1" : "0"); } catch { /* 忽略 */ }
      return on;
    },
    get enabled() { return on; },
  };
})();

// ────────────────────────────── WS 事件流 ──────────────────────────────

let ws = null;
let wsGen = 0;
let wsBackoff = 0;
let wsLastFrame = 0;
const WS_STALE_MS = 45000;

function wsConnect() {
  const sessionId = S.sessionId;
  if (!sessionId) return;
  const gen = ++wsGen;
  ws?.close();
  const proto = GW.url.startsWith("https") ? "wss" : "ws";
  const host = GW.url.replace(/^https?:\/\//, "");
  const keyQ = GW.key ? `&key=${encodeURIComponent(GW.key)}` : "";
  ws = new WebSocket(`${proto}://${host}/ws?sessionId=${sessionId}${keyQ}`);
  wsLastFrame = Date.now();
  let reconnected = false;

  ws.onopen = () => {
    if (gen !== wsGen) return;
    wsBackoff = 0;
    setConn(true);
    if (!reconnected) return;
    void refreshApprovalsBadge(); // 重连补偿的第一拍：先校准跨会话审批角标
    // 断连补偿：重拉消息 + 任务图 + 在途判定，补齐丢失事件
    void (async () => {
      try {
        const [messages, graph] = await Promise.all([api.messages(sessionId), api.graph(sessionId)]);
        if (gen !== wsGen || S.sessionId !== sessionId) return;
        S.messages = messages;
        S.graph = graph?.nodes?.length ? graph : null;
        const pending = (graph?.nodes ?? []).some((n) => ["running", "dispatched", "syncing"].includes(String(n.status ?? "")));
        setRunning(pending);
        renderStream();
        scheduleRight(true);
      } catch { /* 下轮重连再补偿 */ }
    })();
  };
  ws.onclose = () => {
    if (gen !== wsGen) return;
    setConn(false);
    reconnected = true;
    const delay = [2000, 5000, 10000, 30000][Math.min(wsBackoff++, 3)];
    setTimeout(() => { if (gen === wsGen && S.sessionId === sessionId) wsConnect(); }, delay);
  };
  ws.onmessage = (m) => {
    wsLastFrame = Date.now();
    try { handleEvent(JSON.parse(String(m.data))); } catch { /* 忽略坏帧 */ }
  };
  if (!wsConnect.watch) {
    wsConnect.watch = setInterval(() => {
      if (ws && ws.readyState === WebSocket.OPEN && Date.now() - wsLastFrame > WS_STALE_MS) ws.close();
    }, 10000);
  }
}

function handleEvent(evt) {
  // 审批角标先于会话过滤：通道发起的审批不属于当前会话也要亮（事件驱动即时刷新，轮询仅兜底）
  if (evt.type === "approval.required" || evt.type === "approval.resolved") {
    if (evt.type === "approval.required") sfx.play("notify"); // 审批到达 → 三连注意音（通道等跨会话审批同样提醒）
    refreshApprovalsBadge();
    // 审批落地（代执行 push/丢弃等）可能改动工作区：编码页开着就顺手刷新变更与 Git 面板
    if (evt.type === "approval.resolved" && S.view === "code") {
      void refreshChanges();
      void refreshGitOverview();
    }
  }
  if (evt.sessionId !== S.sessionId) return;
  const p = evt.payload ?? {};
  const live = S.live;
  switch (evt.type) {
    case "orchestrator.token":
      live.orch += String(p.delta ?? "");
      scheduleLive();
      break;
    case "orchestrator.thinking":
      live.thinking += String(p.delta ?? "");
      scheduleLive();
      break;
    case "unit.token":
      S.live.units[String(p.agentId ?? "")] = (S.live.units[String(p.agentId ?? "")] ?? "") + String(p.delta ?? "");
      if (S.unitView && S.unitView === String(p.agentId ?? "")) scheduleUnitStream();
      scheduleRight();
      break;
    case "unit.thinking":
      S.live.unitThinking[String(p.agentId ?? "")] = (S.live.unitThinking[String(p.agentId ?? "")] ?? "") + String(p.delta ?? "");
      if (S.unitView && S.unitView === String(p.agentId ?? "")) scheduleUnitStream();
      break;
    case "tool.call":
      live.toolCalls = live.toolCalls.filter((c) => c.callId !== String(p.callId));
      live.toolCalls.push({ callId: String(p.callId), agentId: String(p.agentId ?? ""), tool: String(p.tool ?? ""), status: "running", args: p.args ?? {} });
      scheduleLive();
      break;
    case "tool.result":
      live.toolCalls = live.toolCalls.map((c) => c.callId === String(p.callId)
        ? { ...c, status: p.ok ? "ok" : "error", durationMs: Number(p.durationMs ?? 0), summary: String(p.summary ?? "").slice(0, 400) }
        : c);
      scheduleLive();
      break;
    case "approval.required":
      live.approvals = [...live.approvals.filter((a) => a.approvalId !== String(p.approvalId)),
        { approvalId: String(p.approvalId), agentId: String(p.agentId ?? ""), command: String(p.command ?? "") }];
      renderApprovals();
      break;
    case "approval.resolved":
      live.approvals = live.approvals.filter((a) => a.approvalId !== String(p.approvalId));
      renderApprovals();
      break;
    case "unit.finished": {
      // 子代理直聊收束：实时流转为消息（子代理会话视图即时可见）
      const aid2 = String(p.agentId ?? "");
      delete S.live.units[aid2];
      delete S.live.unitThinking[aid2];
      S.messages = [...S.messages, {
        id: `unit-${Date.now()}`, role: "unit", agentId: aid2,
        statements: p.statements ?? [], createdAt: new Date().toISOString(),
      }];
      scheduleRight();
      renderStream(true);
      break;
    }
    case "graph.updated":
      S.graph = p ?? null;
      scheduleRight();
      // 任务视图开着：跟随推送实时刷新（不依赖手动刷新）
      if (S.view === "tasks") refreshTasks();
      break;
    case "dispatch.sent": {
      // 本地同步节点状态（graph.updated 全量图随后也会到达，此处即时反馈）
      const node = S.graph?.nodes?.find((n) => n.id === String(p.nodeId ?? ""));
      if (node && ["pending", "ready"].includes(node.status)) node.status = "dispatched";
      live.activity.push({ text: `派发 <b>@${esc(String(p.agentIdentifier ?? ""))}</b> → ${esc(String(node?.title || p.nodeId || ""))}` });
      scheduleLive();
      scheduleRight(true);
      break;
    }
    case "sync.received": {
      const r = p.report ?? {};
      const node = S.graph?.nodes?.find((n) => n.id === String(r.taskNodeId ?? ""));
      if (node) node.status = "done";
      delete live.units[String(r.sourceAgent ?? "")];
      S.reports = [...S.reports, { agent: String(r.sourceAgent ?? ""), summary: String(r.summary ?? ""), confidence: Number(r.confidence ?? 0), nodeId: String(r.taskNodeId ?? "") }].slice(-12);
      live.activity.push({ text: `<b>@${esc(String(r.sourceAgent ?? ""))}</b> 回执：${esc(String(r.summary ?? "").slice(0, 90))}（置信 ${Number(r.confidence ?? 0).toFixed(2)}）` });
      sfx.play("notify"); // 子个体回流 → 注意音
      scheduleLive();
      scheduleRight(true);
      break;
    }
    case "run.finished": {
      const statements = p.statements ?? [];
      S.messages = [...S.messages, {
        id: `fin-${Date.now()}`, role: "orchestrator",
        agentId: typeof p.agentId === "string" ? p.agentId : "orchestrator",
        statements,
        toolCalls: live.toolCalls.length ? live.toolCalls : undefined,
        thinking: live.thinking.trim() ? live.thinking : undefined,
        activity: live.activity.length ? live.activity : undefined,
        createdAt: new Date().toISOString(),
      }];
      resetLive();
      setRunning(false);
      renderStream(true);
      scheduleRight(true);
      updateSessionPreview(S.sessionId, statements.map((s) => s.text).join(" "));
      break;
    }
    case "run.error":
      S.messages = [...S.messages, { id: `err-${Date.now()}`, role: "error", statements: [{ tag: "警告", text: String(p.message ?? "运行出错") }], createdAt: new Date().toISOString() }];
      sfx.play("error"); // 运行失败 → 低哑否定
      resetLive();
      setRunning(false);
      renderStream(true);
      break;
  }
}

function resetLive() {
  S.live = { orch: "", thinking: "", units: {}, unitThinking: {}, toolCalls: [], activity: [], approvals: liveApprovalsKeep() };
  // S.reports 有意保留：回执是本轮执行痕迹，右栏继续展示，切会话时才清
}

// 子代理会话实时流：rAF 合帧（token 高频到达）
let unitStreamTimer = null;
function scheduleUnitStream() {
  if (unitStreamTimer) return; // 120ms 合帧；后台标签页 rAF 不触发，故用定时器
  unitStreamTimer = setTimeout(() => {
    unitStreamTimer = null;
    if (!S.unitView) return;
    renderStream(false);
    if (nearBottom()) scrollBottom();
  }, 120);
}

// 进入 / 退出子代理会话（与指挥体主会话同款界面）
function openUnitView(agent) {
  S.unitView = agent;
  if (!S.dispatches.some((d) => d.to === agent)) {
    api.sessionEvents(S.sessionId).then((evs) => {
      S.dispatches = (evs ?? []).filter((e) => e.type === "dispatch" && e.to);
      if (S.unitView === agent) renderStream(false);
    }).catch(() => {});
  }
  updateTopbar();
  renderStream(true);
}

function exitUnitView() {
  S.unitView = null;
  updateTopbar();
  renderStream(true);
}

function updateTopbar() {
  const back = $("btn-back");
  back.classList.toggle("hidden", !S.unitView);
  back.textContent = `‹ @${S.unitView ?? ""}`;
}

function liveApprovalsKeep() { return S.live.approvals; }

// ────────────────────────────── 迷你 Markdown 渲染 ──────────────────────────────
// 安全优先：先整体 HTML 转义再生成有限标记；链接仅放行 http(s)。

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function md(src) {
  const lines = esc(src).split(/\r?\n/);
  let html = "", i = 0;
  const inline = (t) => t
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
    .replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<i>$2</i>")
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
  while (i < lines.length) {
    const line = lines[i];
    if (/^```/.test(line)) {
      const buf = [];
      for (i++; i < lines.length && !/^```/.test(lines[i]); i++) buf.push(lines[i]);
      i++;
      html += `<pre><code>${buf.join("\n")}</code></pre>`;
      continue;
    }
    const h = line.match(/^(#{1,4})\s+(.*)/);
    if (h) { html += `<h${h[1].length}>${inline(h[2])}</h${h[1].length}>`; i++; continue; }
    if (/^\s*([-*_])\s*\1\s*\1[\s-*_]*$/.test(line)) { html += "<hr/>"; i++; continue; }
    if (/^>\s?/.test(line)) {
      const buf = [];
      for (; i < lines.length && /^>\s?/.test(lines[i]); i++) buf.push(lines[i].replace(/^>\s?/, ""));
      html += `<blockquote>${md(buf.join("\n"))}</blockquote>`;
      continue;
    }
    if (/^\s*[-*+]\s+/.test(line)) {
      const buf = [];
      for (; i < lines.length && /^\s*[-*+]\s+/.test(lines[i]); i++) buf.push(`<li>${inline(lines[i].replace(/^\s*[-*+]\s+/, ""))}</li>`);
      html += `<ul>${buf.join("")}</ul>`;
      continue;
    }
    if (/^\s*\d+[.)]\s+/.test(line)) {
      const buf = [];
      for (; i < lines.length && /^\s*\d+[.)]\s+/.test(lines[i]); i++) buf.push(`<li>${inline(lines[i].replace(/^\s*\d+[.)]\s+/, ""))}</li>`);
      html += `<ol>${buf.join("")}</ol>`;
      continue;
    }
    if (/^\|.*\|/.test(line) && i + 1 < lines.length && /^\|[\s:|-]+\|/.test(lines[i + 1])) {
      const rows = [];
      const cells = (l) => l.replace(/^\||\|$/g, "").split("|").map((c) => inline(c.trim()));
      rows.push(`<tr>${cells(line).map((c) => `<th>${c}</th>`).join("")}</tr>`);
      i += 2;
      for (; i < lines.length && /^\|.*\|/.test(lines[i]); i++) rows.push(`<tr>${cells(lines[i]).map((c) => `<td>${c}</td>`).join("")}</tr>`);
      html += `<table>${rows.join("")}</table>`;
      continue;
    }
    if (line.trim() === "") { i++; continue; }
    const buf = [];
    for (; i < lines.length && lines[i].trim() !== "" && !/^(#{1,4}\s|```|>|\s*[-*+]\s|\s*\d+[.)]\s|\|)/.test(lines[i]); i++) buf.push(lines[i]);
    if (!buf.length) buf.push(lines[i++]); // 兜底：孤立的 `|` 行等不满足任何块分支时至少消费一行，防空转
    html += `<p>${inline(buf.join("\n")).replace(/\n/g, "<br/>")}</p>`;
  }
  return html;
}

// ────────────────────────────── 渲染 ──────────────────────────────

const $ = (id) => document.getElementById(id);
const streamEl = $("stream-inner");
const liveEl = document.createElement("div");
liveEl.className = "live";
liveEl.style.display = "none";

// ────────────────────────────── 动效工具（品牌纹样 / stagger 编排） ──────────────────────────────
// 纪律：只写 class 与 CSS 变量，动画本体全部在 style.css（transform/opacity 合成器路径）。

// 品牌纹样：中枢环 + 四向弧（与启动画面 icons/icon.svg 同几何），用于空态 / 加载态 / 分区徽记
function brandMark(cls = "", size = 16) {
  return `<svg class="mark-svg ${cls}" width="${size}" height="${size}" viewBox="0 0 512 512" aria-hidden="true"><circle cx="256" cy="256" r="72" fill="none" stroke="currentColor" stroke-width="50"/><g fill="none" stroke="currentColor" stroke-width="46"><path d="M 396.95 307.30 A 150 150 0 0 1 256 406"/><path d="M 204.70 396.95 A 150 150 0 0 1 106 256"/><path d="M 115.05 204.70 A 150 150 0 0 1 256 106"/><path d="M 307.30 115.05 A 150 150 0 0 1 406 256"/></g></svg>`;
}

// 列表 stagger 入场：递增延迟（默认 36ms，封顶 10 档——长列表尾部不拖尾，全部动画只走合成器）
function staggerIn(els, { step = 36, cap = 10 } = {}) {
  let i = 0;
  for (const el of els) {
    el.style.setProperty("--stagger-delay", `${Math.min(i, cap) * step}ms`);
    el.classList.add("stagger-in");
    i++;
  }
}

function relTime(iso) {
  if (!iso) return "";
  const d = new Date(iso), diff = (Date.now() - d.getTime()) / 1000;
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 172800) return "昨天";
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

function setConn(ok) {
  S.connected = ok;
  const el = $("conn-status");
  el.className = `conn ${ok ? "ok" : "err"}`;
  $("conn-text").textContent = ok ? "已连接" : "重连中…";
}

function setRunning(on) {
  S.running = on;
  $("run-state").classList.toggle("hidden", !on);
  const stop = $("btn-stop");
  stop.classList.toggle("hidden", !on);
  // 复位「停止中…」态：收束（finished/error）或空闲兜底时按钮必须可预期
  if (!on) { stop.disabled = false; stop.textContent = "■ 停止"; }
  renderSidebar(); // 当前会话行的运行态小转圈跟随亮/灭
}

function renderSidebar() {
  const nav = $("session-list");
  nav.innerHTML = "";
  // 按项目（组）分类：组头可折叠，会话归属组内（组 = 项目，workspace 即项目目录）
  const byGroup = new Map();
  for (const s of S.sessions) {
    const gid = s.groupId ?? "default";
    if (!byGroup.has(gid)) byGroup.set(gid, []);
    byGroup.get(gid).push(s);
  }
  // 当前目标组置顶；其余按名称排序
  const activeGid = S.target?.mode === "group" ? (S.target.id ?? "default") : null;
  const gids = [...byGroup.keys()].sort((a, b) => (a === activeGid ? -1 : b === activeGid ? 1 : groupName(a).localeCompare(groupName(b), "zh")));
  for (const gid of gids) {
    const head = document.createElement("div");
    head.className = "group-head";
    head.innerHTML = `<span class="group-dot"></span><span class="group-name">${esc(groupName(gid))}</span><span class="group-count">${byGroup.get(gid).length}</span>`;
    const box = document.createElement("div");
    box.className = "group-box";
    for (const s of byGroup.get(gid)) box.appendChild(sessionEl(s));
    head.addEventListener("click", () => {
      const folded = box.style.display === "none";
      box.style.display = folded ? "" : "none";
      head.classList.toggle("folded", !folded);
    });
    nav.appendChild(head);
    nav.appendChild(box);
  }
  // 入场编排（签名比对：列表重建高频，避免每次都重放动画）——
  // 会话集变化 → 整列级联浮现；仅活动会话切换 → 活动行高亮脉冲
  const idsSig = S.sessions.map((s) => s.id).join(",");
  const activeSig = String(S.sessionId ?? "");
  if (idsSig !== sidebarIdsSig) staggerIn(nav.querySelectorAll(".session-item"), { step: 36, cap: 10 });
  else if (activeSig !== sidebarActiveSig) nav.querySelector(".session-item.active")?.classList.add("flash-in");
  sidebarIdsSig = idsSig;
  sidebarActiveSig = activeSig;
}

// 侧栏动效签名（模块级记忆：与上次渲染比对决定放不放动画）
let sidebarIdsSig = "";
let sidebarActiveSig = "";

function sessionEl(s) {
  const item = document.createElement("div");
  const isActive = s.id === S.sessionId;
  item.className = `session-item${isActive ? " active" : ""}`;
  item.dataset.sid = s.id; // 行内重命名按 id 定位 DOM
  item.innerHTML = `
    <div class="session-title">${isActive && S.running ? '<span class="session-run"><span class="spinner"></span></span>' : ""}${esc(s.title || "未命名会话")}</div>
    <div class="session-preview">${esc(s.lastMessagePreview ?? "")}</div>
    <span class="session-acts"><button class="session-act" data-op="rename" title="重命名">✎</button><button class="session-act danger" data-op="del" title="删除会话">✕</button></span>`;
  item.addEventListener("click", () => selectSession(s.id));
  item.querySelector('[data-op="rename"]').addEventListener("click", (e) => { e.stopPropagation(); startRename(s.id); });
  item.querySelector('[data-op="del"]').addEventListener("click", (e) => {
    e.stopPropagation();
    if (!confirm(`删除会话「${s.title || "未命名"}」？`)) return;
    api.deleteSession(s.id).then(() => {
      S.sessions = S.sessions.filter((x) => x.id !== s.id);
      if (S.sessionId === s.id) {
        S.sessionId = null;
        if (S.sessions.length) selectSession(S.sessions[0].id);
        else newSession();
      } else renderSidebar();
    }).catch(alertErr);
  });
  return item;
}

function stmtHtml(st) {
  return `<div class="stmt"><span class="stmt-tag">${esc(st.tag ?? "报告")}</span><span class="stmt-text md">${md(st.text ?? "")}</span></div>`;
}

function traceHtml(msg) {
  const thinking = msg.thinking?.trim();
  const tools = msg.toolCalls ?? [];
  const activity = msg.activity ?? [];
  if (!thinking && !tools.length && !activity.length) return "";
  return `<div class="trace"><details><summary>过程回看（思维链 / 工具轨迹）</summary>
    ${thinking ? `<div class="thinking-text">${esc(thinking)}</div>` : ""}
    ${tools.map((t) => `<div class="tool-row"><span class="tool-name">${esc(t.tool)}</span>
      <span class="tool-status"><span class="${t.status}">${t.status === "ok" ? "✓" : t.status === "error" ? "✗" : "…"} ${t.durationMs ? `${(t.durationMs / 1000).toFixed(1)}s` : ""}</span></span>
      <span class="tool-summary">${esc(t.summary ?? "")}</span></div>`).join("")}
    ${activity.map((a) => `<div class="act-line">${a.text}</div>`).join("")}
  </details></div>`;
}

function msgHtml(msg) {
  const isUser = msg.role === "user";
  const isError = msg.role === "error";
  const who = isUser ? "你" : isError ? "系统" : `@${msg.agentId || "orchestrator"}`;
  const body = isUser
    ? `<div class="msg-bubble">${esc(msg.statements?.map((s) => s.text).join("\n") ?? "")}</div>`
    : `<div class="msg-bubble">${(msg.statements ?? []).map(stmtHtml).join("") || "<span class='md'></span>"}</div>`;
  return `<div class="msg ${isUser ? "user" : isError ? "assistant error" : "assistant"}">
    <div class="msg-body"><div class="msg-meta">${esc(who)} · ${relTime(msg.createdAt)}</div>${body}${traceHtml(msg)}</div>
  </div>`;
}

// 消息入场动效的增量检测：与上次渲染的消息数比对，只对「新到的」消息挂动画类
let renderedMsgCount = 0;

function renderStream(scroll) {
  // 独立整页（设置 / 编码 / 任务）接管期间不触碰对话 DOM（返回对话时统一重渲染）
  if (S.view !== "chat") return;
  // 子代理会话视图：与主会话同款界面（流 + 输入区共用），内容过滤为该个体的对话
  if (S.unitView) { renderUnitStream(scroll); return; }
  // 已收束消息
  streamEl.innerHTML = "";
  if (!S.messages.length && !S.running) streamEl.appendChild(welcomeEl());
  for (const m of S.messages) streamEl.insertAdjacentHTML("beforeend", msgHtml(m));
  // 入场编排：全新渲染（切会话/首帧）→ 尾部级联；增量到达（新消息）→ 仅新消息滑入
  const prevCount = renderedMsgCount;
  renderedMsgCount = S.messages.length;
  const fresh = S.messages.length - prevCount;
  const msgEls = streamEl.querySelectorAll(":scope > .msg");
  if (prevCount === 0 && S.messages.length) {
    staggerIn([...msgEls].slice(-8), { step: 45, cap: 8 });
  } else if (fresh > 0 && prevCount > 0) {
    if (fresh <= 3) for (const el of [...msgEls].slice(-fresh)) el.classList.add("msg-in");
    else staggerIn([...msgEls].slice(-6), { step: 45, cap: 6 }); // 会话切换 / 撤销等大幅变化
  }
  // 运行中实时区
  streamEl.appendChild(liveEl);
  liveEl.style.display = S.running ? "" : "none";
  if (scroll !== false) scrollBottom();
  scheduleLive(true);
}

// 子代理会话（与指挥体主会话同款界面）：派发单 + 用户/该个体的全部往来 + 实时流
function renderUnitStream(scroll) {
  const agent = S.unitView;
  streamEl.innerHTML = "";
  // 派发单契约卡（谁发的就显示谁的名字）
  const dp = S.dispatches.find((d) => d.to === agent)?.payload;
  const node = (S.graph?.nodes ?? []).find((n) => n.agentIdentifier === agent);
  if (dp || node) {
    streamEl.insertAdjacentHTML("beforeend", `<div class="msg assistant"><div class="msg-body">
      <div class="msg-meta">@${esc(orchestratorId())} · ${esc(node?.title || "派发任务")}</div><div class="msg-bubble">
        <div class="stmt"><span class="stmt-tag">任务</span><span class="stmt-text md">${esc(node?.objective || dp?.goal || "")}</span></div>
        ${(dp?.acceptance ?? []).length ? `<div class="stmt"><span class="stmt-tag">验收</span><span class="stmt-text md">${esc(dp.acceptance.join("；"))}</span></div>` : ""}
      </div></div></div>`);
  }
  // 对话历史：用户消息 + 该个体的回传
  const msgs = S.messages.filter((m) => m.role === "user" || (m.role === "unit" && m.agentId === agent));
  if (!msgs.length && !S.live.units[agent]) {
    streamEl.insertAdjacentHTML("beforeend", `<div class="msg assistant"><div class="msg-body"><div class="msg-bubble" style="color:var(--text-dim)">还没有与 @${esc(agent)} 的直接对话——发消息即可单独与这个个体交流，它会带着上面的任务上下文回答。</div></div></div>`);
  }
  for (const m of msgs) streamEl.insertAdjacentHTML("beforeend", msgHtml(m));
  // 实时流（直聊进行中）
  const liveText = S.live.units[agent];
  const liveThink = S.live.unitThinking[agent];
  if (liveText || liveThink) {
    streamEl.insertAdjacentHTML("beforeend", `<div class="msg assistant" id="unit-live"><div class="msg-body"><div class="msg-meta">@${esc(agent)} · 正在输入</div>
        ${liveThink ? `<div class="trace" style="margin:0 0 6px"><details open><summary>思考中</summary><div class="thinking-text">${esc(liveThink)}</div></details></div>` : ""}
        <div class="live-orch md">${md(liveText ?? "")}<span class="cursor"></span></div>
      </div></div>`);
  }
  // 最新消息滑入（进入子代理视图 / 新消息到达时；流式重建不重放）
  if (scroll !== false) streamEl.querySelector(".msg:last-of-type")?.classList.add("msg-in");
  if (scroll !== false) scrollBottom();
}

// 指挥体标识：组主智能体 id，缺省 orchestrator
function orchestratorId() {
  const gid = S.target?.mode === "group" ? (S.target.id ?? "default") : null;
  return S.groups.find((g) => g.id === gid)?.primary || "orchestrator";
}

function welcomeEl() {
  const el = document.createElement("div");
  el.className = "welcome";
  el.innerHTML = `
    <div class="welcome-mark">${brandMark("", 360)}</div>
    <h1>有什么可以帮你？</h1>
    <p>对话交由当前对象（智能体或智能体组）协作完成；侧栏「编码」进入工作台、「任务」看派发与台账</p>
    <div class="suggest">
      <button data-q="帮我梳理一下这个项目的整体结构，给出模块说明">梳理项目结构，输出模块说明</button>
      <button data-q="写一个 Python 脚本：批量重命名当前目录下的图片文件，按日期编号">写一个批量重命名图片的脚本</button>
      <button data-q="总结今天的待办事项，按优先级排序列出">总结今天的待办，按优先级排序</button>
    </div>`;
  el.querySelectorAll(".suggest button").forEach((b) =>
    b.addEventListener("click", () => { $("input").value = b.dataset.q; autosize(); $("input").focus(); }));
  staggerIn(el.querySelectorAll(".suggest button"), { step: 50, cap: 6 }); // 建议项逐条浮现
  return el;
}

// 实时区（流式 token 高频到达 → rAF 合帧渲染）
let liveTimer = null;
function scheduleLive(force) {
  // setTimeout 而非 rAF：窗口隐藏/后台时 rAF 永不触发，实时区会冻住
  if (liveTimer) { if (!force) return; clearTimeout(liveTimer); }
  liveTimer = setTimeout(() => {
    liveTimer = null;
    if (!S.running) { liveEl.style.display = "none"; return; }
    liveEl.style.display = "";
    const live = S.live;
    const thinking = live.thinking.trim()
      ? `<div class="trace" style="margin:0 0 8px"><details open><summary>思考中</summary><div class="thinking-text">${esc(live.thinking)}</div></details></div>`
      : "";
    const orch = live.orch
      ? `<div class="msg-body"><div class="msg-meta">@orchestrator</div><div class="live-orch md">${md(live.orch)}<span class="cursor"></span></div></div>`
      : `<div class="msg-body"><div class="msg-meta" style="color:var(--text-faint)">正在思考…</div>${thinking}</div>`;
    const tools = live.toolCalls.map((t) => `<div class="tool-row"><span class="tool-name">${esc(t.tool)}</span>
      <span class="tool-status"><span class="${t.status}">${t.status === "ok" ? "✓" : t.status === "error" ? "✗" : "…"}</span></span>
      <span class="tool-summary">${esc(t.summary ?? "")}</span></div>`).join("");
    const activity = live.activity.slice(-12).map((a) => `<div class="act-line">${a.text}</div>`).join("");
    liveEl.innerHTML = `<div class="msg assistant">
      <div class="msg-body">${orch}
        ${tools || activity ? `<div class="trace"><details open><summary>执行过程</summary>${tools}${activity}</details></div>` : ""}
      </div></div>`;
    if (nearBottom()) scrollBottom();
  });
}

let liveAgentId = "";

function nearBottom() {
  const el = $("stream");
  return el.scrollHeight - el.scrollTop - el.clientHeight < 120;
}
function scrollBottom() {
  const el = $("stream");
  el.scrollTop = el.scrollHeight;
}

function renderApprovals() {
  const box = $("approvals");
  box.innerHTML = "";
  for (const a of S.live.approvals) {
    const card = document.createElement("div");
    card.className = "approval-card";
    card.innerHTML = `<div class="approval-head">⚠ 审批请求 · @${esc(a.agentId)}</div>
      <div class="approval-cmd">${esc(a.command)}</div>
      <div class="approval-actions"><button class="approve">批准</button><button class="deny">拒绝</button></div>`;
    card.querySelector(".approve").addEventListener("click", () => decide(a.approvalId, true));
    card.querySelector(".deny").addEventListener("click", () => decide(a.approvalId, false));
    box.appendChild(card);
  }
}

async function decide(id, approve) {
  try {
    await api.decideApproval(id, approve);
    S.live.approvals = S.live.approvals.filter((x) => x.approvalId !== id);
    renderApprovals();
    refreshApprovalsBadge(); // 侧栏角标同步消化
  } catch (e) { alertErr(e); }
}

function updateSessionPreview(id, preview) {
  S.sessions = S.sessions.map((s) => (s.id === id ? { ...s, lastMessagePreview: preview } : s));
  renderSidebar();
}

function alertErr(e) {
  // 统一错误口径：toast 短提示（对话内的运行失败仍走 run.error 消息块，不在此列）
  console.error(e);
  sfx.play("error"); // 失败 → 低哑否定（toast 本体不重复发声）
  toast(String(e).replace(/^Error:\s*/, ""), false);
}

// ────────────────────────────── 会话操作 ──────────────────────────────

async function selectSession(id) {
  S.sessionId = id;
  resetLive();
  setRunning(false);
  S.graph = null;
  S.reports = [];
  S.unitView = null;
  S.dispatches = [];
  updateTopbar();
  renderSidebar();
  renderRight();
  try {
    const [messages, graph] = await Promise.all([api.messages(id), api.graph(id)]);
    if (S.sessionId !== id) return;
    S.messages = messages;
    S.graph = graph?.nodes?.length ? graph : null;
    const pending = (graph?.nodes ?? []).some((n) => ["running", "dispatched", "syncing"].includes(String(n.status ?? "")));
    setRunning(pending);
  } catch (e) {
    S.messages = [];
    alertErr(e);
  }
  renderStream();
  scheduleRight(true);
  wsConnect();
}

async function newSession() {
  const s = await api.createSession("新对话");
  S.sessions = [s, ...S.sessions];
  await selectSession(s.id);
}

async function send() {
  const input = $("input");
  const text = input.value.trim();
  if (!text || !S.sessionId) return;
  sfx.play("send"); // 消息发出 → 发射感上滑
  input.value = "";
  autosize();
  const agent = S.unitView; // 子代理会话视图内 → 直聊该个体
  const label = S.images.length ? `${text}（附 ${S.images.length} 张图片）` : text;
  S.messages = [...S.messages, { id: `local-${Date.now()}`, role: "user", statements: [{ tag: "要求", text: label }], createdAt: new Date().toISOString() }];
  if (!agent) { resetLive(); setRunning(true); }
  renderStream(true);
  updateSessionPreview(S.sessionId, label);
  const images = S.images;
  S.images = [];
  renderAttachRow();
  try {
    await api.chat(S.sessionId, text, images, agent);
  } catch (e) {
    if (!agent) setRunning(false);
    alertErr(e);
  }
}

// ────────────────────────────── 右栏：子个体执行面板 ──────────────────────────────
// 任务派发图（指挥体 → 子个体节点）+ 各子个体实时流 + 回执；运行中自动弹出

const ST_LABELS = {
  pending: "待派发", ready: "就绪", dispatched: "已派发", running: "执行中", syncing: "回流中",
  done: "完成", blocked: "受阻", failed: "失败", arbitrating: "裁决中", cancelled: "已取消",
};

function scheduleRight(force) {
  if (!S.running && !S.graph?.nodes?.length) { renderRight(); return; }
  // 有活动任务时确保面板可见
  if (!S.rightOpen && (S.running || activeNodeCount() > 0)) setRightOpen(true);
  if (scheduleRight.timer) { if (!force) return; clearTimeout(scheduleRight.timer); }
  scheduleRight.timer = setTimeout(() => { scheduleRight.timer = null; renderRight(); }, 120);
}

function activeNodeCount() {
  return (S.graph?.nodes ?? []).filter((n) => ["dispatched", "running", "syncing", "arbitrating"].includes(n.status)).length;
}

function setRightOpen(on) {
  S.rightOpen = on;
  try { localStorage.setItem("exm.rightOpen", on ? "1" : "0"); } catch { /* 忽略 */ }
  $("rightpanel").classList.toggle("collapsed", !on);
  $("btn-right").classList.toggle("on", on);
}

// 右栏动效签名（流式期间 120ms 重建频繁：状态集合没变就不重放入场动画）
let rightSig = "";

function renderRight() {
  const body = $("rp-body");
  const nodes = S.graph?.nodes ?? [];
  const units = Object.entries(S.live.units);
  const unitThink = Object.entries(S.live.unitThinking);
  $("rp-count").textContent = nodes.length ? `${nodes.filter((n) => n.status === "done").length}/${nodes.length}` : "";
  if (!nodes.length && !units.length && !unitThink.length && !S.reports.length) {
    body.innerHTML = `<div class="rp-empty"><div class="empty-mark">${brandMark("", 34)}</div><div>本轮暂无派发任务<br/>指挥体拆解任务后，子个体的执行情况会在这里实时展示；点击个体卡可进入其会话</div></div>`;
    rightSig = "";
    return;
  }
  const sig = `${nodes.map((n) => n.status).join("")}|${units.map(([a]) => a).join(",")}|${S.reports.length}`;
  const sigChanged = sig !== rightSig;
  rightSig = sig;
  let html = "";
  if (nodes.length) {
    html += `<div class="rp-section">任务派发（指挥体 → 子个体，点击查看个体会话）</div>`;
    for (const n of nodes) {
      const isActive = ["dispatched", "running", "syncing", "arbitrating"].includes(n.status);
      const deps = (n.dependsOn ?? []).map((d) => nodes.find((x) => x.id === d)?.title ?? d);
      html += `<div class="task-card${isActive ? " active" : ""}" data-agent="${esc(n.agentIdentifier ?? "")}" style="cursor:pointer" title="进入 @${esc(n.agentIdentifier ?? "")} 的会话">
        <div class="task-top"><span class="task-agent">@${esc(n.agentIdentifier ?? "")}</span>
          <span class="task-status st-${esc(n.status ?? "pending")}">${ST_LABELS[n.status] ?? esc(n.status ?? "")}</span></div>
        <div class="task-title">${esc(n.title || n.objective || "未命名任务")}</div>
        ${n.objective && n.title ? `<div class="task-obj">${esc(n.objective)}</div>` : ""}
        ${deps.length ? `<div class="task-deps">依赖：${esc(deps.join("、"))}</div>` : ""}
      </div>`;
    }
  }
  const liveCards = [...new Set([...units.map(([a]) => a), ...unitThink.map(([a]) => a)])];
  if (liveCards.length) {
    html += `<div class="rp-section">子个体实时输出（点击进入会话）</div>`;
    for (const agent of liveCards) {
      const text = S.live.units[agent];
      const think = S.live.unitThinking[agent];
      html += `<div class="unit-card" data-agent="${esc(agent)}" style="cursor:pointer">
        <div class="unit-head"><span class="spinner"></span>@${esc(agent)}</div>
        ${think ? `<div class="unit-text" style="color:var(--text-faint)">${esc(think)}</div>` : ""}
        ${text ? `<div class="unit-text">${esc(text)}</div>` : ""}
      </div>`;
    }
  }
  if (S.reports.length) {
    html += `<div class="rp-section">回执</div>`;
    for (const r of S.reports.slice(-6).reverse()) {
      html += `<div class="unit-card" data-agent="${esc(r.agent)}" style="border-color:var(--border);background:var(--panel);cursor:pointer">
        <div class="unit-head" style="color:var(--ok)">✓ @${esc(r.agent)} <span style="color:var(--text-faint)">置信 ${r.confidence.toFixed(2)}</span></div>
        <div class="unit-text">${esc(r.summary.slice(0, 160))}</div>
      </div>`;
    }
  }
  body.innerHTML = html;
  // 点击个体卡 → 进入与指挥体同款的子代理会话界面
  body.querySelectorAll("[data-agent]").forEach((el) =>
    el.addEventListener("click", () => openUnitView(el.dataset.agent)));
  // 状态集合变化（新派发 / 新个体 / 回执到达）→ 卡片级联浮现；纯文本流式更新不重放
  if (sigChanged) staggerIn(body.querySelectorAll(".task-card,.unit-card"), { step: 36, cap: 10 });
}

// ────────────────────────────── 设置视图（原生，直连环口 API） ──────────────────────────────
// 桌面端不依赖任何 webui：模型与提供商 / 组与个体 / 通道 / 定时任务 / 安全 全部壳内直管。
// 一切核心功能在 exm-core，桌面端与 webui、channel 一样只是连结核心的客户端。

// 启动画面：最少展示 1.6s（品牌瞬间），网关就绪/初始化完成后淡出移除
const SPLASH_MIN = 1600;
const splashT0 = Date.now();

function hideSplash() {
  const sp = document.getElementById("splash");
  if (!sp || sp.dataset.done) return;
  sp.dataset.done = "1";
  const wait = Math.max(0, SPLASH_MIN - (Date.now() - splashT0));
  setTimeout(() => {
    sp.classList.add("splash-out");
    sfx.play("connect"); // 网关就绪开屏淡出 → 启动完成上扫一声（splash 期间无循环音）
    setTimeout(() => sp.remove(), 750);
  }, wait);
}

const SETTINGS_TABS = [["model", "模型"], ["group", "智能体组"], ["single", "智能体"], ["channel", "通道"], ["cron", "定时任务"], ["approval", "审批"], ["memory", "记忆"], ["skills", "技能"], ["audit", "审计"], ["config", "配置"], ["version", "版本"]];

function showView(v) {
  // 编码页有未保存修改时，切走前先确认（切文件在 openFile、换目录在 applyWorkspace 各自拦截）
  if (S.view === "code" && v !== "code" && !confirmDiscardDirty("离开编码页")) return;
  S.view = v;
  updateTopbar();
  // 独立整页（设置 / 编码 / 任务）接管整个窗口，chat 恢复三栏对话
  $("settings-page").classList.toggle("hidden", v !== "settings");
  $("code-page").classList.toggle("hidden", v !== "code");
  $("tasks-page").classList.toggle("hidden", v !== "tasks");
  $("app").classList.toggle("hidden", v !== "chat");
  if (v === "settings") renderSettings();
  else if (v === "code") enterCode();
  else if (v === "tasks") enterTasks();
  else renderStream(true);
}

function renderView() {
  if (S.view === "settings") renderSettings();
  else if (S.view === "code") enterCode();
  else if (S.view === "tasks") enterTasks();
  else renderStream(true);
}

function toast(msg, ok = true) {
  // 统一容器纵向堆叠：旧实现 position:fixed 同点重叠，多条提示会互相遮挡
  const box = $("toasts");
  const t = document.createElement("div");
  t.className = `toast ${ok ? "ok" : "err"}`; // 进场滑入由 .toast 的 CSS 动画承担
  t.textContent = String(msg).replace(/^Error:\s*/, "");
  box.appendChild(t);
  while (box.children.length > 4) box.firstChild.remove(); // 上限 4 条，防刷屏
  setTimeout(() => t.classList.add("out"), 2800); // 退场：下滑淡出后再移除
  setTimeout(() => t.remove(), 3140);
}

function renderSettings() {
  // 竖排导航
  const tabs = $("sp-tabs");
  tabs.innerHTML = "";
  for (const [id, label] of SETTINGS_TABS) {
    const b = document.createElement("button");
    b.className = `set-tab${S.settingsTab === id ? " on" : ""}`;
    b.textContent = label;
    b.addEventListener("click", () => { S.settingsTab = id; renderSettings(); });
    tabs.appendChild(b);
  }
  // 内容区
  const body = $("sp-body");
  body.innerHTML = "";
  ({ model: renderSetModel, group: renderSetGroup, single: renderSetSingles, channel: renderSetChannel, cron: renderSetCron, approval: renderSetApprovals, memory: renderSetMemory, skills: renderSetSkills, audit: renderSetAudit, config: renderSetConfig, version: renderSetVersion }[S.settingsTab] ?? renderSetModel)(body);
}

// —— 表单小件 ——
function mkField(label, input) {
  const w = document.createElement("label");
  w.className = "fld";
  const l = document.createElement("span");
  l.className = "fld-label";
  l.textContent = label;
  w.appendChild(l);
  w.appendChild(input);
  return w;
}
function mkInput(value, opts = {}) {
  const i = document.createElement("input");
  i.className = "pop-input";
  if (opts.type) i.type = opts.type;
  if (value != null) i.value = value;
  i.placeholder = opts.placeholder ?? "";
  return i;
}
function mkSelect(options, value) {
  const s = document.createElement("select");
  s.className = "pop-input";
  for (const [v, t] of options) {
    const o = document.createElement("option");
    o.value = v; o.textContent = t;
    if (v === value) o.selected = true;
    s.appendChild(o);
  }
  return s;
}
function mkBtn(text, onclick, cls = "pop-save") {
  const b = document.createElement("button");
  b.className = cls;
  b.textContent = text;
  // 全部 mkBtn 点击 → click（批量包装处：此处一行覆盖所有调用点；批量表单渲染只建按钮，不发声）
  // 统一 loading 态：async 处理器执行期间挂 .btn-busy（转圈 + 禁点），结束自动摘除
  b.addEventListener("click", (...args) => {
    sfx.play("click");
    const r = onclick(...args);
    if (r && typeof r.finally === "function") {
      b.classList.add("btn-busy");
      r.finally(() => b.classList.remove("btn-busy"));
    }
  });
  return b;
}
function setCard(title) {
  const c = document.createElement("div");
  c.className = "set-card";
  const h = document.createElement("div");
  h.className = "set-card-title";
  const mark = document.createElement("span");
  mark.className = "card-mark";
  mark.innerHTML = brandMark("", 12); // 分区标题旁的品牌小徽记
  h.appendChild(mark);
  h.appendChild(document.createTextNode(title));
  c.appendChild(h);
  return c;
}
async function settingsSave(promise, okMsg) {
  try { await promise; sfx.play("success"); toast(okMsg); return true; }
  catch (e) { sfx.play("error"); toast(String(e).slice(0, 160), false); return false; }
}

// —— 模型与提供商 ——
function renderSetModel(body) {
  const info = { active: S.activeProfileId, profiles: S.profiles };
  const list = setCard("提供商档案（LLM）");
  if (!S.profiles.length) list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "尚未配置提供商——新增档案并填写端点与 API Key 后即可对话。" }));
  for (const p of S.profiles) {
    const row = document.createElement("div");
    row.className = "set-row";
    row.innerHTML = `<div class="set-row-main"><b>${esc(p.name)}</b>${p.id === S.activeProfileId ? '<span class="tag-ok">✓ 生效</span>' : ""}
      <div class="set-sub">${esc(p.baseUrl)} · ${esc(p.apiFormat || "openai")} · 模型 ${esc(p.model || "未指定")}</div></div>`;
    const ops = document.createElement("div");
    ops.className = "set-ops";
    if (p.id !== S.activeProfileId) ops.appendChild(mkBtn("设为生效", async () => {
      if (await settingsSave(req("/llm/active", { method: "PUT", body: JSON.stringify({ id: p.id }) }), "已切换生效档案")) await refreshContext(), renderSettings();
    }, "set-btn"));
    ops.appendChild(mkBtn("测连通", async () => {
      const r = await req("/llm/test", { method: "POST", body: JSON.stringify({ id: p.id }) }).catch((e) => ({ error: String(e) }));
      toast(r?.ok ? `连通正常（HTTP ${r.status ?? "mock"}）` : `失败：${r?.error || r?.message || "未知"}`, Boolean(r?.ok));
    }, "set-btn"));
    ops.appendChild(mkBtn("编辑", () => { profileForm(p); }, "set-btn"));
    ops.appendChild(mkBtn("删除", async () => {
      if (!confirm(`删除档案「${p.name}」？`)) return;
      if (await settingsSave(req(`/llm/profiles/${p.id}`, { method: "DELETE" }), "已删除")) await refreshContext(), renderSettings();
    }, "set-btn danger"));
    row.appendChild(ops);
    list.appendChild(row);
  }
  body.appendChild(list);
  body.appendChild(mkBtn("＋ 新增档案", () => profileForm(null)));
  body.appendChild(profileFormSlot());
  // 能力模型槽位："档案ID" 或 "档案ID/模型名"；空 = 全局档案默认
  const caps = setCard("能力模型槽位（语音 / 转写 / 视觉转述 / 嵌入）");
  req("/llm/capabilities").then((cap) => {
    const mk = (label, key) => {
      const i = mkInput(cap[key] ?? "", { placeholder: "档案ID 或 档案ID/模型名（空 = 档案默认）" });
      caps.appendChild(mkField(label, i));
      return i;
    };
    const speech = mk("语音合成（TTS）", "speech");
    const transcribe = mk("语音转写（STT）", "transcribe");
    const vision = mk("视觉转述", "visionRelay");
    const embedding = mk("语义检索嵌入", "embedding");
    caps.appendChild(mkBtn("保存能力槽位", async () => {
      if (await settingsSave(req("/llm/capabilities", { method: "PUT", body: JSON.stringify({ speech: speech.value.trim(), transcribe: transcribe.value.trim(), visionRelay: vision.value.trim(), embedding: embedding.value.trim() }) }), "能力槽位已保存")) renderSettings();
    }));
  }).catch(() => caps.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "能力槽位加载失败" })));
  body.appendChild(caps);
  function profileFormSlot() { const d = document.createElement("div"); d.id = "profile-form"; return d; }
  function profileForm(p) {
    const slot = body.querySelector("#profile-form") || body.appendChild(profileFormSlot());
    slot.innerHTML = "";
    const c = setCard(p ? `编辑档案：${p.name}` : "新增档案");
    const PRESETS = [
      ["", "厂商预设（可选）"],
      ["openai|https://api.openai.com/v1|gpt-4o|openai", "OpenAI"],
      ["anthropic|https://api.anthropic.com|claude-sonnet-4-5|anthropic", "Anthropic"],
      ["gemini|https://generativelanguage.googleapis.com/v1beta|gemini-2.5-flash|gemini", "Google Gemini"],
      ["azure|https://<resource>.openai.azure.com|<deployment>|azure", "Azure OpenAI"],
      ["deepseek|https://api.deepseek.com/v1|deepseek-chat|openai", "DeepSeek"],
      ["moonshot|https://api.moonshot.cn/v1|moonshot-v1-32k|openai", "Moonshot / Kimi"],
      ["qwen|https://dashscope.aliyuncs.com/compatible-mode/v1|qwen-plus|openai", "Qwen 通义千问"],
      ["zhipu|https://open.bigmodel.cn/api/paas/v4|glm-4-plus|openai", "Zhipu GLM"],
      ["minimax|https://api.minimaxi.com/v1|MiniMax-M1|openai", "MiniMax"],
      ["ollama|http://127.0.0.1:11434/v1|llama3.1|openai", "Ollama 本地"],
    ];
    const preset = mkSelect(PRESETS, "");
    preset.addEventListener("change", () => {
      if (!preset.value) return;
      const [tag, url, model2, format] = preset.value.split("|");
      baseUrl.value = url;
      model.value = model2;
      fmt.value = format;
    });
    const name = mkInput(p?.name ?? "", { placeholder: "如 DeepSeek / GLM" });
    const baseUrl = mkInput(p?.baseUrl ?? "https://api.openai.com/v1");
    const fmt = mkSelect([["openai", "OpenAI 兼容"], ["anthropic", "Anthropic"], ["gemini", "Gemini"], ["azure", "Azure OpenAI"]], p?.apiFormat || "openai");
    const keys = document.createElement("textarea");
    keys.className = "pop-input"; keys.rows = 2;
    keys.placeholder = p?.apiKeys?.length ? `已配置 ${p.apiKeys.length} 把（留空沿用）` : "API Key，每行一把（多 Key 自动负载均衡）";
    const model = mkInput(p?.model ?? "", { placeholder: "默认模型名，如 GLM-5.3-Flash" });
    const profilesNow = S.profiles.filter((x) => x.id !== p?.id);
    const fallback = mkSelect([["", "无回退（跟随候选链）"], ...profilesNow.map((x) => [x.id, `失败回退 → ${x.name}`])], p?.fallback ?? "");
    c.appendChild(mkField("厂商预设", preset));
    c.appendChild(mkField("名称", name));
    c.appendChild(mkField("端点 Base URL", baseUrl));
    c.appendChild(mkField("协议", fmt));
    c.appendChild(mkField("API Key（每行一把）", keys));
    c.appendChild(mkField("默认模型", model));
    c.appendChild(mkField("失败回退", fallback));

    // 模型清单管理：名称 + 视觉 + 语音 + 移除；可从端点拉取填充
    const modelsState = (p?.models ?? []).map((m) => ({ ...m }));
    const mBox = document.createElement("div");
    const renderModels = () => {
      mBox.innerHTML = "";
      if (!modelsState.length) { mBox.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "模型清单为空——拉取端点清单或手动添加（视觉/语音影响多模态路由）" })); return; }
      modelsState.forEach((m2, idx) => {
        const row = document.createElement("div");
        row.className = "agent-add";
        const mi = mkInput(m2.model, { placeholder: "模型名" });
        mi.addEventListener("input", () => { modelsState[idx].model = mi.value; });
        const vision = mkSelect([["", "视觉?"], ["true", "有视觉"], ["false", "无视觉"]], String(m2.vision ?? ""));
        vision.addEventListener("change", () => { modelsState[idx].vision = vision.value === "true"; });
        const audio = mkSelect([["", "语音?"], ["true", "有语音"], ["false", "无语音"]], String(m2.audio ?? ""));
        audio.addEventListener("change", () => { modelsState[idx].audio = audio.value === "true"; });
        const rm = mkBtn("✕", () => { modelsState.splice(idx, 1); renderModels(); }, "set-btn danger");
        row.appendChild(mi); row.appendChild(vision); row.appendChild(audio); row.appendChild(rm);
        mBox.appendChild(row);
      });
    };
    renderModels();
    c.appendChild(mkField("模型清单", mBox));
    const mops = document.createElement("div");
    mops.className = "set-ops";
    mops.appendChild(mkBtn("＋ 手动添加模型", () => { modelsState.push({ model: "", vision: false, audio: false }); renderModels(); }, "set-btn"));
    mops.appendChild(mkBtn("从端点拉取", async () => {
      const payload = { baseUrl: baseUrl.value.trim(), apiFormat: fmt.value };
      if (p?.id) payload.id = p.id;
      if (keys.value.trim()) payload.apiKey = keys.value.trim().split("\n")[0];
      const r = await req("/llm/models", { method: "POST", body: JSON.stringify(payload) }).catch((e) => ({ error: String(e) }));
      if (!r?.ok) return toast(`拉取失败：${r?.error || r?.message || "未知"}`, false);
      for (const m2 of r.models ?? []) if (!modelsState.some((x) => x.model === m2)) modelsState.push({ model: m2, vision: false, audio: false });
      renderModels();
      toast(`端点返回 ${r.models.length} 个模型`);
    }, "set-btn"));
    c.appendChild(mops);

    const ops = document.createElement("div");
    ops.className = "set-ops";
    ops.appendChild(mkBtn("保存", async () => {
      const payload = { id: p?.id, name: name.value.trim() || "未命名", baseUrl: baseUrl.value.trim(), apiFormat: fmt.value, model: model.value.trim() };
      if (keys.value.trim()) payload.apiKeys = keys.value.split("\n").map((s) => s.trim()).filter(Boolean);
      if (fallback.value) payload.fallback = fallback.value;
      if (modelsState.length && modelsState.every((m2) => m2.model.trim())) payload.models = modelsState.map((m2) => ({ model: m2.model.trim(), vision: Boolean(m2.vision), audio: Boolean(m2.audio) }));
      if (await settingsSave(req("/llm/profiles", { method: "POST", body: JSON.stringify(payload) }), "档案已保存")) { await refreshContext(); renderSettings(); }
    }));
    if (p) ops.appendChild(mkBtn("测连通", async () => {
      const r = await req("/llm/test", { method: "POST", body: JSON.stringify({ id: p.id }) }).catch((e) => ({ error: String(e) }));
      toast(r?.ok ? `连通正常（HTTP ${r.status ?? "mock"}）` : `失败：${r?.error || r?.message || "未知"}`, Boolean(r?.ok));
    }, "set-btn"));
    c.appendChild(ops);
    slot.appendChild(c);
    slot.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }
}

// —— 组与个体 ——
function renderSetGroup(body) {
  // 编成模板（建组/添个体可选）
  let templates = [];
  req("/templates").then((r) => { templates = r.templates ?? []; }).catch(() => {});

  const list = setCard("智能体组（项目协作单元）");
  for (const g of S.groups) {
    const row = document.createElement("div");
    row.className = "set-row";
    const active = S.target?.mode === "group" && (S.target.id ?? "default") === g.id;
    row.innerHTML = `<div class="set-row-main"><b>${esc(g.name)}</b>${active ? '<span class="tag-ok">✓ 当前</span>' : g.builtin ? '<span class="tag-dim">内置</span>' : ""}
      <div class="set-sub">${esc(g.description || "")}</div>
      <div class="set-sub set-overview" data-gid="${esc(g.id)}">统计加载中…</div>
      <div class="set-sub">主智能体 ${esc(g.primary || "未指定")} · ${esc(g.workspace || "未设工作目录")}</div>
      <div class="set-agents" data-gid="${esc(g.id)}"></div></div>`;
    const ops = document.createElement("div");
    ops.className = "set-ops";
    if (!active) ops.appendChild(mkBtn("设为当前", async () => {
      if (await settingsSave(req("/groups/active", { method: "PUT", body: JSON.stringify({ id: g.id }) }), "已切换")) await refreshContext(), renderSettings();
    }, "set-btn"));
    ops.appendChild(mkBtn("组设置", () => groupSettingsCard(row, g, templates), "set-btn"));
    if (!g.builtin) ops.appendChild(mkBtn("删除", async () => {
      if (!confirm(`删除组「${g.name}」？`)) return;
      if (await settingsSave(req(`/groups/${g.id}`, { method: "DELETE" }), "已删除")) await refreshContext(), renderSettings();
    }, "set-btn danger"));
    row.appendChild(ops);
    list.appendChild(row);

    // overview 统计（个体/编成/会话/记忆）
    req(`/groups/${g.id}/overview`).then((ov) => {
      row.querySelector(".set-overview").textContent =
        `个体 ${ov.agents} · 编成 ${ov.playbooks} · 会话 ${ov.sessions} · 记忆 组 ${ov.memory.group} / 共享 ${ov.memory.shared}`;
    }).catch(() => { const e = row.querySelector(".set-overview"); if (e) e.textContent = ""; });

    // 组内个体：行式管理（设主 / 人格 / 提示词 / 移除）
    req(`/groups/${g.id}/agents`).then((agents) => {
      const box = row.querySelector(".set-agents");
      box.innerHTML = "";
      for (const a of agents) {
        const isPrimary = g.primary === a.identifier;
        const ar = document.createElement("div");
        ar.className = "agent-row";
        ar.innerHTML = `<span class="agent-name">@${esc(a.identifier)}</span><span class="agent-desc">${esc(a.name || "")} · ${esc(a.domain || "")}</span>
          <span class="agent-ops">
            ${isPrimary ? '<i class="pri">主</i>' : `<button class="set-btn" data-op="pri">设主</button>`}
            <button class="set-btn" data-op="persona">人格</button>
            <button class="set-btn" data-op="prompt">提示词</button>
            <button class="set-btn danger" data-op="rm">移除</button>
          </span>`;
        const edit = document.createElement("div");
        ar.appendChild(edit);
        ar.querySelector('[data-op="pri"]')?.addEventListener("click", async () => {
          if (await settingsSave(req(`/groups/${g.id}/primary`, { method: "POST", body: JSON.stringify({ identifier: a.identifier }) }), `主智能体 → @${a.identifier}`)) renderSettings();
        });
        ar.querySelector('[data-op="persona"]').addEventListener("click", () => personaCard(edit, a.identifier, { kind: "group-agent", gid: g.id }));
        ar.querySelector('[data-op="prompt"]').addEventListener("click", () => promptCard(edit, { kind: "group-agent", gid: g.id, identifier: a.identifier }));
        ar.querySelector('[data-op="rm"]').addEventListener("click", async () => {
          if (!confirm(`从组移除 @${a.identifier}？`)) return;
          if (await settingsSave(req(`/groups/${g.id}/agents/${a.identifier}`, { method: "DELETE" }), "已移除")) renderSettings();
        });
        box.appendChild(ar);
      }
      // 组内新增个体（可选编成模板）；内置组编成受保护（服务端拒绝），仅自定义组开放
      if (g.builtin) {
        box.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "内置组编成受保护——新建自定义组后即可添加个体。" }));
        return;
      }
      const add = document.createElement("div");
      add.className = "agent-add";
      const nameI = mkInput("", { placeholder: "名称" });
      const idI = mkInput("", { placeholder: "标识（如 coder-1）" });
      const domI = mkInput("", { placeholder: "负责域（可选）" });
      const tpl = mkSelect([["", "模板（可选）"], ...templates.filter((t) => t.kind === "unit").map((t) => [t.id, t.name])], "");
      add.appendChild(nameI); add.appendChild(idI); add.appendChild(domI); add.appendChild(tpl);
      add.appendChild(mkBtn("添加个体", async () => {
        if (!nameI.value.trim() || !idI.value.trim()) return toast("名称与标识必填", false);
        const payload = { name: nameI.value.trim(), identifier: idI.value.trim(), description: nameI.value.trim(), domain: domI.value.trim(), group: g.id };
        if (tpl.value) payload.template = tpl.value;
        if (await settingsSave(req("/agents", { method: "POST", body: JSON.stringify(payload) }), "个体已添加")) renderSettings();
      }, "set-btn"));
      box.appendChild(add);
    }).catch(() => {});
  }
  body.appendChild(list);

  // 组设置卡：模型 + 能力槽位（组级覆盖）
  function groupSettingsCard(row, g, templates) {
    const main = row.querySelector(".set-row-main");
    const old = main.querySelector(".group-settings");
    if (old) { old.remove(); return; }
    const card = document.createElement("div");
    card.className = "group-settings";
    const modelI = mkInput(g.model ?? "", { placeholder: "档案ID 或 档案ID/模型名（空 = 跟随全局）" });
    card.appendChild(mkField("组默认模型", modelI));
    req("/llm/capabilities").then(async (gc) => {
      const cap = g.capabilities ?? {};
      const s1 = mkInput(cap.speech ?? gc.speech ?? "", { placeholder: "档案ID 或 档案ID/模型名" });
      const s2 = mkInput(cap.transcribe ?? gc.transcribe ?? "", { placeholder: "同上" });
      const s3 = mkInput(cap.visionRelay ?? gc.visionRelay ?? "", { placeholder: "同上" });
      const s4 = mkInput(cap.embedding ?? gc.embedding ?? "", { placeholder: "同上" });
      card.appendChild(mkField("语音合成", s1));
      card.appendChild(mkField("语音转写", s2));
      card.appendChild(mkField("视觉转述", s3));
      card.appendChild(mkField("语义嵌入", s4));
      card.appendChild(mkBtn("保存组设置", async () => {
        const caps = {};
        if (s1.value.trim()) caps.speech = s1.value.trim();
        if (s2.value.trim()) caps.transcribe = s2.value.trim();
        if (s3.value.trim()) caps.visionRelay = s3.value.trim();
        if (s4.value.trim()) caps.embedding = s4.value.trim();
        const ok1 = await settingsSave(req(`/groups/${g.id}/model`, { method: "PUT", body: JSON.stringify({ model: modelI.value.trim() }) }), "组模型已保存");
        const ok2 = await settingsSave(req(`/groups/${g.id}/capabilities`, { method: "PUT", body: JSON.stringify(caps) }), "组能力已保存");
        if (ok1 && ok2) { await refreshContext(); renderSettings(); }
      }));
    }).catch(() => {});
    main.appendChild(card);
  }

  // 新建组（可选编成模板）
  const create = setCard("新建智能体组");
  const name = mkInput("", { placeholder: "组名称，如 前端小组" });
  const desc = mkInput("", { placeholder: "一句话描述（可选）" });
  const gtpl = mkSelect([["", "编成模板（可选）"], ...templates.map((t) => [t.id, `${t.name}（${t.kind === "group" ? "组" : "个体"}）`])], "");
  create.appendChild(mkField("名称", name));
  create.appendChild(mkField("描述", desc));
  create.appendChild(mkField("模板", gtpl));
  create.appendChild(mkBtn("创建", async () => {
    if (!name.value.trim()) return toast("组名称不能为空", false);
    const payload = { name: name.value.trim() };
    if (desc.value.trim()) payload.description = desc.value.trim();
    if (gtpl.value) payload.template = gtpl.value;
    if (await settingsSave(req("/groups", { method: "POST", body: JSON.stringify(payload) }), "组已创建")) { await refreshContext(); renderSettings(); }
  }));
  body.appendChild(create);
}

// —— 智能体（单体）：对齐 SinglesView——档案卡 + 模型/描述/人格/提示词/删除/设为对话目标 ——
function renderSetSingles(body) {
  const list = setCard("智能体（单体，可直接作为对话目标）");
  const box = document.createElement("div");
  list.appendChild(box);
  const render = async () => {
    const [{ singles: list2 }, profiles] = await Promise.all([req("/singles"), req("/llm/profiles").catch(() => ({ profiles: [] }))]);
    box.innerHTML = "";
    const modelLabel = (hint) => {
      if (!hint) return "跟随全局";
      if (hint.includes("/")) return hint.split("/").pop();
      const p = profiles.profiles.find((x) => x.id === hint);
      return p ? (p.model || p.name) : hint;
    };
    if (!list2.length) list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "暂无智能体——下方新建后可设为对话目标。" }));
    for (const s of list2) {
      const active = S.target?.mode === "single" && S.target.id === s.identifier;
      const card = document.createElement("div");
      card.className = "set-card";
      card.style.marginBottom = "10px";
      card.innerHTML = `<div class="set-card-title">@${esc(s.identifier)} ${active ? '<span class="tag-ok">✓ 对话中</span>' : ""}</div>
        <div class="set-sub">${esc(s.name || "")} · ${esc(s.domain || "")} · 模型 <b>${esc(modelLabel(s.modelHint))}</b></div>
        <div class="set-sub">${esc(s.description || "")}</div>`;
      const ops = document.createElement("div");
      ops.className = "set-ops";
      ops.style.marginTop = "8px";
      if (!active) ops.appendChild(mkBtn("设为对话目标", async () => {
        if (await settingsSave(req("/target", { method: "PUT", body: JSON.stringify({ mode: "single", id: s.identifier }) }), `对话目标 → @${s.identifier}`)) { await refreshContext(); renderSettings(); }
      }, "set-btn"));
      ops.appendChild(mkBtn("切换模型", () => {
        const old = card.querySelector(".model-edit");
        if (old) { old.remove(); return; }
        const e = document.createElement("div");
        e.className = "model-edit";
        const mi = mkInput(s.modelHint ?? "", { placeholder: "档案ID 或 档案ID/模型名（空 = 跟随全局）" });
        e.appendChild(mkField("模型", mi));
        e.appendChild(mkBtn("保存", async () => {
          if (await settingsSave(req(`/singles/${s.identifier}`, { method: "PUT", body: JSON.stringify({ modelHint: mi.value.trim() || null }) }), "模型已切换")) renderSettings();
        }, "set-btn"));
        card.appendChild(e);
      }, "set-btn"));
      ops.appendChild(mkBtn("编辑描述", () => {
        const old = card.querySelector(".desc-edit");
        if (old) { old.remove(); return; }
        const e = document.createElement("div");
        e.className = "desc-edit";
        const di = mkInput(s.description ?? "", { placeholder: "一句话描述" });
        e.appendChild(mkField("描述", di));
        e.appendChild(mkBtn("保存", async () => {
          if (await settingsSave(req(`/singles/${s.identifier}`, { method: "PUT", body: JSON.stringify({ description: di.value.trim() }) }), "描述已更新")) renderSettings();
        }, "set-btn"));
        card.appendChild(e);
      }, "set-btn"));
      ops.appendChild(mkBtn("人格", () => personaCard(card, s.identifier, { kind: "single" }), "set-btn"));
      ops.appendChild(mkBtn("提示词", () => promptCard(card, { kind: "single", identifier: s.identifier }), "set-btn"));
      ops.appendChild(mkBtn("删除", async () => {
        if (!confirm(`删除智能体 @${s.identifier}？`)) return;
        if (await settingsSave(req(`/singles/${s.identifier}`, { method: "DELETE" }), "已删除")) renderSettings();
      }, "set-btn danger"));
      card.appendChild(ops);
      box.appendChild(card);
    }
  };
  render().catch((e) => box.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `加载失败：${e}` })));
  body.appendChild(list);

  const create = setCard("新建智能体");
  const name = mkInput("", { placeholder: "名称，如 写作助手" });
  const id = mkInput("", { placeholder: "标识（如 writer）" });
  const dom = mkInput("", { placeholder: "负责域（可选，如 文案" });
  const desc = mkInput("", { placeholder: "一句话描述（可选）" });
  create.appendChild(mkField("名称", name));
  create.appendChild(mkField("标识", id));
  create.appendChild(mkField("负责域", dom));
  create.appendChild(mkField("描述", desc));
  create.appendChild(mkBtn("创建", async () => {
    if (!name.value.trim() || !id.value.trim()) return toast("名称与标识必填", false);
    const payload = { name: name.value.trim(), identifier: id.value.trim(), description: desc.value.trim() };
    if (dom.value.trim()) payload.domain = dom.value.trim();
    if (await settingsSave(req("/singles", { method: "POST", body: JSON.stringify(payload) }), "智能体已创建")) renderSettings();
  }));
  body.appendChild(create);
}

// 人格（SOUL）编辑卡：组内个体走组作用域，单体走 singles 作用域
function personaCard(container, identifier, { kind, gid }) {
  container.innerHTML = "";
  const c = setCard(`人格（SOUL）· @${identifier}`);
  const ta = document.createElement("textarea");
  ta.className = "pop-input";
  ta.rows = 6;
  const url = kind === "single" ? `/singles/${identifier}/persona` : `/agents/${identifier}/persona`;
  req(url).then((r) => { ta.value = r.persona ?? r.default ?? ""; }).catch(() => {});
  c.appendChild(ta);
  const ops = document.createElement("div");
  ops.className = "set-ops";
  ops.appendChild(mkBtn("保存", async () => {
    if (await settingsSave(req(url, { method: "PUT", body: JSON.stringify({ persona: ta.value }) }), "人格已保存")) container.innerHTML = "";
  }, "set-btn"));
  ops.appendChild(mkBtn("恢复默认", async () => {
    if (await settingsSave(req(url, { method: "DELETE" }), "已恢复默认")) container.innerHTML = "";
  }, "set-btn danger"));
  c.appendChild(ops);
  container.appendChild(c);
}

// 系统提示词编辑卡
function promptCard(container, { kind, gid, identifier }) {
  container.innerHTML = "";
  const c = setCard(`系统提示词 · @${identifier}`);
  const ta = document.createElement("textarea");
  ta.className = "pop-input";
  ta.rows = 6;
  const url = kind === "single" ? `/singles/${identifier}/prompt` : `/groups/${gid}/agents/${identifier}/prompt`;
  req(url).then((r) => { ta.value = r.prompt ?? ""; }).catch(() => {});
  c.appendChild(ta);
  c.appendChild(mkBtn("保存", async () => {
    if (await settingsSave(req(url, { method: "PUT", body: JSON.stringify({ prompt: ta.value }) }), "提示词已保存")) container.innerHTML = "";
  }, "set-btn"));
  c.appendChild(ops_row());
  function ops_row() { const d = document.createElement("div"); d.className = "set-ops"; return d; }
  container.appendChild(c);
}

// —— 通道 ——
function renderSetChannel(body) {
  const list = setCard("消息通道（Telegram / QQ / Discord…）");
  req("/channels").then(async (channels) => {
    const status = await req("/channels/status").catch(() => ({}));
    if (!channels.length) list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "尚未配置通道——新增后智能体组可通过 IM 收发消息。" }));
    for (const ch of channels) {
      const st = status[ch.id];
      const row = document.createElement("div");
      row.className = "set-row";
      row.innerHTML = `<div class="set-row-main"><b>${esc(ch.id)}</b><span class="tag-dim">${esc(ch.type || ch.platform || "")}</span>
        ${ch.enabled ? "" : '<span class="tag-dim">已停用</span>'}
        <div class="set-sub">${st ? `${st.state === "ok" ? "● 正常" : "● 异常"} ${esc(st.detail || "")}` : "无运行状态"}</div></div>`;
      const ops = document.createElement("div");
      ops.className = "set-ops";
      ops.appendChild(mkBtn(ch.enabled ? "停用" : "启用", async () => {
        if (await settingsSave(req(`/channels/${ch.id}`, { method: "PUT", body: JSON.stringify({ enabled: !ch.enabled }) }), ch.enabled ? "已停用" : "已启用")) renderSettings();
      }, "set-btn"));
      ops.appendChild(mkBtn("测试", async () => {
        const r = await req(`/channels/${ch.id}/test`, { method: "POST", body: JSON.stringify({}) }).catch((e) => ({ error: String(e) }));
        toast(r?.ok ? `连通正常${r.message ? `：${r.message}` : ""}` : `失败：${r?.error || r?.message || "未知"}`, Boolean(r?.ok));
      }, "set-btn"));
      ops.appendChild(mkBtn("删除", async () => {
        if (!confirm(`删除通道「${ch.id}」？`)) return;
        if (await settingsSave(req(`/channels/${ch.id}`, { method: "DELETE" }), "已删除")) renderSettings();
      }, "set-btn danger"));
      row.appendChild(ops);
      list.appendChild(row);
    }
    body.appendChild(list);
    body.appendChild(channelCreateForm());
  }).catch((e) => body.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `通道加载失败：${e}` })));

  function channelCreateForm() {
    const c = setCard("新增通道");
    const id = mkInput("", { placeholder: "唯一标识，如 tg-main" });
    const platform = mkSelect([["telegram", "Telegram"], ["qqbot", "QQ 官方机器人"], ["napcat", "NapCat（QQ）"], ["discord", "Discord"], ["slack", "Slack"], ["matrix", "Matrix"]], "telegram");
    c.appendChild(mkField("标识", id));
    c.appendChild(mkField("平台", platform));
    // 平台相关字段动态渲染：qqbot → appId/appSecret；napcat → url/token；其余 → token
    const dyn = document.createElement("div");
    c.appendChild(dyn);
    const rebuild = () => {
      dyn.innerHTML = "";
      const pf = platform.value;
      if (pf === "qqbot") {
        dyn.appendChild(mkField("AppID", mkInput("", { placeholder: "QQ 机器人 AppID" })));
        dyn.appendChild(mkField("AppSecret", mkInput("", { type: "password", placeholder: "AppSecret" })));
      } else if (pf === "napcat") {
        dyn.appendChild(mkField("服务地址", mkInput("", { placeholder: "http://127.0.0.1:3000" })));
        dyn.appendChild(mkField("Token", mkInput("", { type: "password" })));
      } else {
        dyn.appendChild(mkField("Bot Token", mkInput("", { type: "password" })));
      }
    };
    platform.addEventListener("change", rebuild);
    rebuild();
    c.appendChild(mkBtn("创建", async () => {
      if (!id.value.trim()) return toast("通道标识不能为空", false);
      const payload = { id: id.value.trim(), platform: platform.value, enabled: true };
      const inputs = [...dyn.querySelectorAll(".pop-input")];
      if (platform.value === "qqbot") {
        payload.config = { appId: inputs[0]?.value.trim() ?? "", appSecret: inputs[1]?.value.trim() ?? "" };
      } else if (platform.value === "napcat") {
        payload.config = { url: inputs[0]?.value.trim() ?? "", token: inputs[1]?.value.trim() ?? "" };
      } else if (inputs[0]?.value.trim()) {
        payload.token = inputs[0].value.trim();
      }
      if (await settingsSave(req("/channels", { method: "POST", body: JSON.stringify(payload) }), "通道已创建")) renderSettings();
    }));
    return c;
  }
}

// —— 定时任务 ——
function renderSetCron(body) {
  const list = setCard("定时任务");
  req("/cron").then((jobs) => {
    if (!jobs.length) list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "暂无定时任务——按 cron 表达式或单次时间推送提示词给当前组。" }));
    for (const j of jobs) {
      const row = document.createElement("div");
      row.className = "set-row";
      row.innerHTML = `<div class="set-row-main"><b>${esc(j.name)}</b>${j.enabled ? "" : '<span class="tag-dim">已停用</span>'}
        <div class="set-sub">${esc(j.cron ? `cron: ${j.cron}` : `单次: ${j.at ?? ""}`)} · 上次 ${esc(j.lastStatus || "未运行")}</div>
        <div class="set-sub">${esc((j.prompt ?? "").slice(0, 80))}</div></div>`;
      const ops = document.createElement("div");
      ops.className = "set-ops";
      ops.appendChild(mkBtn(j.enabled ? "停用" : "启用", async () => {
        if (await settingsSave(req(`/cron/${j.id}`, { method: "PUT", body: JSON.stringify({ enabled: !j.enabled }) }), j.enabled ? "已停用" : "已启用")) renderSettings();
      }, "set-btn"));
      ops.appendChild(mkBtn("立即执行", async () => {
        if (await settingsSave(req(`/cron/${j.id}/run`, { method: "POST", body: JSON.stringify({}) }), "已触发")) renderSettings();
      }, "set-btn"));
      ops.appendChild(mkBtn("删除", async () => {
        if (!confirm(`删除定时任务「${j.name}」？`)) return;
        if (await settingsSave(req(`/cron/${j.id}`, { method: "DELETE" }), "已删除")) renderSettings();
      }, "set-btn danger"));
      row.appendChild(ops);
      list.appendChild(row);
    }
    body.appendChild(list);
    body.appendChild(cronCreateForm());
  }).catch((e) => body.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `定时任务加载失败：${e}` })));

  function cronCreateForm() {
    const c = setCard("新增定时任务");
    const name = mkInput("", { placeholder: "任务名称，如 每日站会摘要" });
    const prompt = mkInput("", { placeholder: "推送给组的提示词" });
    const cron = mkInput("", { placeholder: "cron 表达式，如 0 9 * * 1-5" });
    const at = mkInput("", { placeholder: "单次执行时间（可选，ISO 格式，填了则忽略 cron）" });
    c.appendChild(mkField("名称", name));
    c.appendChild(mkField("提示词", prompt));
    c.appendChild(mkField("cron 表达式", cron));
    c.appendChild(mkField("单次时间", at));
    c.appendChild(mkBtn("创建", async () => {
      if (!name.value.trim() || !prompt.value.trim()) return toast("名称与提示词必填", false);
      const payload = { name: name.value.trim(), prompt: prompt.value.trim() };
      if (cron.value.trim()) payload.cron = cron.value.trim();
      if (at.value.trim()) payload.at = at.value.trim();
      if (await settingsSave(req("/cron", { method: "POST", body: JSON.stringify(payload) }), "定时任务已创建")) renderSettings();
    }));
    return c;
  }
}

// —— 安全 ——
// —— 记忆 ——
function renderSetMemory(body) {
  const stats = setCard("记忆库");
  const statBox = document.createElement("div");
  stats.appendChild(statBox);
  const search = setCard("语义 / 词项检索");
  const q = mkInput("", { placeholder: "检索记忆…" });
  search.appendChild(mkField("查询", q));
  search.appendChild(mkBtn("检索", () => {
    req("/memory/search", { method: "POST", body: JSON.stringify({ query: q.value.trim(), limit: 8 }) }).then((r) => {
      listBox.innerHTML = "";
      const hits = r.hits ?? [];
      if (!hits.length) listBox.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "无命中" }));
      for (const h of hits) listBox.appendChild(memRow(h.entry, `得分 ${h.score.toFixed(2)}`));
    }).catch((e) => toast(String(e), false));
  }, "set-btn"));
  body.appendChild(search);
  const listBox = setCard("记忆条目");
  body.appendChild(stats);
  body.appendChild(listBox);
  const add = setCard("新增记忆");
  const title = mkInput("", { placeholder: "标题" });
  const bodyI = mkInput("", { placeholder: "内容" });
  const tags = mkInput("", { placeholder: "标签，逗号分隔（可选）" });
  add.appendChild(mkField("标题", title));
  add.appendChild(mkField("内容", bodyI));
  add.appendChild(mkField("标签", tags));
  add.appendChild(mkBtn("添加", async () => {
    if (!title.value.trim() || !bodyI.value.trim()) return toast("标题与内容必填", false);
    const payload = { kind: "fact", title: title.value.trim(), body: bodyI.value.trim() };
    if (tags.value.trim()) payload.tags = tags.value.split(",").map((s) => s.trim()).filter(Boolean);
    if (await settingsSave(req("/memory", { method: "POST", body: JSON.stringify(payload) }), "记忆已添加")) renderSettings();
  }));
  body.appendChild(add);

  req("/memory/stats").then((s) => {
    statBox.innerHTML = `<div class="set-sub">总计 ${s.memory.total} 条 · 置顶 ${s.memory.pinned} · 个体 ${s.memory.individual} / 共享 ${s.memory.shared} · 词项索引 ${s.memory.terms}</div>`;
  }).catch(() => {});
  const load = () => req("/memory?limit=50").then((entries) => {
    listBox.innerHTML = "";
    if (!entries.length) listBox.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "记忆库为空" }));
    for (const e of entries) listBox.appendChild(memRow(e, null, () => load()));
  }).catch((e) => listBox.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `加载失败：${e}` })));
  load();
}

function memRow(e, badge, refresh) {
  const row = document.createElement("div");
  row.className = "set-row";
  row.innerHTML = `<div class="set-row-main">
    <b>${esc(e.title || "未命名")}</b>${e.pinned ? '<span class="tag-ok">📌 置顶</span>' : ""}<span class="tag-dim">${esc(e.kind || "")}</span>
    <div class="set-sub">${esc((e.body || "").slice(0, 120))}</div>
    ${badge ? `<div class="set-sub">${esc(badge)}</div>` : ""}</div>`;
  const ops = document.createElement("div");
  ops.className = "set-ops";
  if (refresh) {
    ops.appendChild(mkBtn(e.pinned ? "取消置顶" : "置顶", async () => {
      if (await settingsSave(req(`/memory/${e.id}/pin`, { method: "POST", body: JSON.stringify({ pinned: !e.pinned }) }), e.pinned ? "已取消置顶" : "已置顶")) refresh();
    }, "set-btn"));
    ops.appendChild(mkBtn("删除", async () => {
      if (await settingsSave(req(`/memory/${e.id}`, { method: "DELETE" }), "已删除")) refresh();
    }, "set-btn danger"));
  }
  row.appendChild(ops);
  return row;
}

// —— 技能 ——
function renderSetSkills(body) {
  const list = setCard("技能（触发词命中后注入指令）");
  req("/skills").then((skills) => {
    if (!skills.length) list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "暂无技能——命中触发词时向个体注入对应指令。" }));
    for (const s of skills) {
      const row = document.createElement("div");
      row.className = "set-row";
      row.innerHTML = `<div class="set-row-main"><b>${esc(s.name)}</b><span class="tag-dim">${esc(s.id)}</span>
        <div class="set-sub">${esc(s.description || "")}</div>
        <div class="set-sub">触发：${esc((s.triggers ?? []).join("、") || "—")}</div></div>`;
      const ops = document.createElement("div");
      ops.className = "set-ops";
      ops.appendChild(mkBtn("删除", async () => {
        if (!confirm(`删除技能「${s.name}」？`)) return;
        if (await settingsSave(req(`/skills/${s.id}`, { method: "DELETE" }), "已删除")) renderSettings();
      }, "set-btn danger"));
      row.appendChild(ops);
      list.appendChild(row);
    }
    body.appendChild(list);
    const c = setCard("新增技能");
    const id = mkInput("", { placeholder: "唯一 id，如 deploy-check" });
    const name = mkInput("", { placeholder: "技能名称" });
    const desc = mkInput("", { placeholder: "描述（可选）" });
    const triggers = mkInput("", { placeholder: "触发词，逗号分隔" });
    const instructions = document.createElement("textarea");
    instructions.className = "pop-input"; instructions.rows = 4;
    c.appendChild(mkField("ID", id));
    c.appendChild(mkField("名称", name));
    c.appendChild(mkField("描述", desc));
    c.appendChild(mkField("触发词", triggers));
    c.appendChild(mkField("指令内容", instructions));
    c.appendChild(mkBtn("创建", async () => {
      if (!id.value.trim() || !name.value.trim() || !instructions.value.trim()) return toast("ID / 名称 / 指令必填", false);
      const payload = { id: id.value.trim(), name: name.value.trim(), description: desc.value.trim(), instructions: instructions.value, triggers: triggers.value.split(",").map((s) => s.trim()).filter(Boolean) };
      if (await settingsSave(req("/skills", { method: "POST", body: JSON.stringify(payload) }), "技能已创建")) renderSettings();
    }));
    body.appendChild(c);
  }).catch((e) => body.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `技能加载失败：${e}` })));
}

// —— 审计 ——
function renderSetAudit(body) {
  const list = setCard("工具审计（最近 50 条）");
  req("/audit?limit=50").then(({ items }) => {
    if (!items.length) list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "暂无审计记录" }));
    for (const it of items) {
      const row = document.createElement("div");
      row.className = "set-row";
      const ok = Boolean(it.ok);
      row.innerHTML = `<div class="set-row-main">
        <span class="tool-name">${esc(String(it.tool ?? ""))}</span>
        <span class="tool-status"><span class="${ok ? "ok" : "err"}">${ok ? "✓" : "✗"} ${(Number(it.duration_ms ?? it.durationMs ?? 0) / 1000).toFixed(1)}s</span></span>
        <div class="set-sub">@${esc(String(it.agent_id ?? it.agentId ?? ""))} · ${esc(String(it.summary ?? "").slice(0, 100))}</div></div>`;
      list.appendChild(row);
    }
    body.appendChild(list);
  }).catch((e) => body.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `审计加载失败：${e}` })));
}

// —— 配置（schema 驱动：安全/自动化/沙箱/Computer Use/搜索/记忆参数/钩子/限流 全覆盖）——
const CONFIG_NESTED = ["memory", "security", "automation", "search", "sandbox", "browser", "hooks", "tools"];
const CONFIG_TEXTAREA = ["automation.heartbeatPrompt", "security.execAllowlist", "tools.custom"];

function renderSetConfig(body) {
  const wrap = setCard("通用配置");
  wrap.style.display = "flex";
  wrap.style.gap = "14px";
  wrap.style.alignItems = "flex-start";
  const nav = document.createElement("div");
  nav.className = "set-subnav";
  const panel = document.createElement("div");
  panel.style.flex = "1";
  panel.style.minWidth = "0";
  wrap.appendChild(nav);
  wrap.appendChild(panel);
  body.appendChild(wrap);

  Promise.all([req("/config/schema"), req("/config")]).then(([schema, cfg]) => {
    const groups = schema.groups ?? [];
    const values = {};
    for (const g of groups) for (const f of g.fields) {
      const dot = f.key.indexOf(".");
      values[f.key] = dot < 0 ? cfg[f.key] : (cfg[f.key.split(".")[0]] ?? {})[f.key.split(".")[1]];
    }
    let current = groups[0]?.key ?? "";
    const setField = (k, v) => { values[k] = v; };
    const renderPanel = () => {
      const g = groups.find((x) => x.key === current);
      panel.innerHTML = "";
      if (!g) return;
      const card = setCard(g.label);
      for (const f of g.fields) {
        const v = values[f.key];
        let ctrl;
        if (f.kind === "boolean") {
          ctrl = mkSelect([["true", "开"], ["false", "关"]], String(v === undefined ? f.default === "true" : Boolean(v)));
        } else if (CONFIG_TEXTAREA.includes(f.key)) {
          ctrl = document.createElement("textarea");
          ctrl.className = "pop-input"; ctrl.rows = 3;
          ctrl.value = String(v ?? f.default ?? "");
        } else {
          ctrl = mkInput(v ?? f.default ?? "", { type: f.kind === "password" ? "password" : f.kind === "number" ? "number" : "text" });
        }
        ctrl.dataset.key = f.key;
        ctrl.addEventListener("input", () => setField(f.key, ctrl.value));
        ctrl.addEventListener("change", () => setField(f.key, ctrl.value));
        card.appendChild(mkField(f.label + (f.help ? `（${f.help}）` : ""), ctrl));
      }
      card.appendChild(mkBtn("保存本组", async () => {
        const body2 = {};
        for (const [k, v] of Object.entries(values)) {
          if (v === undefined) continue;
          const dot = k.indexOf(".");
          if (dot < 0) { if (v !== "") body2[k] = v; continue; }
          const gg = k.slice(0, dot), ff = k.slice(dot + 1);
          if (!CONFIG_NESTED.includes(gg)) { if (v !== "") body2[k] = v; continue; }
          body2[gg] = { ...(body2[gg] ?? {}), [ff]: v };
        }
        if (await settingsSave(req("/config", { method: "PUT", body: JSON.stringify(body2) }), "配置已保存（即时生效）")) renderSettings();
      }));
      panel.appendChild(card);
    };
    const renderNav = () => {
      nav.innerHTML = "";
      for (const g of groups) {
        const b = document.createElement("button");
        b.className = `set-subnav-item${g.key === current ? " on" : ""}`;
        b.textContent = g.label;
        b.addEventListener("click", () => { current = g.key; renderNav(); renderPanel(); });
        nav.appendChild(b);
      }
    };
    renderNav();
    renderPanel();
  }).catch((e) => body.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `配置加载失败：${e}` })));
}
// 工作目录 / 智能体·智能体组 / 模型 / 思考强度：底部工具条芯片 + 弹出菜单；
// 附件：最左下角「＋」（图片，多模态 data URL 直传）

const basename = (p) => String(p ?? "").replace(/[\\/]+$/, "").split(/[\\/]/).pop();

function groupName(gid) {
  return S.groups.find((g) => g.id === gid)?.name ?? (gid === "default" ? "默认组" : (gid ?? "默认组"));
}

function currentModelValue() {
  if (S.target?.mode === "single") {
    return S.singles.find((s) => s.identifier === S.target.id)?.modelHint ?? "";
  }
  return S.groups.find((g) => g.id === (S.target?.id ?? "default"))?.model ?? "";
}

function currentWorkspace() {
  return S.workspace;
}


function renderChips() {
  const ws = currentWorkspace();
  $("chip-ws").innerHTML = `📂 ${ws ? `<b>${esc(basename(ws))}</b>` : "不使用工作空间"}`;
  const t = S.target;
  $("chip-target").innerHTML = t?.mode === "single"
    ? `<span class="chip-label">智能体</span>@${esc(t.name || t.id)}`
    : `<span class="chip-label">组</span>${esc(groupName(t?.id ?? "default"))}`;
  // 生效模型：组/个体显式指定 > 生效档案的默认模型
  const shown = activeModelName();
  $("chip-model").innerHTML = `<span class="chip-label">模型</span><b>${esc(shown)}</b><span class="chip-effort">${esc(EFFORT_DISPLAY[S.effort] ?? "Default")}</span>`;
}

// —— 弹出菜单骨架：统一开合、点外即收 ——
function closePopover() { $("popover").classList.add("hidden"); }

function openPopover(build) {
  const pop = $("popover");
  pop.innerHTML = "";
  build(pop);
  pop.classList.remove("hidden");
}

function popItem({ title, sub, current, onclick }) {
  const b = document.createElement("button");
  b.className = `pop-item${current ? " current" : ""}`;
  b.innerHTML = `<span style="min-width:0;overflow:hidden"><span>${esc(title)}</span>${sub ? `<div class="sub">${esc(sub)}</div>` : ""}</span><span class="mark">${current ? "✓" : ""}</span>`;
  // stopPropagation：子菜单切换会重建浮窗内容，原 target 脱离容器后
  // 冒泡到 document 的关闭监听会被误判为「点在外面」而直接关浮窗
  b.addEventListener("click", (e) => { e.stopPropagation(); sfx.play("click"); onclick(e); });
  return b;
}

function popTitle(text) {
  const d = document.createElement("div");
  d.className = "pop-title";
  d.textContent = text;
  return d;
}

async function switchTarget(mode, id) {
  closePopover();
  try {
    await api.setTarget(mode, id);
    await refreshContext();
    // 目标即交互上下文：切换后回到该上下文的最新会话（与 WebUI 同口径）
    S.sessions = await api.sessions();
    renderSidebar();
    if (S.sessions.length) await selectSession(S.sessions[0].id);
    else await newSession();
  } catch (e) { alertErr(e); }
}

async function refreshContext() {
  try {
    const [target, groups, singles, profiles, ws] = await Promise.all([api.target(), api.groups(), api.singles(), api.llmProfiles(), api.getWorkspace()]);
    S.target = target;
    S.groups = groups.groups ?? [];
    S.singles = singles.singles ?? [];
    S.profiles = profiles.profiles ?? [];
    S.activeProfileId = profiles.active ?? null;
    S.workspace = String(ws?.workspace ?? "");
    const name = S.target?.mode === "single" ? `智能体 · @${S.target.name || S.target.id}` : `组 · ${groupName(S.target?.id ?? "default")}`;
    $("target-chip").innerHTML = `对话对象：<b>${esc(name)}</b>`;
    renderChips();
  } catch { /* 上下文拉取失败不阻塞对话 */ }
}

function wsMenu(pop) {
  // 图一口径：最近工作空间 / 打开本地文件夹 / 不使用工作空间
  let recents = [];
  try { recents = JSON.parse(localStorage.getItem("exm.wsRecent") ?? "[]"); } catch { recents = []; }
  recents = recents.filter((p) => p && p !== currentWorkspace());

  if (currentWorkspace()) {
    pop.appendChild(popItem({ title: `📂 ${basename(currentWorkspace())}`, sub: currentWorkspace(), current: true, onclick: closePopover }));
  }
  for (const p of recents) {
    pop.appendChild(popItem({ title: `📂 ${basename(p)}`, sub: p, onclick: async () => { closePopover(); await applyWorkspace(p); } }));
  }
  // 打开本地文件夹：桌面端原生目录选择器（浏览器调试回退为手输）
  pop.appendChild(popItem({
    title: "打开本地文件夹",
    onclick: async () => {
      try {
        const picked = await window.__TAURI__?.dialog?.open({ directory: true, multiple: false });
        const path = Array.isArray(picked) ? picked[0] : picked;
        if (path) { closePopover(); await applyWorkspace(String(path)); }
        else closePopover();
      } catch {
        // 非桌面环境：回退为手输路径
        pop.querySelectorAll(".pop-input,.pop-save").forEach((e) => e.remove());
        manualInput(pop);
      }
    },
  }));
  pop.appendChild(popItem({ title: "不使用工作空间", sub: "回退全局数据目录", onclick: async () => { closePopover(); await applyWorkspace(""); } }));

  function manualInput(container) {
    const input = document.createElement("input");
    input.className = "pop-input";
    input.placeholder = "/path/to/project（目录须已存在）";
    container.appendChild(input);
    const btn = document.createElement("button");
    btn.className = "pop-save";
    btn.textContent = "保存工作目录";
    btn.addEventListener("click", () => applyWorkspace(input.value.trim()));
    container.appendChild(btn);
  }
}

function rememberWorkspace(path) {
  try {
    const list = JSON.parse(localStorage.getItem("exm.wsRecent") ?? "[]").filter((p) => p !== path);
    list.unshift(path);
    localStorage.setItem("exm.wsRecent", JSON.stringify(list.slice(0, 5)));
  } catch { /* 忽略 */ }
}

async function applyWorkspace(path) {
  if (!confirmDiscardDirty("切换工作目录")) return; // 编码页有未保存修改时先确认
  try {
    const r = await api.setWorkspace(path);
    S.workspace = String(r?.workspace ?? "");
    // 工作目录变了：编码页的树与打开文件整体失效，清空待重载
    S.code.tree = { "": { open: true, loaded: false, entries: [] } };
    S.code.file = null;
    if (path) rememberWorkspace(path);
    closePopover();
    renderChips();
  } catch (e) { alertErr(e); }
}

function targetMenu(pop) {
  pop.appendChild(popTitle("智能体组"));
  for (const g of S.groups) {
    const active = S.target?.mode === "group" && (S.target.id ?? "default") === g.id;
    pop.appendChild(popItem({
      title: g.name, sub: g.workspace ? `组目录：${g.workspace}` : (g.description || "使用全局工作目录"), current: active,
      onclick: () => switchTarget("group", g.id),
    }));
  }
  if (S.singles.length) {
    pop.appendChild(popTitle("智能体"));
    for (const s of S.singles) {
      const active = S.target?.mode === "single" && S.target.id === s.identifier;
      pop.appendChild(popItem({
        title: `@${s.name}`, sub: s.description || s.domain, current: active,
        onclick: () => switchTarget("single", s.identifier),
      }));
    }
  }
}

// 推理等级展示名（Default/Low/High/Max）与后端档位（default/low/medium/high）的映射
const EFFORT_DISPLAY = { default: "Default", low: "Low", medium: "High", high: "Max" };
const EFFORT_ORDER = ["default", "low", "medium", "high"];

function modelMenu(pop) {
  // 图二口径：两行式入口（当前值 + ›），点击进入各自列表
  pop.appendChild(popRow("模型", activeModelName(), () => openModelList(pop)));
  pop.appendChild(popRow("推理等级", EFFORT_DISPLAY[S.effort] ?? "Default", () => openEffortList(pop)));
}

function popRow(label, value, onclick) {
  const b = document.createElement("button");
  b.className = "pop-row";
  b.innerHTML = `<span>${esc(label)}</span><span class="val">${esc(value)}</span><span class="chev">›</span>`;
  b.addEventListener("click", (e) => { e.stopPropagation(); onclick(e); });
  return b;
}

// 图三：搜索 + 按供应商（档案）分组的模型清单
function openModelList(pop) {
  pop.innerHTML = "";
  const search = document.createElement("input");
  search.className = "pop-input";
  search.placeholder = "搜索模型...";
  pop.appendChild(search);
  const list = document.createElement("div");
  pop.appendChild(list);

  const render = (q) => {
    list.innerHTML = "";
    const kw = (q ?? "").trim().toLowerCase();
    const match = (s) => !kw || String(s ?? "").toLowerCase().includes(kw);
    // 跟随生效档案（清除组/个体覆盖）
    if (match("跟随生效档案")) {
      list.appendChild(popItem({
        title: "跟随生效档案", current: !currentModelValue(),
        onclick: async () => { closePopover(); await setModel(""); },
      }));
    }
    for (const p of S.profiles) {
      const rows = (p.models ?? []).length
        ? (p.models ?? []).map((m) => ({ title: m.model, value: `${p.id}/${m.model}`, current: currentModelValue() === `${p.id}/${m.model}` }))
        : [{ title: p.model || `${p.name}（默认）`, value: p.id, current: currentModelValue() === p.id }];
      const hit = rows.filter((r) => match(r.title) || match(p.name));
      if (!hit.length) continue;
      const head = document.createElement("div");
      head.className = "pop-title";
      head.textContent = p.name;
      list.appendChild(head);
      for (const r of hit) {
        list.appendChild(popItem({ title: r.title, current: r.current, onclick: async () => { closePopover(); await setModel(r.value); } }));
      }
    }
  };
  render("");
  search.addEventListener("input", () => render(search.value));
  search.addEventListener("keydown", (e) => e.stopPropagation());
  setTimeout(() => search.focus(), 0);
}

// 图四：推理等级纯列表（Default/Low/High/Max）
function openEffortList(pop) {
  pop.innerHTML = "";
  for (const v of EFFORT_ORDER) {
    pop.appendChild(popItem({
      title: EFFORT_DISPLAY[v], current: S.effort === v,
      onclick: () => {
        S.effort = v;
        try { localStorage.setItem("exm.effort", v); } catch { /* 忽略 */ }
        api.setEffort(v === "default" ? "" : v).catch(alertErr);
        closePopover();
        renderChips();
      },
    }));
  }
}

// 当前实际生效的模型名（组/个体覆盖 > 生效档案默认）
function activeModelName() {
  const mv = currentModelValue();
  if (mv) return mv.includes("/") ? mv.split("/").pop() : (S.profiles.find((p) => p.id === mv)?.name ?? mv);
  const p = S.profiles.find((x) => x.id === S.activeProfileId);
  return p ? (p.model || p.name) : "默认";
}

async function setModel(value) {
  try {
    if (S.target?.mode === "single") await api.setSingleModel(S.target.id, value || null);
    else await api.setGroupModel(S.target?.id ?? "default", value);
    if (S.target?.mode === "single") {
      const s = S.singles.find((x) => x.identifier === S.target.id);
      if (s) s.modelHint = value || null;
    } else {
      const g = S.groups.find((x) => x.id === (S.target?.id ?? "default"));
      if (g) g.model = value || null;
    }
    renderChips();
  } catch (e) { alertErr(e); }
}

// —— 附件（图片，data URL 随本轮进入规划）——
function renderAttachRow() {
  const row = $("attach-row");
  row.innerHTML = "";
  row.classList.toggle("hidden", !S.images.length);
  S.images.forEach((url, i) => {
    const t = document.createElement("div");
    t.className = "attach-thumb";
    t.innerHTML = `<img src="${url}" alt="附件${i + 1}"/><button class="rm" title="移除">✕</button>`;
    t.querySelector(".rm").addEventListener("click", () => { S.images.splice(i, 1); renderAttachRow(); });
    row.appendChild(t);
  });
}

async function addFiles(files) {
  for (const f of files) {
    if (!f.type.startsWith("image/")) continue;
    if (S.images.length >= 6) break; // 上限：6 张（多模态输入合理边界）
    const url = await new Promise((ok, err) => {
      const r = new FileReader();
      r.onload = () => ok(String(r.result));
      r.onerror = err;
      r.readAsDataURL(f);
    });
    S.images.push(url);
  }
  renderAttachRow();
}

// ────────────────────────────── 审批：侧栏角标 + 设置审批页 ──────────────────────────────
// 口径：审批单跨会话存在（通道发起的不在当前会话流里）——角标靠轮询 + WS 事件即时刷新

async function refreshApprovalsBadge() {
  // 序号防乱序：required/resolved 连发时，过期响应不回写角标（初值 0，避免 NaN 恒不等）
  const seq = (refreshApprovalsBadge.seq = (refreshApprovalsBadge.seq ?? 0) + 1);
  try {
    const items = await api.approvals("pending", 50);
    if (seq !== refreshApprovalsBadge.seq) return;
    S.approvalsPending = Array.isArray(items) ? items.length : 0;
  } catch {
    if (seq !== refreshApprovalsBadge.seq) return;
    S.approvalsPending = 0;
  }
  const b = $("approvals-badge");
  b.textContent = S.approvalsPending > 50 ? "50+" : String(S.approvalsPending);
  b.classList.toggle("hidden", !S.approvalsPending);
}

// 设置 → 审批：该 tab 此前在映射里被引用但从未实现（点击即 ReferenceError），这里补齐
function renderSetApprovals(body) {
  const card = setCard("审批单（破坏性操作的放行闸口）");
  const filter = mkSelect([
    ["pending", "待处理"], ["approved", "已批准"], ["denied", "已拒绝"],
    ["executed", "已执行"], ["failed", "执行失败"], ["", "全部状态"],
  ], "pending");
  const list = document.createElement("div");
  const load = () => {
    list.innerHTML = `<div class="set-hint">加载中…</div>`;
    api.approvals(filter.value || null, 100).then((items) => {
      list.innerHTML = "";
      if (!Array.isArray(items) || !items.length) {
        list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "没有符合条件的审批单" }));
        return;
      }
      for (const it of items) list.appendChild(approvalRow(it, load));
    }).catch((e) => {
      list.innerHTML = "";
      list.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `加载失败：${e}` }));
    });
  };
  filter.addEventListener("change", load);
  card.appendChild(mkField("状态筛选", filter));
  card.appendChild(list);
  body.appendChild(card);
  load();
}

// 审批单 → 所属会话：sessionId 完整则直用；疑似截断 id（列表无精确命中）做前缀匹配，
// 仅唯一命中才返回（多命中/零命中置灰，避免跳错会话）
function sessionJumpTarget(sessionId) {
  const sid = String(sessionId ?? "").trim();
  if (!sid) return null;
  if (S.sessions.some((s) => s.id === sid)) return sid;
  const hits = S.sessions.filter((s) => String(s.id).startsWith(sid));
  return hits.length === 1 ? hits[0].id : null;
}

function approvalRow(it, refresh) {
  const row = document.createElement("div");
  row.className = "set-row";
  const stCls = { pending: "st-blocked", approved: "st-done", executed: "st-done", denied: "st-failed", failed: "st-failed" }[it.status] ?? "";
  row.innerHTML = `<div class="set-row-main">
    <span class="approval-cmd">${esc(it.command || "（无命令内容）")}</span>
    <div class="set-sub">@${esc(it.agentId || "?")} · ${esc(relTime(it.createdAt))} · 会话 ${esc(String(it.sessionId ?? "").slice(0, 10))}</div>
    ${it.result ? `<div class="set-sub">结果：${esc(String(it.result).slice(0, 160))}</div>` : ""}
  </div><span class="task-status ${stCls}">${esc(it.status ?? "")}</span>`;
  const ops = document.createElement("div");
  ops.className = "set-ops";
  // 跳转会话：切回对话页并选中审批来源会话；定位不到则置灰说明原因
  const jump = mkBtn("跳转", async () => {
    const target = sessionJumpTarget(it.sessionId); // 点击时现算，避免列表渲染后过期
    if (!target) return;
    showView("chat");
    await selectSession(target);
  }, "set-btn");
  if (!sessionJumpTarget(it.sessionId)) {
    jump.disabled = true;
    jump.title = "会话不在当前列表（可能已归档或 id 不足以定位）";
  }
  ops.appendChild(jump);
  if (it.status === "pending") {
    ops.appendChild(mkBtn("批准", async () => {
      if (await settingsSave(api.decideApproval(it.id, true), "已批准，由系统代执行")) { refresh(); refreshApprovalsBadge(); }
    }, "set-btn"));
    ops.appendChild(mkBtn("拒绝", async () => {
      if (await settingsSave(api.decideApproval(it.id, false), "已拒绝")) { refresh(); refreshApprovalsBadge(); }
    }, "set-btn danger"));
  }
  row.appendChild(ops);
  return row;
}

// —— 版本：本机版本 + 更新检查 + 一键更新（映射里被引用但此前从未实现，这里补齐）——
function renderSetVersion(body) {
  const card = setCard("版本与更新");
  const info = document.createElement("div");
  info.className = "set-hint";
  info.textContent = "读取中…";
  card.appendChild(info);
  const detail = document.createElement("div");
  card.appendChild(detail);
  const ops = document.createElement("div");
  ops.className = "set-ops";
  card.appendChild(ops);
  body.appendChild(card);

  // 本机版本（version + git 短哈希）
  const renderInfo = (v) => {
    info.innerHTML = `本机版本 <b>${esc(v.version ?? "?")}</b> · commit <b>${esc(v.gitHash ?? "dev")}</b>${v.branch ? ` · 分支 <b>${esc(v.branch)}</b>` : ""}`;
  };
  req("/version").then(renderInfo).catch(() => { info.textContent = "版本信息读取失败"; });

  // 更新检查：分支感知（本地在什么分支就对比该分支上游）
  const check = async () => {
    detail.innerHTML = `<div class="set-hint">检查更新中…（需网络，约数秒）</div>`;
    let v;
    try { v = await req("/version/check"); }
    catch (e) { detail.innerHTML = ""; detail.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: `检查失败：${e}` })); return; }
    renderInfo(v);
    detail.innerHTML = "";
    if (v.note) detail.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: String(v.note) }));
    const up = v.upstream;
    if (up) {
      detail.insertAdjacentHTML("beforeend", `<div class="set-sub">上游 ${esc(up.ref ?? "")} @ ${esc(up.short ?? "")}${up.date ? ` · ${esc(String(up.date).slice(0, 10))}` : ""}${up.url ? ` · <a href="${esc(up.url)}" target="_blank" rel="noopener">查看提交</a>` : ""}</div>`);
    }
    const avail = v.updateAvailable === true;
    if (v.updateAvailable !== undefined && v.updateAvailable !== null) {
      detail.insertAdjacentHTML("beforeend", `<div class="set-sub">${avail ? '<span class="tag-ok">有可用更新</span>' : "已是最新"}</div>`);
    }
    ops.innerHTML = "";
    ops.appendChild(mkBtn("重新检查", check, "set-btn"));
    if (avail) {
      // 一键更新：容器部署（/host 挂载）才可用；桌面壳场景服务端会拒绝并说明
      ops.appendChild(mkBtn("一键更新", async () => {
        if (!confirm("更新会拉取上游并重建服务（git pull → build → up），期间网关可能短暂不可用。继续？")) return;
        try {
          await req("/version/apply", { method: "POST", body: JSON.stringify({}) });
          toast("更新任务已启动，进度见下方日志");
          pollApply();
        } catch (e) { toast(String(e).slice(0, 200), false); }
      }, "set-btn"));
    }
  };
  ops.appendChild(mkBtn("检查更新", check, "set-btn"));

  // 一键更新进度：轮询 status（running + 日志尾部），任务结束即停
  let pollTimer = null;
  const logBox = document.createElement("pre");
  logBox.className = "cp-diff";
  logBox.style.maxHeight = "220px";
  const pollApply = () => {
    if (pollTimer) return;
    if (!logBox.isConnected) body.appendChild(logBox);
    pollTimer = setInterval(async () => {
      try {
        const st = await req("/version/apply/status");
        logBox.textContent = `更新日志（${st.running ? "进行中" : "已结束"}）\n${st.log ?? "（暂无输出）"}`;
        if (!st.running) { clearInterval(pollTimer); pollTimer = null; toast("更新任务已结束（详情见日志）"); }
      } catch { /* 网关重建期间拉不到状态：下轮再试 */ }
    }, 2000);
  };
}

// ────────────────────────────── 任务视图（整页）：任务图 + 台账 ──────────────────────────────
// 数据源：GET /sessions/:id/graph + GET /sessions/:id（ledger）+ WS graph.updated 推送刷新

const TASK_ST_COLOR = {
  pending: "var(--text-faint)", ready: "var(--text-faint)", dispatched: "var(--accent-2)",
  running: "var(--accent-2)", syncing: "var(--accent-2)", arbitrating: "var(--warn)",
  done: "var(--ok)", blocked: "var(--warn)", failed: "var(--err)", cancelled: "var(--text-faint)",
};
const TASK_LEGEND = [["pending", "待派发"], ["running", "执行中"], ["done", "完成"], ["blocked", "受阻"], ["failed", "失败"]];

function enterTasks() {
  $("tp-body").innerHTML = `<div class="loading-hint"><span class="mark-spin mark-accent">${brandMark("", 22)}</span><span>加载任务图与台账…</span></div>`;
  refreshTasks();
}

async function refreshTasks() {
  const id = S.sessionId;
  if (!id) return;
  const [graph, session] = await Promise.all([api.graph(id).catch(() => null), api.sessionDetail(id).catch(() => null)]);
  // 会话或视图已切走：丢弃过期响应，避免串渲染
  if (S.view !== "tasks" || S.sessionId !== id) return;
  if (graph?.nodes?.length) S.graph = graph;
  renderTasksPage(graph, session);
}

function renderTasksPage(graph, session) {
  const body = $("tp-body");
  body.innerHTML = "";
  const nodes = graph?.nodes ?? [];
  // —— 任务图：SVG 连线 DAG（依赖深度分层 + 状态色贝塞尔连线），点节点直聊子代理 ——
  const graphCard = setCard(`任务图 · ${nodes.length ? `${nodes.filter((n) => n.status === "done").length}/${nodes.length} 完成` : "空"}`);
  graphCard.appendChild(Object.assign(document.createElement("div"), {
    className: "tk-legend",
    innerHTML: TASK_LEGEND.map(([k, t]) => `<span><span class="tk-dot" style="background:${TASK_ST_COLOR[k] ?? "var(--text-faint)"}"></span>${t}</span>`).join(""),
  }));
  if (!nodes.length) {
    graphCard.appendChild(Object.assign(document.createElement("div"), {
      className: "set-hint",
      textContent: "本会话还没有任务图——指挥体把目标拆解派发给子个体后，节点与状态会在这里实时呈现。",
    }));
  } else {
    renderTaskDag(graphCard, nodes);
  }
  body.appendChild(graphCard);
  // —— 任务台账：goal / 验收 / 证据 / 风险 分区渲染 ——
  body.appendChild(ledgerCard(session));
}

// —— SVG 连线 DAG ——
// 布局：节点按依赖深度分列（orderedNodes 给出 depth），同层纵向堆叠；
// 连线：依赖节点右缘 → 依赖者左缘的三次贝塞尔，颜色取依赖节点状态色。
const DAG_NODE_W = 252;  // 节点卡宽（.tk-dnode 同步）
const DAG_GAP_X = 46;    // 层间距
const DAG_GAP_Y = 16;    // 同层节点纵间距
const DAG_ZOOM_MIN = 0.5;
const DAG_ZOOM_MAX = 2;
let dagZoom = 1;         // 视图缩放倍率（模块级：graph.updated 推送重渲染时保持当前倍率）

function renderTaskDag(container, nodes) {
  const items = orderedNodes(nodes);
  const byId = new Map(nodes.map((n) => [n.id, n]));
  // 分层（depth 连续自 0 起；forEach 天然跳过稀疏空洞）
  const layers = [];
  for (const it of items) (layers[it.depth] ??= []).push(it);
  const wrap = document.createElement("div");
  wrap.className = "tk-dag-wrap";
  // scaleBox 撑出与缩放后内容等大的布局盒（transform 不参与布局，滚动范围靠它兜底）
  const scaleBox = document.createElement("div");
  scaleBox.className = "tk-dag-scale";
  const dag = document.createElement("div");
  dag.className = "tk-dag";
  scaleBox.appendChild(dag);
  wrap.appendChild(scaleBox);
  // 缩放工具条：实时倍率 + 重置视图（回到 1 倍与左上原点）
  const bar = document.createElement("div");
  bar.className = "tk-zoom-bar";
  const zoomVal = document.createElement("span");
  zoomVal.className = "tk-zoom-val";
  bar.appendChild(zoomVal);
  bar.appendChild(mkBtn("重置视图", () => {
    dagZoom = 1;
    applyDagZoom(scaleBox, dag, zoomVal);
    wrap.scrollTo({ left: 0, top: 0 });
  }, "set-btn"));
  container.appendChild(bar);
  container.appendChild(wrap);
  // 先挂载再量高：卡片高度随内容（objective / 验收条数）自适应
  const els = new Map();
  const pos = new Map();
  for (const it of items) {
    const el = dagNodeEl(it, nodes);
    dag.appendChild(el);
    els.set(it.n.id, el);
  }
  const maxDepth = Math.max(0, ...items.map((i) => i.depth));
  const width = (maxDepth + 1) * DAG_NODE_W + maxDepth * DAG_GAP_X;
  let height = 0;
  layers.forEach((list, d) => {
    let y = 0;
    for (const it of list) {
      const h = els.get(it.n.id).offsetHeight;
      pos.set(it.n.id, { x: d * (DAG_NODE_W + DAG_GAP_X), y, h });
      y += h + DAG_GAP_Y;
    }
    height = Math.max(height, y - DAG_GAP_Y);
  });
  for (const [id, p] of pos) {
    const el = els.get(id);
    el.style.left = `${p.x}px`;
    el.style.top = `${p.y}px`;
  }
  height = Math.max(height, 0);
  dag.style.width = `${width}px`;
  dag.style.height = `${height}px`;
  dag.insertBefore(dagEdgesSvg(nodes, byId, pos, width, height), dag.firstChild);
  // 节点级联浮现（封顶 10 档）：图更新时保持「图是活的」感知
  staggerIn(dag.querySelectorAll(".tk-dnode"), { step: 45, cap: 10 });
  applyDagZoom(scaleBox, dag, zoomVal);
  // Ctrl+滚轮缩放（0.5~2 倍）：只拦带 Ctrl 的滚轮，普通滚动 / 触控板平移不受影响
  wrap.addEventListener("wheel", (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    dagZoom = Math.min(DAG_ZOOM_MAX, Math.max(DAG_ZOOM_MIN, dagZoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1)));
    applyDagZoom(scaleBox, dag, zoomVal);
  }, { passive: false });
}

// 缩放呈现：dag 层 transform scale（节点布局仍在原坐标系量取），scaleBox 按倍率撑出滚动范围
function applyDagZoom(scaleBox, dag, zoomVal) {
  const w = parseFloat(dag.style.width) || dag.offsetWidth;
  const h = parseFloat(dag.style.height) || dag.offsetHeight;
  dag.style.transform = `scale(${dagZoom})`;
  scaleBox.style.width = `${Math.round(w * dagZoom)}px`;
  scaleBox.style.height = `${Math.round(h * dagZoom)}px`;
  if (zoomVal) zoomVal.textContent = `${Math.round(dagZoom * 100)}%`;
}

// 连线层：依赖 → 依赖者，三次贝塞尔（水平进出、垂直过渡），状态色 + 悬停提示
function dagEdgesSvg(nodes, byId, pos, width, height) {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("class", "tk-dag-edges");
  svg.setAttribute("width", String(Math.max(width, 1)));
  svg.setAttribute("height", String(Math.max(height, 1)));
  for (const n of nodes) {
    const tp = pos.get(n.id);
    if (!tp) continue;
    for (const dep of n.dependsOn ?? []) {
      const sp = pos.get(dep);
      const src = byId.get(dep);
      if (!sp || !src) continue; // 缺失依赖（被撤销的轮次等）安全跳过
      const x1 = sp.x + DAG_NODE_W, y1 = sp.y + sp.h / 2;
      const x2 = tp.x, y2 = tp.y + tp.h / 2;
      const bend = Math.max(24, (x2 - x1) / 2);
      const path = document.createElementNS(NS, "path");
      path.setAttribute("d", `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`);
      path.setAttribute("fill", "none");
      path.setAttribute("stroke", TASK_ST_COLOR[String(src.status ?? "pending")] ?? "var(--text-faint)");
      path.setAttribute("stroke-width", "1.5");
      path.setAttribute("opacity", "0.72");
      path.setAttribute("class", "tk-edge");
      const tip = document.createElementNS(NS, "title");
      tip.textContent = `${src.title || dep} → ${n.title || n.objective || n.id}`;
      path.appendChild(tip);
      svg.appendChild(path);
    }
  }
  return svg;
}

// 依赖深度（拓扑层级）：依赖者比被依赖者深一层；环路与缺失依赖安全兜底
function orderedNodes(nodes) {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const memo = new Map();
  const depthOf = (nid, stack = new Set()) => {
    if (memo.has(nid)) return memo.get(nid);
    if (stack.has(nid)) return 0; // 环：不再下钻
    stack.add(nid);
    const n = byId.get(nid);
    const d = (n?.dependsOn ?? []).filter((x) => byId.has(x)).reduce((m, x) => Math.max(m, depthOf(x, stack) + 1), 0);
    stack.delete(nid);
    memo.set(nid, d);
    return d;
  };
  return nodes
    .map((n, i) => ({ n, i, depth: depthOf(n.id) }))
    .sort((a, b) => (a.depth - b.depth) || (a.i - b.i)); // 浅层在前，同层保持原始顺序
}

function dagNodeEl({ n }, nodes) {
  const byId = new Map(nodes.map((x) => [x.id, x]));
  const el = document.createElement("div");
  el.className = `tk-node tk-dnode st-${esc(n.status ?? "pending")}`;
  if (n.agentIdentifier) el.dataset.agent = n.agentIdentifier;
  const deps = (n.dependsOn ?? []).map((d) => byId.get(d)?.title ?? d);
  el.innerHTML = `
    <div class="tk-node-head">
      ${n.agentIdentifier ? `<span class="tk-agent">@${esc(n.agentIdentifier)}</span>` : ""}
      <span class="tk-title" title="${esc(n.title || n.objective || "")}">${esc(n.title || n.objective || "未命名任务")}</span>
      <span class="task-status st-${esc(n.status ?? "pending")}">${ST_LABELS[n.status] ?? esc(n.status ?? "")}</span>
    </div>
    ${n.objective && n.objective !== n.title ? `<div class="tk-obj">${esc(n.objective)}</div>` : ""}
    ${deps.length ? `<div class="tk-deps">↳ 依赖：${esc(deps.join("、"))}</div>` : ""}
    ${(n.acceptance ?? []).length ? `<div class="tk-acc">${n.acceptance.slice(0, 4).map((a) => `<div>${esc(a)}</div>`).join("")}${n.acceptance.length > 4 ? `<div>…共 ${n.acceptance.length} 条</div>` : ""}</div>` : ""}`;
  // 点击节点 → 直聊该子代理（与右栏个体卡同口径）
  if (n.agentIdentifier) el.addEventListener("click", () => { showView("chat"); openUnitView(n.agentIdentifier); });
  return el;
}

// 台账分区：目标 / 验收 / 约束 / 禁区 / 优先 / 证据 / 缺口 / 未决 / 影响 / 回退 / 阻塞
function ledgerCard(session) {
  const led = session?.ledger;
  const card = setCard(`任务台账 · ${session?.title || "当前会话"}`);
  if (!led?.task) {
    card.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "台账暂不可用（会话详情拉取失败或为空）" }));
    return card;
  }
  const t = led.task ?? {};
  if (String(t.goal ?? "").trim()) {
    const g = document.createElement("div");
    g.className = "ledger-goal";
    g.textContent = t.goal;
    card.appendChild(g);
  } else {
    card.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "目标（goal）尚未登记——台账由网关在派发与回流时滚动维护。" }));
  }
  const addBlock = (title, items, opts = {}) => {
    // 空分区默认折叠不渲染（验收标准除外——它是契约核心，空也要可见）
    if (!items?.length && !opts.always) return;
    const w = document.createElement("div");
    w.className = "ledger-block";
    w.innerHTML = `<div class="ledger-block-title">${esc(title)}</div>`;
    if (!items?.length) w.appendChild(Object.assign(document.createElement("div"), { className: "set-hint", textContent: "（暂无）" }));
    else for (const it of items) {
      const row = document.createElement("div");
      row.className = "ledger-item";
      if (typeof it === "string") row.textContent = it;
      else row.innerHTML = `<span>${esc(it.text ?? "")}</span><span class="tag-dim">${esc(it.level ?? "")}</span><span class="ledger-src" title="${esc(it.source ?? "")}">${esc(it.source ?? "")}</span>`;
      w.appendChild(row);
    }
    card.appendChild(w);
  };
  addBlock("验收标准", t.acceptance, { always: true });
  addBlock("约束", t.constraints);
  addBlock("禁区", t.forbidden);
  addBlock("优先级", t.priorities);
  addBlock("已确认证据", led.evidence?.confirmed);
  addBlock("信息缺口", led.evidence?.gaps);
  addBlock("未决断言", led.risk?.openAssertions);
  addBlock("影响面", led.risk?.impact);
  addBlock("回退路径", led.risk?.revertPaths);
  addBlock("阻塞项", led.risk?.blockers);
  return card;
}

// ────────────────────────────── 编码页（整页）：文件树 / 编辑器 / 变更 / Git ──────────────────────────────
// 口径：fs/list 懒加载目录（网关要求 path 非空，根目录传 "."，返回项统一剥掉 "./" 前缀）；
// fs/file 读（200KB 截断）+ PUT 写回；变更与 Git 直连 workspace/changes、git/overview、git/op。

function enterCode() {
  if (!S.workspace) { renderCodeNoWorkspace(); return; }
  $("cp-ws-name").textContent = basename(S.workspace);
  $("cp-ws-name").title = S.workspace;
  renderEditor();
  renderTree();
  renderChangesPanel();
  renderGitPanel();
  refreshCodeAll(); // 后台拉新数据，不阻塞首次呈现
}

function renderCodeNoWorkspace() {
  $("cp-ws-name").textContent = "未设置";
  const box = $("cp-tree");
  box.innerHTML = "";
  const tip = document.createElement("div");
  tip.className = "cp-tree-empty";
  tip.innerHTML = `尚未选择工作目录。<br/>回到对话页，点底部「📂」芯片选择项目文件夹，<br/>这里就会成为编码工作台。`;
  box.appendChild(tip);
  box.appendChild(mkBtn("去选择工作目录", () => { showView("chat"); openPopover(wsMenu); }));
  S.code.file = null;
  renderEditor();
  $("cp-changes").innerHTML = `<div class="cp-hint">未设置工作目录——变更视图不可用</div>`;
  $("cp-git").innerHTML = `<div class="cp-hint">未设置工作目录——Git 视图不可用</div>`;
}

async function refreshCodeAll() {
  if (!S.workspace) return;
  // 根目录 + 变更 + 概览并行拉取；单项失败不拖累整体
  await Promise.all([loadDir("").catch(() => {}), refreshChanges(), refreshGitOverview()]);
  if (S.view === "code") renderTree();
}

// 拉取目录清单（懒加载）：dir 为 "" 表示工作区根
async function loadDir(dir) {
  const r = await api.fsList(dir || ".");
  const slot = S.code.tree[dir] ?? (S.code.tree[dir] = { open: dir === "", loaded: false, entries: [] });
  // 根目录（"."）返回的子路径带 "./" 前缀：统一剥掉，树内路径保持干净的相对形式
  slot.entries = (r.entries ?? []).map((e) => ({ ...e, path: String(e.path ?? "").replace(/^\.\//, "") }));
  slot.loaded = true;
}

async function toggleDir(entry) {
  const slot = S.code.tree[entry.path] ?? (S.code.tree[entry.path] = { open: false, loaded: false, entries: [] });
  slot.open = !slot.open;
  if (slot.open && !slot.loaded) {
    try { await loadDir(entry.path); }
    catch (e) { toast(String(e).slice(0, 160), false); }
  }
  renderTree();
}

function renderTree() {
  const box = $("cp-tree");
  box.innerHTML = "";
  if (!S.workspace) return; // 空态已由 renderCodeNoWorkspace 呈现
  const walk = (dir, depth) => {
    const slot = S.code.tree[dir];
    for (const e of slot?.entries ?? []) {
      box.appendChild(treeItem(e, depth));
      // 只渲染「已加载且展开」的子目录（懒加载树的核心）
      if (e.dir && slot.open && S.code.tree[e.path]?.open) walk(e.path, depth + 1);
    }
  };
  walk("", 0);
  // 只在根目录真实加载过后才判空，避免懒加载间隙误显「目录为空」
  if (S.code.tree[""]?.loaded && !S.code.tree[""]?.entries?.length) box.innerHTML = `<div class="cp-tree-empty">目录为空</div>`;
}

function treeItem(e, depth) {
  const item = document.createElement("div");
  const slot = S.code.tree[e.path];
  const open = Boolean(e.dir && slot?.open);
  const mark = S.code.changeMap.get(e.path);
  const dirty = S.code.file?.path === e.path && S.code.file.dirty;
  item.className = `cp-tree-item${S.code.file?.path === e.path ? " on" : ""}`;
  item.style.paddingLeft = `${10 + depth * 14}px`;
  item.title = e.path;
  item.innerHTML = `<span class="tw">${e.dir ? (open ? "▾" : "▸") : ""}</span><span class="fico">${e.dir ? "▤" : "▪"}</span>`
    + `${mark ? `<span class="fdot st-${esc(mark)}" title="变更：${esc(CHANGE_STATUS_LABELS[mark] ?? mark)}"></span>` : ""}`
    + `<span class="fname">${esc(e.name)}</span>
    <span class="fmark">${dirty ? `<span class="st-dirty">●</span>` : mark ? `<span class="st-${esc(mark)}">${esc(mark)}</span>` : ""}</span>`;
  item.addEventListener("click", () => (e.dir ? toggleDir(e) : openFile(e.path)));
  return item;
}

// 未保存修改统一拦截口径：dirty 时弹确认，返回 false 表示用户选择留下
function confirmDiscardDirty(action = "离开") {
  const f = S.code.file;
  if (!f?.dirty) return true;
  return confirm(`「${f.path}」有未保存修改，${action}将丢弃。继续？`);
}

async function openFile(path) {
  if (S.code.file?.dirty && S.code.file.path !== path && !confirmDiscardDirty("打开新文件")) return;
  try {
    const r = await api.fsFile(path);
    const content = String(r.content ?? "");
    const binary = content.includes("\uFFFD"); // 替换字符 = 非 UTF-8 字节被改写，禁存防损坏
    S.code.file = {
      path,
      content,
      size: Number(r.size ?? 0),
      truncated: Boolean(r.truncated),
      binary,
      // 截断 / 非 UTF-8 一律只读预览：防误改后因禁存白干
      readonly: Boolean(r.truncated) || binary,
      dirty: false,
    };
    renderEditor();
    renderTree();
  } catch (e) { toast(String(e).slice(0, 160), false); }
}

function renderEditor() {
  const ta = $("cp-editor");
  const f = S.code.file;
  $("cp-editor-empty").classList.toggle("hidden", Boolean(f));
  $("cp-editor-wrap").classList.toggle("hidden", !f);
  // 截断 / 非 UTF-8 文件：textarea readOnly + 顶部提示条，从源头防误改
  ta.readOnly = Boolean(f?.readonly);
  $("cp-readonly-bar").classList.toggle("hidden", !f?.readonly);
  if (f) {
    ta.value = f.content;
    renderGutter(true);
    syncEditorScroll();
  } else {
    $("cp-curline").classList.add("hidden");
  }
  renderEditorState();
}

// —— 行号列 / 当前行 / 未保存行标记 ——
// 口径：textarea wrap="off"（不软换行），逻辑行与可视行一一对应，
// 行高等于 CSS 的 20px；行号列用 overflow:hidden + scrollTop 跟随文本层滚动。
const EDITOR_LINE_H = 20;
const EDITOR_PAD_TOP = 14;
let gutterSig = ""; // 行号列重建签名：行数 + 未保存区间，任一变化才重排

// 未保存行区间：当前内容与基线（f.content，保存成功即翻新）逐行对比，
// 公共前/后缀裁掉后剩下的连续区间即视为改动区（简化 diff：区间内行号画点）
function dirtyLineRange(baseLines, curLines) {
  const minLen = Math.min(baseLines.length, curLines.length);
  let lo = 0;
  while (lo < minLen && baseLines[lo] === curLines[lo]) lo++;
  let suf = 0;
  while (suf < minLen - lo && baseLines[baseLines.length - 1 - suf] === curLines[curLines.length - 1 - suf]) suf++;
  // 纯删行会让当前坐标系里的改动区变空：至少标住接缝处一行，保证 dirty 时行号列有可视提示
  let hi = Math.max(curLines.length - suf, Math.min(lo + 1, curLines.length));
  if (hi <= lo) { lo = Math.max(0, curLines.length - 1); hi = curLines.length; }
  return { lo, hi };
}

function renderGutter(force) {
  const lines = $("cp-editor").value.split("\n");
  const range = S.code.file?.dirty ? dirtyLineRange(S.code.file.content.split("\n"), lines) : null;
  const sig = `${lines.length}|${range ? `${range.lo}:${range.hi}` : ""}`;
  if (!force && sig === gutterSig) return;
  gutterSig = sig;
  $("cp-lines").innerHTML = lines
    .map((_, i) => `<div class="cp-ln${range && i >= range.lo && i < range.hi ? " dirty" : ""}">${i + 1}</div>`)
    .join("");
  syncEditorScroll();
}

function syncEditorScroll() {
  const ta = $("cp-editor");
  $("cp-lines").scrollTop = ta.scrollTop; // overflow:hidden 也可编程滚动
  positionCurLine();
}

// 当前行高亮条：按光标所在逻辑行定位（可视区外则隐藏）
function positionCurLine() {
  const ta = $("cp-editor");
  const bar = $("cp-curline");
  if (!S.code.file) { bar.classList.add("hidden"); return; }
  const line = ta.value.slice(0, ta.selectionStart).split("\n").length - 1;
  const top = EDITOR_PAD_TOP + line * EDITOR_LINE_H - ta.scrollTop;
  const viewH = $("cp-editor-wrap").clientHeight;
  if (top < 0 || top > viewH - EDITOR_LINE_H) { bar.classList.add("hidden"); return; }
  bar.classList.remove("hidden");
  bar.style.top = `${top}px`;
}

// 头部状态（路径 / 未保存 / 截断 / 大小 + 按钮可用性）：输入高频触发，保持轻量
function renderEditorState() {
  const f = S.code.file;
  $("cp-file-path").textContent = f?.path ?? "未打开文件";
  $("cp-file-state").innerHTML = !f ? "" : `
    ${f.dirty ? `<span class="cp-chip cp-chip-accent">● 未保存</span>` : ""}
    ${f.truncated ? `<span class="cp-chip cp-chip-warn">超过 200KB 已截断展示</span>` : ""}
    ${f.binary ? `<span class="cp-chip cp-chip-warn">非 UTF-8 内容</span>` : ""}
    <span class="cp-chip">${esc(fmtSize(f.size))}</span>`;
  // 截断 / 二进制（readonly）禁存：写回会把不完整内容覆盖到完整文件上
  $("cp-save").disabled = !f || f.readonly || !f.dirty;
  $("cp-reload").disabled = !f;
}

function onEditorInput() {
  const f = S.code.file;
  if (!f) return;
  const ta = $("cp-editor");
  f.dirty = ta.value !== f.content;
  renderGutter();
  positionCurLine();
  renderEditorState();
}

// —— Tab 缩进：光标处插入两空格；选区跨行时整块缩进 / Shift+Tab 反向去缩进 ——
const EDITOR_INDENT = "  ";

// 返回是否发生了编辑（调用方据此决定要不要 preventDefault 拦下焦点切换）
function editorIndent(shift) {
  const ta = $("cp-editor");
  if (!S.code.file || S.code.file.readonly) return false; // 只读态放行默认 Tab（移动焦点）
  const value = ta.value;
  const s = ta.selectionStart, e = ta.selectionEnd;
  if (s === e && !shift) {
    execEditorInsert(EDITOR_INDENT); // execCommand 走原生 undo 栈并触发 input 事件
    return true;
  }
  const lineStart = value.lastIndexOf("\n", s - 1) + 1;
  let lineEnd = value.indexOf("\n", e);
  if (lineEnd < 0) lineEnd = value.length;
  const block = value.slice(lineStart, lineEnd);
  const next = (shift ? block.split("\n").map((ln) => ln.replace(/^ {1,2}/, "")).join("\n")
    : block.split("\n").map((ln) => EDITOR_INDENT + ln).join("\n"));
  if (next === block) return false; // 无可去缩进：放行默认 Tab 行为（移动焦点）
  ta.setSelectionRange(lineStart, lineEnd);
  if (!execEditorInsert(next)) { ta.setSelectionRange(s, e); return false; }
  ta.setSelectionRange(lineStart, lineStart + next.length);
  return true;
}

// 插入文本并保持输入事件链（dirty / 行号 / 状态条联动）；execCommand 不可用时降级手动拼接
function execEditorInsert(text) {
  const ta = $("cp-editor");
  try {
    if (document.execCommand("insertText", false, text)) return true;
  } catch { /* 老内核降级 */ }
  ta.setRangeText(text, ta.selectionStart, ta.selectionEnd, "end");
  ta.dispatchEvent(new Event("input", { bubbles: true }));
  return true;
}

async function saveFile() {
  const f = S.code.file;
  const ta = $("cp-editor");
  if (!f || f.readonly || !f.dirty) return;
  try {
    await api.fsSave(f.path, ta.value);
    f.content = ta.value; // 基线翻新
    f.dirty = false;
    toast("已保存（含检查点与审计）");
    renderGutter(); // 签名随基线翻新变化，未保存行标记随之清除
    renderEditorState();
    renderTree();
    refreshChanges(); // 变更面板跟随刷新，不 await 阻塞
  } catch (e) { toast(String(e).slice(0, 160), false); }
}

async function refreshChanges() {
  try { S.code.changes = await api.workspaceChanges(); }
  catch { S.code.changes = null; }
  S.code.changeMap = new Map((S.code.changes?.files ?? []).map((f) => [String(f.path), String(f.status ?? "")]));
  if (S.view === "code") { renderChangesPanel(); renderTree(); }
}

// 变更状态字母 → 中文（git status --porcelain 单字母口径；未知状态原样展示）
const CHANGE_STATUS_LABELS = {
  A: "新增", M: "修改", D: "删除", R: "重命名", C: "复制",
  T: "类型变更", U: "冲突", X: "未知", B: "损坏",
};

// 统一 diff 文本的行数统计（+/−；跳过 +++/--- 文件头；truncated 时为下界）
function diffStats(text) {
  let add = 0, del = 0;
  for (const ln of String(text ?? "").split(/\r?\n/)) {
    if (ln.startsWith("+++") || ln.startsWith("---")) continue;
    if (ln.startsWith("+")) add++;
    else if (ln.startsWith("-")) del++;
  }
  return { add, del };
}

function renderChangesPanel() {
  const box = $("cp-changes");
  const c = S.code.changes;
  if (!c) {
    box.innerHTML = `<div class="cp-hint">变更加载失败——点「工作区变更」页签重试，或回对话页确认工作目录。</div>`;
    return;
  }
  const files = c.files ?? [];
  const isGit = c.source === "git";
  const totals = files.reduce((acc, f) => {
    const st = diffStats(f.diff?.text);
    return { add: acc.add + st.add, del: acc.del + st.del };
  }, { add: 0, del: 0 });
  let html = `<div class="cp-changes-head"><span>${isGit ? "分支" : "检查点口径（非 git 仓库）"}</span><span class="cp-branch">${esc(c.branch || "—")}</span>`
    + `<span class="cp-changes-count">${files.length} 个文件${totals.add || totals.del ? ` · <span class="add">+${totals.add}</span> <span class="del">−${totals.del}</span>` : ""}</span></div>`;
  // 批量操作：仅 git 口径提供（暂存全部直发；全部丢弃走审批闸门）
  if (isGit && files.length) {
    html += `<div class="cp-bulk">
      <button class="set-btn" id="cp-stage-all" title="git add -- .">＋ 暂存全部</button>
      <button class="set-btn danger" id="cp-discard-all" title="生成 git checkout -- . 审批单">↩ 全部丢弃</button>
      <span class="cp-bulk-hint">丢弃为破坏性操作，需经「待审批」放行</span>
    </div>`;
  }
  if (!files.length) {
    html += `<div class="cp-hint">工作区暂无未提交变更。<br/>AI 个体或你在此编辑保存后，文件差异会集中展示在这里。</div>`;
  }
  // 按状态分组（A 新增 / M 修改 / D 删除 …），组内保持原始顺序
  const groups = new Map();
  for (const f of files) {
    const key = String(f.status ?? "?");
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(f);
  }
  for (const [status, group] of groups) {
    html += `<div class="cp-group-head"><span class="cp-fst st-${esc(status)}">${esc(status)}</span>${esc(CHANGE_STATUS_LABELS[status] ?? status)}<span class="cp-group-count">${group.length}</span></div>`;
    for (const f of group) {
      const diffText = f.diff?.text ? diffHtml(f.diff.text) + (f.diff.truncated ? `\n<span class="d-hunk">… diff 过长已截断</span>` : "") : "";
      const stats = diffStats(f.diff?.text);
      const statHtml = f.diff?.text && (stats.add || stats.del)
        ? `<span class="cp-fstats"><span class="add">+${stats.add}</span><span class="del">−${stats.del}</span>${f.diff.truncated ? `<span class="trunc">…</span>` : ""}</span>`
        : "";
      html += `<div class="cp-file-row" data-path="${esc(f.path)}">
      <div class="cp-file-top">
        <span class="cp-fst st-${esc(f.status)}">${esc(f.status)}${f.staged ? "*" : ""}</span>
        <span class="cp-file-path2" title="${esc(f.path)}">${esc(f.path)}</span>
        ${statHtml}
        ${isGit ? `<span class="cp-file-ops"><button class="set-btn" data-op="${f.staged ? "unstage" : "stage"}" data-path="${esc(f.path)}">${f.staged ? "取消暂存" : "暂存"}</button><button class="set-btn danger" data-op="discard" data-path="${esc(f.path)}">丢弃</button></span>` : ""}
      </div>
      ${diffText ? `<pre class="cp-diff hidden">${diffText}</pre>` : (!f.diff && !f.staged ? `<pre class="cp-diff hidden" data-untracked="${esc(f.path)}"><span class="d-hunk">未跟踪文件 —— 展开加载全文</span></pre>` : "")}
    </div>`;
    }
  }
  if (isGit) {
    html += `<div class="cp-commit">
      <input id="cp-commit-msg" class="pop-input" placeholder="提交信息（只提交已暂存文件）" />
      <button id="cp-commit-btn" class="pop-save">提交</button>
      <div class="cp-hint">push / 硬重置 / 丢弃改动是破坏性操作：会生成审批单，经侧栏「待审批」放行后代执行。</div>
    </div>`;
  }
  box.innerHTML = html;
  // 行点击展开/收起 diff；未跟踪文件首次展开时懒加载全文
  box.querySelectorAll(".cp-file-top").forEach((top) => top.addEventListener("click", (e) => {
    if (e.target.closest("button")) return;
    const pre = top.closest(".cp-file-row")?.querySelector(".cp-diff");
    if (!pre) return;
    const showing = !pre.classList.contains("hidden");
    pre.classList.toggle("hidden", showing);
    if (!showing && pre.dataset.untracked) loadUntracked(pre, pre.dataset.untracked);
  }));
  box.querySelectorAll(".cp-file-ops button").forEach((b) => b.addEventListener("click", () => {
    const { op, path } = b.dataset;
    if (op === "discard" && !confirm(`丢弃「${path}」的未提交修改？将生成审批单。`)) return;
    runGitOp({ op, path }, op === "stage" ? "已暂存" : op === "unstage" ? "已取消暂存" : "已生成丢弃审批单");
  }));
  $("cp-stage-all")?.addEventListener("click", () =>
    // 暂存动作会触发面板重渲染，等它落定再聚焦新提交框，缩短「暂存 → 提交」路径
    runGitOp({ op: "stage", path: "." }, "已暂存全部变更").then(() => $("cp-commit-msg")?.focus()));
  $("cp-discard-all")?.addEventListener("click", () => {
    if (!confirm("丢弃全部未提交修改？将生成审批单「git checkout -- .」等待放行（不影响未跟踪的新文件）。")) return;
    runGitOp({ op: "discard", path: "." }, "已生成全部丢弃审批单");
  });
  box.querySelector("#cp-commit-btn")?.addEventListener("click", () => {
    const msg = box.querySelector("#cp-commit-msg").value.trim();
    if (!msg) return toast("提交信息不能为空", false);
    runGitOp({ op: "commit", message: msg }, "已提交");
  });
  // 提交框内 Enter 直接提交（输入法组词中不触发）
  box.querySelector("#cp-commit-msg")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.isComposing) box.querySelector("#cp-commit-btn")?.click();
  });
}

// diff 文本着色：+ 行绿 / - 行红 / @@ 与文件头暗（逐行包 span，内容整体已转义）
function diffHtml(text) {
  return String(text ?? "").split(/\r?\n/).map((ln) => {
    const cls = /^(\+\+\+|---)/.test(ln) ? "d-hunk" : ln.startsWith("+") ? "d-add" : ln.startsWith("-") ? "d-del" : ln.startsWith("@@") ? "d-hunk" : "d-ctx";
    return `<span class="${cls}">${esc(ln) || " "}</span>`;
  }).join("\n");
}

// 未跟踪文件没有 diff：全文按「新增行」口径渲染
async function loadUntracked(pre, path) {
  try {
    const r = await api.fsFile(path);
    pre.innerHTML = diffHtml(String(r.content ?? "").split(/\r?\n/).map((l) => `+${l}`).join("\n"));
  } catch (e) { pre.innerHTML = `<span class="d-hunk">${esc(String(e))}</span>`; }
}

// git 操作统一出口：pending = 破坏性操作已转审批单（批准后由系统代执行）
async function runGitOp(payload, okMsg) {
  try {
    const r = await api.gitOp(payload);
    if (r?.pending) {
      toast(`破坏性操作已生成审批单（${r.command}），等待放行`);
      refreshApprovalsBadge();
    } else {
      toast(okMsg ?? "操作成功");
    }
    await Promise.all([refreshChanges(), refreshGitOverview()]);
  } catch (e) { toast(String(e).slice(0, 160), false); }
}

async function refreshGitOverview() {
  try { S.code.overview = await api.gitOverview(); }
  catch { S.code.overview = null; }
  if (S.view === "code") renderGitPanel();
}

function renderGitPanel() {
  const box = $("cp-git");
  const ov = S.code.overview;
  if (!ov) {
    box.innerHTML = `<div class="cp-hint">Git 概览加载失败——点「Git」页签重试。</div>`;
    return;
  }
  if (!ov.repo) {
    box.innerHTML = `<div class="cp-hint">当前工作目录不是 git 仓库。<br/>「工作区变更」页签走检查点口径仍可看 AI 改动；初始化仓库后这里会呈现分支与提交历史。</div>`;
    return;
  }
  const branches = ov.branches ?? [];
  box.innerHTML = `
    <div class="cp-changes-head"><span>当前分支</span><span class="cp-branch">${esc(ov.branch || "—")}</span></div>
    ${branches.length ? `<div class="cp-section">分支（点击切换）</div>${branches.map((b) => `<button class="cp-branch-item${b === ov.branch ? " on" : ""}" data-branch="${esc(b)}">${esc(b)}${b === ov.branch ? " ✓" : ""}</button>`).join("")}` : ""}
    <div class="cp-section">提交历史（最近 20 条）</div>
    ${(ov.log ?? []).map((l) => { const sp = String(l).indexOf(" "); return `<div class="cp-log-line"><b>${esc(String(l).slice(0, sp < 0 ? 7 : sp))}</b> ${esc(sp < 0 ? "" : String(l).slice(sp + 1))}</div>`; }).join("") || `<div class="cp-hint">暂无提交</div>`}`;
  box.querySelectorAll(".cp-branch-item").forEach((b) => b.addEventListener("click", () => {
    if (b.dataset.branch === ov.branch) return;
    if (!confirm(`切换到分支「${b.dataset.branch}」？`)) return;
    runGitOp({ op: "switch", branch: b.dataset.branch }, `已切换到 ${b.dataset.branch}`);
  }));
}

function fmtSize(n) {
  if (!n) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

// ────────────────────────────── 通用模态 ──────────────────────────────

function showModal(title, build) {
  $("modal-title").textContent = title;
  const body = $("modal-body");
  body.innerHTML = "";
  build(body);
  $("modal").classList.remove("hidden");
}

function closeModal() { $("modal").classList.add("hidden"); }

// ────────────────────────────── 会话增强：重命名 / 分叉 / 撤销 / 归档 ──────────────────────────────

function sessionOps() {
  const s = S.sessions.find((x) => x.id === S.sessionId);
  if (!s) return;
  showModal(`会话操作 · ${s.title || "未命名"}`, (body) => {
    const row = (label, sub, fn) => {
      const b = document.createElement("button");
      b.className = "modal-row";
      b.innerHTML = `<b>${esc(label)}</b>${sub ? `<div class="modal-sub">${esc(sub)}</div>` : ""}`;
      b.addEventListener("click", fn);
      body.appendChild(b);
    };
    row("✎ 重命名", "在侧栏该会话上行内编辑标题", () => { closeModal(); startRename(S.sessionId); });
    row("⑂ 分叉新会话", "选择截至轮次，携带前 N 轮完整副本；原会话保持不变", () => turnsModal("fork"));
    row("↩ 撤销到指定轮次", "回退对话历史，可联动回滚该轮之后的文件变更", () => turnsModal("undo"));
    row("☰ 归档留痕", "被撤销 / 编辑重发替换的内容可查证", () => archiveModal());
  });
}

// 侧栏行内重命名：标题替换为输入框，Enter / 失焦提交，Esc 取消
function startRename(id) {
  const item = document.querySelector(`.session-item[data-sid="${CSS.escape(id)}"]`);
  const s = S.sessions.find((x) => x.id === id);
  const titleEl = item?.querySelector(".session-title");
  if (!item || !titleEl || item.querySelector(".session-rename")) return;
  const input = document.createElement("input");
  input.className = "session-rename";
  input.value = s?.title ?? "";
  titleEl.replaceWith(input);
  input.focus();
  input.select();
  let closed = false;
  const cancel = () => { if (!closed) { closed = true; renderSidebar(); } };
  const commit = async () => {
    if (closed) return;
    closed = true;
    const title = input.value.trim();
    if (!title || title === (s?.title ?? "")) { renderSidebar(); return; }
    try {
      await api.renameSession(id, title);
      S.sessions = S.sessions.map((x) => (x.id === id ? { ...x, title } : x));
      toast("已重命名");
    } catch (e) { alertErr(e); }
    renderSidebar();
  };
  // stopPropagation：避免 Enter / Esc 冒泡触发全局快捷键
  input.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") commit();
    else if (e.key === "Escape") cancel();
  });
  input.addEventListener("blur", commit);
  input.addEventListener("click", (e) => e.stopPropagation());
}

// 轮次选择（分叉 / 撤销共用）：快照清单给出可选轮次；无快照时手输轮次号兜底
async function turnsModal(mode) {
  const id = S.sessionId;
  if (!id) return;
  let snaps = [];
  try { snaps = await api.snapshots(id); } catch { /* 拉取失败走手输兜底 */ }
  showModal(mode === "fork" ? "分叉新会话 · 选择截至轮次" : "撤销 · 选择目标轮次", (body) => {
    if (mode === "undo") {
      const cb = document.createElement("label");
      cb.className = "modal-check";
      cb.innerHTML = `<input type="checkbox" id="undo-restore" checked /> 联动回滚该轮之后的文件变更（检查点口径）`;
      body.appendChild(cb);
    }
    const doTurn = async (turn) => {
      const n = Math.max(0, Math.floor(Number(turn) || 0));
      try {
        if (mode === "fork") {
          const ns = await api.forkSession(id, n);
          closeModal();
          toast(`已派生新会话（携带前 ${n} 轮）`);
          S.sessions = await api.sessions();
          renderSidebar();
          await selectSession(ns.id);
        } else {
          const restore = $("undo-restore")?.checked ?? true;
          await api.undoSession(id, n, restore);
          closeModal();
          toast(`已回退到第 ${n} 轮${restore ? "，文件已联动回滚" : ""}`);
          await selectSession(id); // 重拉消息 / 任务图 / 运行态
        }
      } catch (e) { toast(String(e).slice(0, 160), false); }
    };
    if (!Array.isArray(snaps) || !snaps.length) {
      body.appendChild(Object.assign(document.createElement("div"), { className: "cp-hint", textContent: "暂无轮次快照——完成一轮完整对话后再来；也可直接输入轮次号（0 = 空会话起算）。" }));
      const wrap = document.createElement("div");
      wrap.className = "modal-inline";
      const num = mkInput("1", { type: "number" });
      num.style.width = "110px";
      wrap.appendChild(num);
      wrap.appendChild(mkBtn(mode === "fork" ? "分叉" : "撤销", () => doTurn(num.value)));
      body.appendChild(wrap);
      return;
    }
    // 新轮次在上：最近的操作离手最近
    for (const t of [...snaps].sort((a, b) => (b.turn ?? 0) - (a.turn ?? 0))) {
      const b = document.createElement("button");
      b.className = "modal-row";
      const at = t.createdAt ?? t.created_at;
      b.innerHTML = `<b>第 ${esc(t.turn)} 轮</b><div class="modal-sub">截至 ${esc(t.messageCount ?? 0)} 条消息${at ? ` · ${esc(relTime(at))}` : ""}</div>`;
      b.addEventListener("click", () => doTurn(t.turn));
      body.appendChild(b);
    }
  });
}

// 归档查看：撤销 / 编辑重发时被替换的历史内容（服务端 message_archive 留痕）
async function archiveModal() {
  const id = S.sessionId;
  if (!id) return;
  let msgs = [];
  try { msgs = await api.archive(id); }
  catch (e) { toast(String(e).slice(0, 160), false); return; }
  showModal("归档留痕", (body) => {
    if (!Array.isArray(msgs) || !msgs.length) {
      body.appendChild(Object.assign(document.createElement("div"), {
        className: "cp-hint",
        textContent: "暂无归档——撤销或编辑重发时被替换的历史会留存在这里，可随时查证。",
      }));
      return;
    }
    for (const m of msgs) {
      const who = m.role === "user" ? "你" : `@${m.agentId || "orchestrator"}`;
      const text = (m.statements ?? []).map((s) => s.text).join("\n");
      body.insertAdjacentHTML("beforeend", `<div class="arch-msg"><div class="arch-meta">${esc(who)} · ${esc(relTime(m.createdAt))}</div><div class="arch-text">${esc(text)}</div></div>`);
    }
  });
}

// ────────────────────────────── 初始化 ──────────────────────────────

function autosize() {
  const input = $("input");
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 180)}px`;
}

async function init() {
  $("btn-back").addEventListener("click", exitUnitView);
  $("sp-back").addEventListener("click", () => showView("chat"));
  // 自绘标题栏窗口控制（仅桌面端；浏览器调试隐藏按钮）
  if (window.__TAURI__) {
    $("tb-btns").classList.remove("hidden");
    $("tb-min").addEventListener("click", () => window.__TAURI__.core.invoke("min_window"));
    $("tb-max").addEventListener("click", () => window.__TAURI__.core.invoke("max_window"));
    $("tb-close").addEventListener("click", () => window.__TAURI__.core.invoke("close_window"));
  }
  $("btn-new").addEventListener("click", () => newSession().catch(alertErr));
  $("btn-send").addEventListener("click", send);
  $("btn-stop").addEventListener("click", async () => {
    if (!S.sessionId) return;
    const b = $("btn-stop");
    b.disabled = true;
    b.textContent = "停止中…"; // 受理期间禁点，避免重复 stop
    try {
      await api.stop(S.sessionId);
      // 兜底：停止受理后仍未收到 run.finished 就复位运行态（事件丢失不再卡死）
      setTimeout(() => { if (S.running) setRunning(false); }, 1500);
    } catch (e) {
      setRunning(false); // 409 = 本就空闲，直接复位
      alertErr(e);
    }
  });
  $("btn-side").addEventListener("click", () => $("sidebar").classList.toggle("collapsed"));
  $("btn-settings").addEventListener("click", () => showView(S.view === "settings" ? "chat" : "settings"));
  // 界面音效开关：本地合成提示音，状态记忆 localStorage exm.sfx（默认开；prefers-reduced-motion 默认关）
  const paintSfx = () => { $("btn-sfx").textContent = sfx.enabled ? "🔊 音效" : "🔇 静音"; };
  $("btn-sfx").addEventListener("click", () => {
    sfx.toggle();
    paintSfx();
    if (sfx.enabled) sfx.play("click"); // 重新开启的一声反馈；静音本身无声
  });
  paintSfx();
  $("btn-right").addEventListener("click", () => setRightOpen(!S.rightOpen));
  setRightOpen(S.rightOpen);
  // 新视图入口：编码 / 任务 / 待审批（直达设置审批 tab）
  $("btn-code").addEventListener("click", () => showView(S.view === "code" ? "chat" : "code"));
  $("btn-tasks").addEventListener("click", () => showView(S.view === "tasks" ? "chat" : "tasks"));
  $("btn-approvals").addEventListener("click", () => { S.settingsTab = "approval"; showView("settings"); });
  $("rp-full").addEventListener("click", () => showView("tasks"));
  $("btn-session-menu").addEventListener("click", sessionOps);
  // 编码页交互
  $("cp-back").addEventListener("click", () => showView("chat"));
  $("tp-back").addEventListener("click", () => showView("chat"));
  $("cp-tree-refresh").addEventListener("click", () => refreshCodeAll().catch(alertErr));
  $("cp-save").addEventListener("click", saveFile);
  $("cp-reload").addEventListener("click", () => {
    const f = S.code.file;
    if (!f) return;
    if (f.dirty && !confirmDiscardDirty("重新加载")) return;
    openFile(f.path);
  });
  $("cp-editor").addEventListener("input", onEditorInput);
  // 关窗兜底：编码页有未保存修改时浏览器级提醒（平时离开走 confirmDiscardDirty，Tauri 壳另有驻留逻辑）
  window.addEventListener("beforeunload", (e) => {
    if (!S.code.file?.dirty) return;
    e.preventDefault();
    e.returnValue = ""; // 兜住直接关窗 / 刷新丢改动
  });
  // 行号列 / 当前行跟随：滚动同步 + 光标移动（点击 / 按键）重定位
  $("cp-editor").addEventListener("scroll", syncEditorScroll);
  for (const ev of ["click", "keyup", "focus"]) $("cp-editor").addEventListener(ev, positionCurLine);
  $("cp-editor").addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") { e.preventDefault(); saveFile(); return; }
    // Tab 缩进（Shift+Tab 反缩进）；未发生编辑时不拦默认行为
    if (e.key === "Tab" && !e.ctrlKey && !e.metaKey && editorIndent(e.shiftKey)) e.preventDefault();
  });
  $("cp-tabs").querySelectorAll(".cp-tab").forEach((b) => b.addEventListener("click", () => {
    S.code.rightTab = b.dataset.tab;
    $("cp-tabs").querySelectorAll(".cp-tab").forEach((x) => x.classList.toggle("on", x === b));
    $("cp-changes").classList.toggle("hidden", S.code.rightTab !== "changes");
    $("cp-git").classList.toggle("hidden", S.code.rightTab !== "git");
    // 切回页签时顺手拉新：变更 / 概览都是廉价的只读接口
    if (S.code.rightTab === "git") refreshGitOverview();
    else refreshChanges();
  }));
  // 模态开合：关闭按钮 / 点 backdrop / Esc
  $("modal-close").addEventListener("click", closeModal);
  $("modal").addEventListener("click", (e) => { if (e.target === $("modal")) closeModal(); });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !$("modal").classList.contains("hidden")) closeModal();
  });
  const input = $("input");
  input.addEventListener("input", autosize);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); }
  });

  // 输入区上下文芯片：菜单开合与切换
  $("chip-ws").addEventListener("click", () => { sfx.play("click"); openPopover(wsMenu); });
  $("chip-target").addEventListener("click", () => { sfx.play("click"); openPopover(targetMenu); });
  $("chip-model").addEventListener("click", () => { sfx.play("click"); openPopover(modelMenu); });
  $("btn-plus").addEventListener("click", () => { sfx.play("click"); $("file-pick").click(); });
  $("file-pick").addEventListener("change", (e) => { addFiles([...e.target.files]); e.target.value = ""; });
  document.addEventListener("click", (e) => {
    const pop = $("popover");
    if (pop.classList.contains("hidden")) return;
    if (pop.contains(e.target) || e.target.closest?.(".chip")) return;
    closePopover();
  });

  try {
    const health = await api.health();
    setConn(true);
    $("conn-text").textContent = health.mock ? "已连接（测试替身）" : "已连接";
    const sps = document.getElementById("splash-status");
    if (sps) sps.textContent = "已连结";
  } catch { setConn(false); }

  // 上下文（组/智能体/模型/目录）与配置；模型未配置 → 引导设置
  await refreshContext();
  // 待审批角标：启动即拉一次；即时性走 WS 事件 + 窗口聚焦校准，60s 轮询仅作兜底
  void refreshApprovalsBadge();
  window.addEventListener("focus", () => void refreshApprovalsBadge());
  setInterval(() => void refreshApprovalsBadge(), 60000);
  // 思考强度本地记忆重设（网关重启后回到端点默认，此处恢复用户选择）
  if (S.effort !== "default") api.setEffort(S.effort).catch(() => {});
  try {
    S.config = await api.config();
    const llmReady = Boolean(S.config?.llm?.apiKey) || Boolean(S.config?.mock);
    const banner = $("banner");
    if (!llmReady) {
      banner.classList.remove("hidden");
      banner.innerHTML = `<span>模型尚未配置——在设置中填写提供商端点与 API Key，即可开始对话。</span><button class="banner-action" id="banner-settings">去设置</button>`;
      $("banner-settings").addEventListener("click", () => showView("settings"));
    }
  } catch { /* config 拉取失败不阻塞对话 */ }

  try {
    S.sessions = await api.sessions();
    renderSidebar();
    if (S.sessions.length) await selectSession(S.sessions[0].id);
    else await newSession();
  } catch (e) { alertErr(e); }
  hideSplash();
}

init();
