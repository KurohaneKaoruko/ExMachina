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
  chat: (id, text) => req(`/sessions/${id}/chat`, { method: "POST", body: JSON.stringify({ text }) }),
  stop: (id) => req(`/sessions/${id}/stop`, { method: "POST", body: JSON.stringify({}) }),
  graph: (id) => req(`/sessions/${id}/graph`),
  decideApproval: (id, approve) => req(`/approvals/${id}/${approve ? "approve" : "deny"}`, { method: "POST", body: JSON.stringify({}) }),
};

// ────────────────────────────── 全局状态 ──────────────────────────────

const S = {
  sessions: [],
  sessionId: null,
  messages: [],        // 已收束消息 {id, role, agentId?, statements[{tag,text}], toolCalls?, thinking?, createdAt}
  running: false,
  connected: false,
  target: null,        // {mode, id, name?, primary?}
  config: null,
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
    // 断连补偿：重拉消息 + 在途判定，补齐丢失事件
    void (async () => {
      try {
        const [messages, graph] = await Promise.all([api.messages(sessionId), api.graph(sessionId)]);
        if (gen !== wsGen || S.sessionId !== sessionId) return;
        const pending = (graph?.nodes ?? []).some((n) => ["running", "dispatched", "syncing"].includes(String(n.status ?? "")));
        S.messages = messages;
        setRunning(pending);
        renderStream();
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
      live.units[String(p.agentId ?? "")] = (live.units[String(p.agentId ?? "")] ?? "") + String(p.delta ?? "");
      scheduleLive();
      break;
    case "unit.thinking":
      live.unitThinking[String(p.agentId ?? "")] = (live.unitThinking[String(p.agentId ?? "")] ?? "") + String(p.delta ?? "");
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
    case "dispatch.sent":
      live.activity.push({ text: `派发 <b>@${esc(String(p.agentIdentifier ?? ""))}</b> → ${esc(String(p.nodeId ?? ""))}` });
      scheduleLive();
      break;
    case "sync.received": {
      const r = p.report ?? {};
      delete live.units[String(r.sourceAgent ?? "")];
      live.activity.push({ text: `<b>@${esc(String(r.sourceAgent ?? ""))}</b> 回执：${esc(String(r.summary ?? "").slice(0, 90))}（置信 ${Number(r.confidence ?? 0).toFixed(2)}）` });
      scheduleLive();
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
  S.live = { orch: "", thinking: "", units: {}, unitThinking: {}, toolCalls: [], activity: [], approvals: S.live.approvals };
}

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
  for (const s of S.sessions) {
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
    nav.appendChild(item);
  }
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
    <div class="avatar">${isUser ? "你" : "EX"}</div>
    <div class="msg-body"><div class="msg-meta">${esc(who)} · ${relTime(msg.createdAt)}</div>${body}${traceHtml(msg)}</div>
  </div>`;
}

function renderStream(scroll) {
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

function welcomeEl() {
  const el = document.createElement("div");
  el.className = "welcome";
  el.innerHTML = `
    <div class="logo">EX</div>
    <h1>有什么可以帮你？</h1>
    <p>对话将交由当前智能体组协作完成；控制台可管理个体、模型与通道</p>
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
let livePending = false;
function scheduleLive(force) {
  if (livePending && !force) return;
  livePending = true;
  requestAnimationFrame(() => {
    livePending = false;
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
    const units = Object.entries(live.units).map(([agent, text]) =>
      `<div class="live-unit"><div class="live-unit-head">@${esc(agent)}</div><div class="live-unit-text">${esc(text)}</div></div>`).join("");
    liveEl.innerHTML = `<div class="msg assistant"><div class="avatar">EX</div>
      <div class="msg-body">${orch}
        ${tools || activity ? `<div class="trace"><details open><summary>执行过程</summary>${tools}${activity}</details></div>` : ""}
        ${units ? `<div class="live-units">${units}</div>` : ""}
      </div></div>`;
    if (nearBottom()) scrollBottom();
  });
}

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
  renderSidebar();
  try {
    const [messages, graph] = await Promise.all([api.messages(id), api.graph(id)]);
    if (S.sessionId !== id) return;
    S.messages = messages;
    const pending = (graph?.nodes ?? []).some((n) => ["running", "dispatched", "syncing"].includes(String(n.status ?? "")));
    setRunning(pending);
  } catch (e) {
    S.messages = [];
    alertErr(e);
  }
  renderStream();
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
  S.messages = [...S.messages, { id: `local-${Date.now()}`, role: "user", statements: [{ tag: "要求", text }], createdAt: new Date().toISOString() }];
  resetLive();
  setRunning(true);
  renderStream(true);
  updateSessionPreview(S.sessionId, text);
  try {
    await api.chat(S.sessionId, text);
  } catch (e) {
    setRunning(false);
    alertErr(e);
  }
}

// ────────────────────────────── 初始化 ──────────────────────────────

function autosize() {
  const input = $("input");
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 180)}px`;
}

async function openConsole() {
  try {
    await window.__TAURI__?.core?.invoke("open_console");
  } catch {
    window.open(GW.url, "_blank");
  }
}

async function init() {
  $("btn-new").addEventListener("click", () => newSession().catch(alertErr));
  $("btn-send").addEventListener("click", send);
  $("btn-stop").addEventListener("click", () => S.sessionId && api.stop(S.sessionId).catch(alertErr));
  $("btn-side").addEventListener("click", () => $("sidebar").classList.toggle("collapsed"));
  $("btn-console").addEventListener("click", openConsole);
  const input = $("input");
  input.addEventListener("input", autosize);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); }
  });

  try {
    const health = await api.health();
    setConn(true);
    $("conn-text").textContent = health.mock ? "已连接（测试替身）" : "已连接";
  } catch { setConn(false); }

  // 目标指示（组 / 单体）；模型未配置 → 引导控制台
  try {
    const [target, config] = [await api.target(), await api.config()];
    S.target = target;
    S.config = config;
    const name = target?.mode === "single" ? `单体 · ${target.name || target.id}` : `组 · ${target?.id ?? "default"}`;
    $("target-chip").innerHTML = `对话对象：<b>${esc(name)}</b>`;
    const llmReady = Boolean(config?.llm?.apiKey) || health?.mock;
    const banner = $("banner");
    if (!llmReady) {
      banner.classList.remove("hidden");
      banner.innerHTML = `<span>模型尚未配置——先在控制台填写提供商端点与 API Key，即可开始对话。</span><button class="banner-action" id="banner-console">打开控制台</button>`;
      $("banner-console").addEventListener("click", openConsole);
    }
  } catch { /* target/config 拉取失败不阻塞对话 */ }

  try {
    S.sessions = await api.sessions();
    renderSidebar();
    if (S.sessions.length) await selectSession(S.sessions[0].id);
    else await newSession();
  } catch (e) { alertErr(e); }
}

init();
