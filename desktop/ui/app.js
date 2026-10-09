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
    throw new Error(`${init?.method ?? "GET"} ${path} → ${resp.status} ${body.slice(0, 200)}`);
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
};

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
  $("btn-stop").classList.toggle("hidden", !on);
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
}

function sessionEl(s) {
  const item = document.createElement("div");
  item.className = `session-item${s.id === S.sessionId ? " active" : ""}`;
  item.innerHTML = `
    <div class="session-title">${esc(s.title || "未命名会话")}</div>
    <div class="session-preview">${esc(s.lastMessagePreview ?? "")}</div>
    <button class="session-del" title="删除会话">✕</button>`;
  item.addEventListener("click", () => selectSession(s.id));
  item.querySelector(".session-del").addEventListener("click", (e) => {
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

function renderStream(scroll) {
  // 设置整页接管期间不触碰对话 DOM（返回对话时统一重渲染）
  if (S.view === "settings") return;
  // 子代理会话视图：与主会话同款界面（流 + 输入区共用），内容过滤为该个体的对话
  if (S.unitView) { renderUnitStream(scroll); return; }
  // 已收束消息
  streamEl.innerHTML = "";
  if (!S.messages.length && !S.running) streamEl.appendChild(welcomeEl());
  for (const m of S.messages) streamEl.insertAdjacentHTML("beforeend", msgHtml(m));
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
    <h1>有什么可以帮你？</h1>
    <p>对话交由当前对象（智能体或智能体组）协作完成；设置中可管理模型、个体与通道</p>
    <div class="suggest">
      <button data-q="帮我梳理一下这个项目的整体结构，给出模块说明">梳理项目结构，输出模块说明</button>
      <button data-q="写一个 Python 脚本：批量重命名当前目录下的图片文件，按日期编号">写一个批量重命名图片的脚本</button>
      <button data-q="总结今天的待办事项，按优先级排序列出">总结今天的待办，按优先级排序</button>
    </div>`;
  el.querySelectorAll(".suggest button").forEach((b) =>
    b.addEventListener("click", () => { $("input").value = b.dataset.q; autosize(); $("input").focus(); }));
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
  } catch (e) { alertErr(e); }
}

function updateSessionPreview(id, preview) {
  S.sessions = S.sessions.map((s) => (s.id === id ? { ...s, lastMessagePreview: preview } : s));
  renderSidebar();
}

function alertErr(e) {
  console.error(e);
  S.messages = [...S.messages, { id: `err-${Date.now()}`, role: "error", statements: [{ tag: "警告", text: String(e) }], createdAt: new Date().toISOString() }];
  renderStream(true);
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

function renderRight() {
  const body = $("rp-body");
  const nodes = S.graph?.nodes ?? [];
  const units = Object.entries(S.live.units);
  const unitThink = Object.entries(S.live.unitThinking);
  $("rp-count").textContent = nodes.length ? `${nodes.filter((n) => n.status === "done").length}/${nodes.length}` : "";
  if (!nodes.length && !units.length && !unitThink.length && !S.reports.length) {
    body.innerHTML = `<div class="rp-empty">本轮暂无派发任务<br/>指挥体拆解任务后，子个体的执行情况会在这里实时展示；点击个体卡可进入其会话</div>`;
    return;
  }
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
}

// ────────────────────────────── 设置视图（原生，直连环口 API） ──────────────────────────────
// 桌面端不依赖任何 webui：模型与提供商 / 组与个体 / 通道 / 定时任务 / 安全 全部壳内直管。
// 一切核心功能在 exm-core，桌面端与 webui、channel 一样只是连结核心的客户端。

const SETTINGS_TABS = [["model", "模型"], ["group", "组与个体"], ["channel", "通道"], ["cron", "定时任务"], ["security", "安全"]];

function showView(v) {
  S.view = v;
  updateTopbar();
  // 设置是独立整页：接管整个窗口（对话区整体隐藏），返回时还原
  $("settings-page").classList.toggle("hidden", v !== "settings");
  $("app").classList.toggle("hidden", v === "settings");
  if (v === "settings") renderSettings();
  else renderView();
}

function renderView() {
  if (S.view === "settings") renderSettings();
  else renderStream(true);
}

function toast(msg, ok = true) {
  const t = document.createElement("div");
  t.className = `toast ${ok ? "ok" : "err"}`;
  t.textContent = msg;
  document.body.appendChild(t);
  setTimeout(() => t.remove(), 2600);
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
  ({ model: renderSetModel, group: renderSetGroup, channel: renderSetChannel, cron: renderSetCron, security: renderSetSecurity }[S.settingsTab] ?? renderSetModel)(body);
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
  b.addEventListener("click", onclick);
  return b;
}
function setCard(title) {
  const c = document.createElement("div");
  c.className = "set-card";
  const h = document.createElement("div");
  h.className = "set-card-title";
  h.textContent = title;
  c.appendChild(h);
  return c;
}
async function settingsSave(promise, okMsg) {
  try { await promise; toast(okMsg); return true; }
  catch (e) { toast(String(e).slice(0, 160), false); return false; }
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
  function profileFormSlot() { const d = document.createElement("div"); d.id = "profile-form"; return d; }
  function profileForm(p) {
    const slot = body.querySelector("#profile-form") || body.appendChild(profileFormSlot());
    slot.innerHTML = "";
    const c = setCard(p ? `编辑档案：${p.name}` : "新增档案");
    const name = mkInput(p?.name ?? "", { placeholder: "如 DeepSeek / GLM" });
    const baseUrl = mkInput(p?.baseUrl ?? "https://api.openai.com/v1");
    const fmt = mkSelect([["openai", "OpenAI 兼容"], ["anthropic", "Anthropic"], ["gemini", "Gemini"]], p?.apiFormat || "openai");
    const key = mkInput("", { type: "password", placeholder: p ? "留空沿用已配置 Key" : "sk-..." });
    const model = mkInput(p?.model ?? "", { placeholder: "默认模型名，如 GLM-5.3-Flash" });
    c.appendChild(mkField("名称", name));
    c.appendChild(mkField("端点 Base URL", baseUrl));
    c.appendChild(mkField("协议", fmt));
    c.appendChild(mkField("API Key", key));
    c.appendChild(mkField("默认模型", model));
    const ops = document.createElement("div");
    ops.className = "set-ops";
    ops.appendChild(mkBtn("保存", async () => {
      const payload = { id: p?.id, name: name.value.trim() || "未命名", baseUrl: baseUrl.value.trim(), apiFormat: fmt.value, model: model.value.trim() };
      if (key.value.trim()) payload.apiKey = key.value.trim();
      if (await settingsSave(req("/llm/profiles", { method: "POST", body: JSON.stringify(payload) }), "档案已保存")) { await refreshContext(); renderSettings(); }
    }));
    if (p) ops.appendChild(mkBtn("拉取模型清单", async () => {
      const r = await req("/llm/models", { method: "POST", body: JSON.stringify({ id: p.id }) }).catch((e) => ({ error: String(e) }));
      if (!r?.ok) return toast(`失败：${r?.error || r?.message || "未知"}`, false);
      // 清单入库：写入档案 models（模型选择器按供应商分组展示的就是这份）
      const models = (r.models ?? []).map((m) => ({ model: m }));
      if (await settingsSave(req("/llm/profiles", { method: "POST", body: JSON.stringify({ id: p.id, name: p.name, baseUrl: p.baseUrl, apiFormat: p.apiFormat, model: p.model, models }) }), `已拉取并保存 ${models.length} 个模型`)) { await refreshContext(); renderSettings(); }
    }, "set-btn"));
    c.appendChild(ops);
    slot.appendChild(c);
    slot.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }
}

// —— 组与个体 ——
function renderSetGroup(body) {
  const list = setCard("智能体组（项目协作单元）");
  for (const g of S.groups) {
    const row = document.createElement("div");
    row.className = "set-row";
    const active = S.target?.mode === "group" && (S.target.id ?? "default") === g.id;
    row.innerHTML = `<div class="set-row-main"><b>${esc(g.name)}</b>${active ? '<span class="tag-ok">✓ 当前</span>' : g.builtin ? '<span class="tag-dim">内置</span>' : ""}
      <div class="set-sub">${esc(g.workspace || "未设工作目录")} · 主智能体 ${esc(g.primary || "未指定")}</div>
      <div class="set-agents" data-gid="${esc(g.id)}"></div></div>`;
    const ops = document.createElement("div");
    ops.className = "set-ops";
    if (!active) ops.appendChild(mkBtn("设为当前", async () => {
      if (await settingsSave(req("/groups/active", { method: "PUT", body: JSON.stringify({ id: g.id }) }), "已切换")) await refreshContext(), renderSettings();
    }, "set-btn"));
    ops.appendChild(mkBtn("工作目录", () => {
      const input = mkInput(g.workspace || "", { placeholder: "/path/to/project" });
      row.querySelector(".set-row-main").appendChild(mkField("工作目录", input));
      input.after(mkBtn("保存目录", async () => {
        if (await settingsSave(api.setGroupWorkspace(g.id, input.value.trim()), "工作目录已保存")) renderSettings();
      }, "set-btn"));
      input.focus();
    }, "set-btn"));
    if (!g.builtin) ops.appendChild(mkBtn("删除", async () => {
      if (!confirm(`删除组「${g.name}」？`)) return;
      if (await settingsSave(req(`/groups/${g.id}`, { method: "DELETE" }), "已删除")) await refreshContext(), renderSettings();
    }, "set-btn danger"));
    row.appendChild(ops);
    list.appendChild(row);
    // 组内个体（懒加载）
    req(`/groups/${g.id}/agents`).then((agents) => {
      const box = row.querySelector(".set-agents");
      box.innerHTML = "";
      for (const a of agents) {
        const chip = document.createElement("span");
        chip.className = "agent-chip";
        const isPrimary = g.primary === a.identifier;
        chip.innerHTML = `@${esc(a.identifier)}${isPrimary ? ' <i class="pri">主</i>' : ""}`;
        if (!isPrimary) {
          chip.style.cursor = "pointer";
          chip.title = "设为组主智能体";
          chip.addEventListener("click", async () => {
            if (await settingsSave(req(`/groups/${g.id}/primary`, { method: "POST", body: JSON.stringify({ identifier: a.identifier }) }), `主智能体 → @${a.identifier}`)) renderSettings();
          });
        }
        box.appendChild(chip);
      }
    }).catch(() => {});
  }
  body.appendChild(list);
  const create = setCard("新建智能体组");
  const name = mkInput("", { placeholder: "组名称，如 前端小组" });
  const desc = mkInput("", { placeholder: "一句话描述（可选）" });
  create.appendChild(mkField("名称", name));
  create.appendChild(mkField("描述", desc));
  create.appendChild(mkBtn("创建", async () => {
    if (!name.value.trim()) return toast("组名称不能为空", false);
    const payload = { name: name.value.trim() };
    if (desc.value.trim()) payload.description = desc.value.trim();
    if (await settingsSave(req("/groups", { method: "POST", body: JSON.stringify(payload) }), "组已创建")) { await refreshContext(); renderSettings(); }
  }));
  body.appendChild(create);
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
function renderSetSecurity(body) {
  const c = setCard("安全与审批");
  req("/config").then(async (cfg) => {
    const sec = cfg.security ?? {};
    const authKey = mkInput("", { type: "password", placeholder: sec.authKey ? "已配置（留空沿用）" : "未配置" });
    const approval = mkSelect([["off", "off — 不拦截"], ["risky", "risky — 拦截高危命令"], ["always", "always — 全部命令需审批"]], sec.execApproval || "off");
    const allowlist = mkInput((sec.execAllowlist ?? []).join?.(",") ?? sec.execAllowlist ?? "", { placeholder: "命令前缀白名单，逗号分隔" });
    const timeout = mkInput(sec.terminalTimeoutSecs ?? "", { placeholder: "秒" });
    c.appendChild(mkField("后台访问密钥", authKey));
    c.appendChild(mkField("终端命令审批", approval));
    c.appendChild(mkField("命令前缀白名单", allowlist));
    c.appendChild(mkField("终端超时（秒）", timeout));
    c.appendChild(mkBtn("保存", async () => {
      const security = { execApproval: approval.value, execAllowlist: allowlist.value.trim() };
      if (authKey.value.trim()) security.authKey = authKey.value.trim();
      if (String(timeout.value).trim()) security.terminalTimeoutSecs = Number(timeout.value);
      if (await settingsSave(req("/config", { method: "PUT", body: JSON.stringify({ security }) }), "安全设置已保存")) renderSettings();
    }));
    body.appendChild(c);
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
  b.addEventListener("click", (e) => { e.stopPropagation(); onclick(e); });
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
  try {
    const r = await api.setWorkspace(path);
    S.workspace = String(r?.workspace ?? "");
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
  $("btn-stop").addEventListener("click", () => S.sessionId && api.stop(S.sessionId).catch(alertErr));
  $("btn-side").addEventListener("click", () => $("sidebar").classList.toggle("collapsed"));
  $("btn-settings").addEventListener("click", () => showView(S.view === "settings" ? "chat" : "settings"));
  $("btn-right").addEventListener("click", () => setRightOpen(!S.rightOpen));
  setRightOpen(S.rightOpen);
  const input = $("input");
  input.addEventListener("input", autosize);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); }
  });

  // 输入区上下文芯片：菜单开合与切换
  $("chip-ws").addEventListener("click", () => openPopover(wsMenu));
  $("chip-target").addEventListener("click", () => openPopover(targetMenu));
  $("chip-model").addEventListener("click", () => openPopover(modelMenu));
  $("btn-plus").addEventListener("click", () => $("file-pick").click());
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
  } catch { setConn(false); }

  // 上下文（组/智能体/模型/目录）与配置；模型未配置 → 引导设置
  await refreshContext();
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
}

init();
