/** 对话视图（整合控制台）：左栏 = 组切换 + 会话列表；右侧 = 消息流 + 实时流 + 输入 */
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Card, Dropdown, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message } from "antd";
import { ArrowLeftOutlined, CheckOutlined, CloudUploadOutlined, CloseOutlined, EditOutlined, ExportOutlined, FolderOpenOutlined, MessageOutlined, MoreOutlined, PaperClipOutlined, PauseCircleOutlined, PlusOutlined, RobotOutlined, SearchOutlined, SendOutlined, TeamOutlined, UserOutlined } from "@ant-design/icons";
import { useIsNarrow, NARROW_QUERY } from "../useIsNarrow";
import { api } from "../api";
import { agentIdEn } from "../models";
import { useExm, type TimelineItem } from "../store";
import { useT } from "../i18n/core";

interface TargetInfo { mode: "group" | "single"; id: string; name?: string }
import { StatementList } from "../components/Statements";
import { Markdown } from "../components/Markdown";
import { AsciiMeter } from "../components/Ascii";
import { ToolCallList } from "../components/ToolCall";
import { copyText } from "../components/Markdown";
import { UsageBar } from "../components/UsageBar";
import { ProcessSteps } from "../components/ProcessSteps";
import { TimelineTag } from "../components/TimelineTag";
import { SkeletonList } from "../components/SkeletonList";
import { TurnOps } from "../components/HistoryOps";
import { Emblem } from "../components/Emblem";
import type { ChatMessage } from "../types";

function MessageBubble({ m, turn, ops, orchLabel }: { m: ChatMessage; turn?: number; ops?: React.ReactNode; orchLabel?: string }): React.ReactElement {
  const tb = useT();
  const isUser = m.role === "user";
  const title = isUser ? tb("chat.role.user") : m.role === "orchestrator" ? (orchLabel ?? tb("chat.role.orchestrator")) : (m.agentId ?? tb("chat.role.system"));
  const hasThinking = Boolean(m.thinking && m.thinking.trim());
  const hasTools = Boolean(m.toolCalls && m.toolCalls.length > 0);
  const steps = m.steps ?? [];
  const hasProcess = hasThinking || hasTools || steps.length > 0;
  return (
    <Card size="small" className={`msg-bubble ${isUser ? "msg-user" : "msg-agent"}`}>
      <div className="msg-head">
        {isUser ? <UserOutlined /> : <RobotOutlined />} <b>{title}</b>
        {turn != null && <span className="msg-turn mono">#{turn}</span>}
        {!isUser && (
          <button
            className="msg-copy"
            title="复制"
            onClick={async () => {
              const text = m.statements.map((st) => st.text).join("\n\n");
              if (await copyText(text)) {
                message.success("已复制");
              }
            }}
          >
            ⧉
          </button>
        )}
        {ops && <span className="msg-ops">{ops}</span>}
      </div>
      {hasProcess &&
        (steps.length > 0 ? (
          // 有序步骤：思考/工具按发生顺序交错逐次显示（服务端留存，刷新后仍在）
          <ProcessSteps
            parts={steps.map((s) => ({ kind: s.kind, text: s.text, callId: s.callId }))}
            toolCalls={m.toolCalls ?? []}
            labelThinking={(n) => tUsage(tb, n)}
            labelTools={(n) => tb("chat.usedTools", { n: String(n) })}
          />
        ) : (
          <div className="msg-process">
            {hasThinking && (
              <UsageBar label={tUsage(tb, m.thinking!.length)}>
                <pre className="usage-think">{m.thinking}</pre>
              </UsageBar>
            )}
            {hasTools && (
              <UsageBar label={tb("chat.usedTools", { n: m.toolCalls!.length })}>
                <ToolCallList calls={m.toolCalls!} />
              </UsageBar>
            )}
          </div>
        ))}
      <Markdown text={m.statements.map((s) => s.text).join("\n\n")} />
    </Card>
  );
}

/** 思考栏摘要文案（OpenCode 口径：使用了 N 个思考；本地思维流以字数计量） */
function tUsage(tb: (k: string, v?: Record<string, string>) => string, chars: number): string {
  return tb("chat.usedThinking", { n: String(chars) });
}

/** 时间线行（派发/回流/记忆/裁决…）：派发条目带「查看子会话」跳转（子个体过程流入口） */
function TimelineRows({ items, tb, onOpenSub }: { items: TimelineItem[]; tb: (k: string, v?: Record<string, string>) => string; onOpenSub: (id: string) => void }): React.ReactElement {
  return (
    <>
      {items.map((tl, i) => (
        <div key={`tl-${i}`} className={`timeline-item tl-${tl.kind}`}>
          <TimelineTag kind={tl.kind} ns="chat" />
          <span className="tl-text">{tl.text}</span>
          {tl.childSessionId && (
            <button
              className="tl-sub-link"
              onClick={(e) => {
                e.stopPropagation();
                onOpenSub(tl.childSessionId!);
              }}
            >
              {tb("chat.openSubSession")} →
            </button>
          )}
        </div>
      ))}
    </>
  );
}

/** 相对时间（chat-ergonomics）：刚刚 / N 分钟前 / N 小时前 / 昨天 / N 天前 / 日期 */
export function relativeTime(iso?: string): string {
  if (!iso) return "";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const diff = Date.now() - t;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const hour = Math.floor(min / 60);
  if (hour < 24) return `${hour} 小时前`;
  const day = Math.floor(hour / 24);
  if (day === 1) return "昨天";
  if (day < 7) return `${day} 天前`;
  return iso.slice(0, 10);
}

export function ChatView(): React.ReactElement {
  const t = useT();
  const {     messages, liveOrch, liveThinking, liveParts, runToolCalls, approvals,
    timeline, timelineSession, running, send, wsConnected, decideApproval,
    sessions, sessionId, selectSession, newSession,
    groups, activeGroup, setTarget,
    refreshAgents,
    sessionsLoaded,
  } = useExm();
  const [target, setTargetInfo] = useState<TargetInfo>({ mode: "group", id: activeGroup });
  const [singles, setSinglesList] = useState<{ identifier: string; name: string }[]>([]);
  // 确认弹窗走 useModal 演出器：静态 Modal.confirm 渲染在 React 树外、吃不到 ConfigProvider，
  // 暗色主题下会落回默认白底（与全局自绘样式打架）；演出器版本随主题/语言/令牌同步。
  const [modal, modalContextHolder] = Modal.useModal();

  const loadTarget = useCallback(async () => {
    try {
      const t = await api.getTarget();
      setTargetInfo({ mode: t.mode, id: t.id, name: t.name });
      const s = await api.listSingles();
      setSinglesList(s.singles.map((x) => ({ identifier: x.identifier, name: x.name })));
    } catch {
      // 目标接口不可用时退回组模式展示
    }
  }, []);

  useEffect(() => {
    void loadTarget();
  }, [loadTarget, activeGroup]);

  const changeTarget = async (value: string) => {
    const [mode, id] = value.startsWith("single:") ? (["single", value.slice(7)] as const) : (["group", value.slice(6)] as const);
    await setTarget(mode, id);
    message.success(mode === "single" ? t("chat.switchedSingle", { id }) : t("chat.switchedGroup", { id }));
    await loadTarget();
  };
  const [text, setText] = useState("");
  const [images, setImages] = useState<string[]>([]);
  const [sessionFilter, setSessionFilter] = useState("");
  /** 竖屏导航 = 列表页 / 会话页 两个独立页面（对标主流移动端）：
   *  进入应用先落在会话列表，点某条会话才进入该会话页，会话页有返回按钮回到列表。
   *  带深链（#/chat/<id>）时直接进会话页；桌面端两栏并排，screen 恒为 thread。 */
  const narrow = useIsNarrow();
  // 首屏偏好：窄屏默认落在列表页（深链 #/chat/<id> 直接进会话页）。
  // 注意不要用 effect 在首帧纠正 screen——matchMedia 在懒加载 chunk 落地时可能
  // 还没稳定，一次 setScreen("thread") 就会把用户永远留在会话页（实测复现）。
  const [screenPref, setScreenPref] = useState<"list" | "thread">(() => {
    if (typeof window === "undefined" || !window.matchMedia(NARROW_QUERY).matches) return "thread";
    return /^#\/chat\/[^/]/.test(window.location.hash) ? "thread" : "list";
  });
  const screen: "list" | "thread" = narrow ? screenPref : "thread";
  const setScreen = setScreenPref;
  /** 会话类型过滤（2.2）：all / normal / channel */
  const [sessionKind, setSessionKind] = useState("all");
  const [renaming, setRenaming] = useState<{ id: string; title: string } | null>(null);
  const [usage, setUsage] = useState<{
    estimate: number;
    budget: number;
    unlimited: boolean;
    /** provider 上报的真实用量可用（否则为字符估算） */
    measured?: boolean;
  } | null>(null);
  /** 上箭头召回的游标（null = 未处于召回态） */
  const recallIdxRef = useRef<number | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  // 附件压缩：Canvas 缩到 ≤1280px、JPEG 82%（控制多模态请求体尺寸）
  const addImage = (file: File) => {
    const reader = new FileReader();
    reader.onload = () => {
      const img = new Image();
      img.onload = () => {
        const scale = Math.min(1, 1280 / Math.max(img.width, img.height));
        const canvas = document.createElement("canvas");
        canvas.width = Math.round(img.width * scale);
        canvas.height = Math.round(img.height * scale);
        canvas.getContext("2d")?.drawImage(img, 0, 0, canvas.width, canvas.height);
        const dataUrl = canvas.toDataURL("image/jpeg", 0.82);
        setImages((prev) => [...prev, dataUrl].slice(0, 4));
      };
      img.src = String(reader.result);
    };
    reader.readAsDataURL(file);
  };

  // 子会话 = 派发运行台账：只读查看子个体过程流（不允许在子会话里开工新轮次）
  const isSubSession = Boolean(sessions.find((s) => s.id === sessionId)?.title.startsWith("sub:"));

  const doSend = () => {
    if (isSubSession) return;
    if (text.trim()) {
      void send(text, images);
      setText("");
      setImages([]);
    }
  };

  // 切会话恢复草稿（chat-ergonomics）
  useEffect(() => {
    if (!sessionId) return;
    setText(useExm.getState().drafts[sessionId] ?? "");
    recallIdxRef.current = null;
  }, [sessionId]);

  // 是否已滚离底部（chat-ergonomics：显示「回到底部」按钮）
  const [showBottomBtn, setShowBottomBtn] = useState(false);
  useEffect(() => {
    const el = document.querySelector(".chat-scroll");
    if (!el) return;
    const onScroll = () => setShowBottomBtn(el.scrollHeight - el.scrollTop - el.clientHeight > 120);
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    setShowBottomBtn(false);
  }, [messages, liveOrch, liveParts, runToolCalls, timeline]);

  useEffect(() => {
    if (!sessionId) return;
    void (async () => {
      try {
        setUsage(await api.sessionTokens(sessionId));
      } catch {
        // 用量接口不可用时静默
      }
    })();
  }, [sessionId, messages.length, running]);

  const liveUnitEntries = Object.entries(liveParts).filter(([, v]) => v.length > 0);
  const activeMeta = groups.find((g) => g.id === activeGroup);
  // 指挥体显示名（10）：智能体模式显示该智能体名；组模式显示「指挥体 [组名]」
  const orchLabel = target.mode === "single"
    ? (singles.find((x) => x.identifier === target.id)?.name ?? target.id)
    : t("chat.orchGroup", { group: activeMeta?.name ?? target.id });

  /** 会话归属的组名（列表副标题） */
  const groupNameOf = useCallback(
    (gid: string) => groups.find((g) => g.id === gid)?.name ?? gid,
    [groups],
  );

  /** 会话列表（按类型与关键字过滤） */
  const visibleSessions = useMemo(
    () =>
      sessions
        .filter((s) =>
          sessionKind === "all" ? true : sessionKind === "channel" ? s.title.startsWith("channel:") : !s.title.startsWith("channel:"),
        )
        .filter((s) => !sessionFilter || s.title.toLowerCase().includes(sessionFilter.toLowerCase())),
    [sessions, sessionKind, sessionFilter],
  );

  /** 按活跃时间分组：今天 / 昨天 / 更早（移动端列表页的段落结构） */
  const sessionGroups = useMemo(() => {
    const startOfToday = new Date();
    startOfToday.setHours(0, 0, 0, 0);
    const startOfYesterday = new Date(startOfToday);
    startOfYesterday.setDate(startOfYesterday.getDate() - 1);
    const buckets: { key: "today" | "yesterday" | "earlier"; labelKey: string; items: typeof sessions }[] = [
      { key: "today", labelKey: "chat.groupToday", items: [] },
      { key: "yesterday", labelKey: "chat.groupYesterday", items: [] },
      { key: "earlier", labelKey: "chat.groupEarlier", items: [] },
    ];
    for (const s of visibleSessions) {
      const ts = new Date(s.lastActiveAt ?? s.updatedAt ?? s.createdAt).getTime();
      const bucket = Number.isNaN(ts)
        ? buckets[2]
        : ts >= startOfToday.getTime()
          ? buckets[0]
          : ts >= startOfYesterday.getTime()
            ? buckets[1]
            : buckets[2];
      bucket.items.push(s);
    }
    return buckets.filter((b) => b.items.length > 0);
  }, [visibleSessions]);

  const removeSession = async (id: string) => {
    try {
      await api.deleteSession(id);
      // 从最新状态构建列表（不用渲染闭包的旧快照），无论删的是否为当前会话都先落列表
      const rest = useExm.getState().sessions.filter((s) => s.id !== id);
      useExm.setState({ sessions: rest });
      if (id === sessionId) {
        useExm.setState({ sessionId: undefined, messages: [], graph: null });
        if (rest.length > 0) {
          await selectSession(rest[0].id);
        } else {
          await newSession();
        }
      }
      // 列表删空后留在列表页（窄屏），否则会落在一个空会话页上
      if (rest.length === 0 && narrow) setScreen("list");
    } catch {
      // 删除失败静默：会话可能在运行中被网关拒绝
    }
  };

  // ---- 会话导出 Markdown（2.3）：角色前缀正文 + 工具摘要 + 思维链折叠引用 ----
  const exportSession = async (id: string, title: string) => {
    try {
      const msgs = await api.messages(id);
      const lines: string[] = [`# ${title}`, "", `> 导出时间：${new Date().toLocaleString()}`, ""];
      for (const m of msgs) {
        const role = m.role === "user" ? "👤 用户" : m.role === "orchestrator" ? (m.agentId ? `🤖 ${m.agentId}` : "🤖 指挥体") : "⚙️ 系统";
        lines.push(`## ${role} · ${m.createdAt}`, "");
        if (m.thinking) {
          lines.push("<details><summary>思维过程</summary>", "", "```", m.thinking, "```", "", "</details>", "");
        }
        if (m.toolCalls && m.toolCalls.length > 0) {
          lines.push("<details><summary>工具调用</summary>", "");
          for (const tc of m.toolCalls) {
            lines.push(`- \`${tc.tool}\` ${tc.status === "ok" ? "✅" : tc.status === "error" ? "❌" : "⏳"} ${tc.summary || ""}`);
          }
          lines.push("", "</details>", "");
        }
        lines.push(m.statements.map((st) => st.text).join("\n\n"), "");
      }
      const blob = new Blob([lines.join("\n")], { type: "text/markdown;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${title.replace(/[\\/:*?"<>|]/g, "_") || "session"}.md`;
      a.click();
      URL.revokeObjectURL(url);
      message.success(t("chat.exportDone"));
    } catch (e) {
      message.error(t("chat.exportFailed", { err: String(e).slice(0, 120) }));
    }
  };

  // ---- 会话历史管控（组 9.5）：撤销 / 编辑重发 / 分叉 ----
  const [editing, setEditing] = useState<{ turn: number; text: string } | null>(null);
  const [historyBusy, setHistoryBusy] = useState(false);

  const refreshSession = async () => {
    if (sessionId) await selectSession(sessionId);
  };

  const doUndo = async (turn: number) => {
    if (!sessionId) return;
    setHistoryBusy(true);
    try {
      const info = await api.undoSession(sessionId, turn - 1, true);
      message.success(t("chat.turn.undoDone", { n: String(info.archived) }));
      await refreshSession();
    } catch (e) {
      message.error(t("chat.turn.failed", { err: String(e) }));
    } finally {
      setHistoryBusy(false);
    }
  };

  const openEdit = (turn: number) => {
    // 原文 = 该轮的用户消息文本（本轮内第一条 user 陈述拼接）
    let n = 0;
    const msg = messages.find((m) => {
      if (m.role === "user") n += 1;
      return n === turn;
    });
    setEditing({ turn, text: msg ? msg.statements.map((s) => s.text).join("\n") : "" });
  };

  const doEdit = async () => {
    if (!sessionId || !editing) return;
    setHistoryBusy(true);
    try {
      await api.editSession(sessionId, editing.turn, editing.text);
      message.success(t("chat.turn.resent"));
      setEditing(null);
      await refreshSession();
    } catch (e) {
      message.error(t("chat.turn.failed", { err: String(e) }));
    } finally {
      setHistoryBusy(false);
    }
  };

  const doFork = async (turn: number) => {
    if (!sessionId) return;
    setHistoryBusy(true);
    try {
      const fork = await api.forkSession(sessionId, turn);
      useExm.setState({ sessions: [{ id: fork.id, title: fork.title } as never, ...useExm.getState().sessions] });
      message.success(t("chat.turn.forkDone"));
      await selectSession(fork.id);
    } catch (e) {
      message.error(t("chat.turn.failed", { err: String(e) }));
    } finally {
      setHistoryBusy(false);
    }
  };

  // 消息 → 轮次映射：用户消息序号即轮次（1 起）
  let userTurn = 0;
  const renderMessages = messages.map((m) => {
    if (m.role !== "user") return { m, turn: undefined as number | undefined };
    userTurn += 1;
    return { m, turn: userTurn };
  });

  return (
    <div className={`chat-shell screen-${screen}`} data-screen={screen}>
      {modalContextHolder}
      {/* 左栏 = 移动端「会话列表页」，桌面端是常驻侧栏（见 styles.css 的屏幕切换） */}
      <aside className="chat-rail">
        {/* 列表页标题栏（窄屏显示）：标题 + 会话计数 */}
        <div className="rail-head">
          <span className="rail-head-title">{t("chat.sessions")}</span>
          <span className="rail-head-count">{visibleSessions.length}</span>
        </div>
        <div className="rail-section">
          <div className="rail-label">
            <span className="rail-no">01</span> {t("chat.target")} [TARGET]
          </div>
          <Select
            value={target.mode === "single" ? `single:${target.id}` : `group:${target.id}`}
            style={{ width: "100%" }}
            className="target-select"
            onChange={(v) => void changeTarget(v)}
            options={[
              {
                label: t("chat.groupLabel"),
                options: groups.map((g) => ({
                  value: `group:${g.id}`,
                  label: `${g.name}（${g.id}${g.builtin ? ` · ${t("chat.builtin")}` : ""}）`,
                })),
              },
              {
                label: t("chat.singlesLabel"),
                options: singles.map((s) => ({ value: `single:${s.identifier}`, label: s.name })),
              },
            ]}
          />
          {target.mode === "single" ? (
            <div className="rail-hint">{t("chat.soloMode")} · <span className="mono">{agentIdEn(target.id)}</span></div>
          ) : (
            activeMeta && (
              <div className="rail-hint">{t("chat.primary")}：<span className="mono">{activeMeta.primary ? agentIdEn(activeMeta.primary) : t("chat.unset")}</span></div>
            )
          )}
        </div>
        <div className="rail-section grow">
          {/* 工具条：搜索 + 类型过滤（桌面端沿用原竖排，见样式表） */}
          <div className="rail-toolbar">
            <Input
              size="small"
              allowClear
              prefix={<SearchOutlined className="rail-search-icon" />}
              placeholder={t("chat.searchSessions")}
              className="session-search"
              value={sessionFilter}
              onChange={(e) => setSessionFilter(e.target.value)}
            />
            {/* 会话类型过滤（2.2）：全部 / 普通 / 通道 */}
            <Select
              value={sessionKind}
              onChange={setSessionKind}
              className="session-kind"
              popupMatchSelectWidth={false}
              options={[
                { value: "all", label: t("chat.kind.all") },
                { value: "normal", label: t("chat.kind.normal") },
                { value: "channel", label: t("chat.kind.channel") },
              ]}
            />
          </div>
          <div className="session-list">
            {sessions.length === 0 && !sessionsLoaded && <SkeletonList rows={6} />}
            {sessionGroups.map((g, gi) => (
              <section className="session-group" key={g.key}>
                <header className="session-group-head">
                  <span className="session-group-left">
                    <span className="session-group-label">{t(g.labelKey)}</span>
                    <span className="session-group-count">{g.items.length}</span>
                  </span>
                  {/* 新建入口跟着第一段走：省一整行控制条，也符合移动端列表的惯例 */}
                  {gi === 0 && (
                    <Button
                      type="text"
                      size="small"
                      icon={<PlusOutlined />}
                      className="rail-new-session"
                      onClick={() => {
                        void newSession();
                        setScreen("thread"); // 竖屏：新建后直接进会话页
                      }}
                    >
                      {t("session.new")}
                    </Button>
                  )}
                </header>
                {g.items.map((s) => (
                  <div
                    key={s.id}
                    className={`session-row ${s.id === sessionId ? "active" : ""}`}
                    onClick={() => {
                      void selectSession(s.id);
                      if (narrow) setScreen("thread"); // 竖屏：点会话才进入该会话页
                    }}
                  >
                    <span className="session-bar" />
                    {/* 头像承载「会话类型」而非组名首字母：副标题已经写了组名，
                     * 首字母与之一一对应（整组同字母）＝零信息；类型恰好是列表里可筛的维度。 */}
                    {(() => {
                      const isChannel = s.title.startsWith("channel:");
                      const isSub = s.title.startsWith("sub:");
                      const subAgent = isSub ? s.title.split(":")[1] : "";
                      const kind = isChannel ? "channel" : isSub ? "sub" : "normal";
                      return (
                        <>
                          <span className={`session-avatar kind-${kind}`} aria-hidden="true">
                            {isChannel ? <CloudUploadOutlined /> : isSub ? <TeamOutlined /> : <MessageOutlined />}
                          </span>
                          <span className="session-main">
                            <span className="session-title">
                              {isChannel
                                ? s.title.replace(/^channel:[^:]+:/, "↗ ")
                                : isSub
                                  ? s.title.replace(/^sub:[^:]+:/, "")
                                  : s.title}
                            </span>
                            <span className="session-sub">
                              {isSub ? `@${subAgent} · ${t("chat.subSession")}` : groupNameOf(s.groupId)}
                            </span>
                          </span>
                        </>
                      );
                    })()}
                    {/* 操作收进「⋮」菜单：此前三个图标常驻，窄屏上既挤占标题又换行 */}
                    <span className="session-ops" onClick={(e) => e.stopPropagation()}>
                      <Dropdown
                        trigger={["click"]}
                        menu={{
                          items: [
                            { key: "export", icon: <ExportOutlined />, label: t("chat.export") },
                            { key: "rename", icon: <EditOutlined />, label: t("common.rename") },
                            { type: "divider" },
                            { key: "delete", icon: <CloseOutlined />, label: t("chat.delete"), danger: true },
                          ],
                          onClick: ({ key, domEvent }) => {
                            domEvent.stopPropagation();
                            if (key === "export") void exportSession(s.id, s.title);
                            else if (key === "rename") setRenaming({ id: s.id, title: s.title });
                            else {
                              modal.confirm({
                                title: t("chat.deleteConfirm"),
                                okText: t("common.confirm"),
                                okButtonProps: { danger: true },
                                cancelText: t("common.cancel"),
                                onOk: () => removeSession(s.id),
                              });
                            }
                          },
                        }}
                      >
                        <Button
                          size="small"
                          type="text"
                          className="session-more"
                          icon={<MoreOutlined />}
                          aria-label={t("chat.moreOps")}
                          onClick={(e) => e.stopPropagation()}
                        />
                      </Dropdown>
                    </span>
                    <span className="session-time">{relativeTime(s.lastActiveAt ?? s.createdAt)}</span>
                  </div>
                ))}
              </section>
            ))}
            {visibleSessions.length === 0 && (
              <div className="rail-empty">
                <p className="dim">{sessionFilter ? t("chat.noMatch") : t("chat.noSessions")}</p>
                {!sessionFilter && (
                  <Button
                    type="primary"
                    icon={<PlusOutlined />}
                    onClick={() => {
                      void newSession();
                      setScreen("thread");
                    }}
                  >
                    {t("session.new")}
                  </Button>
                )}
              </div>
            )}
          </div>
        </div>
      </aside>

      {/* 右侧：消息流 + 输入 */}
      <div className="chat-wrap">
        <div className="console-bar">
          <span className="console-title">{t("nav.chat")}</span>
          <span className="page-en">CHAT</span>
          <span className="console-sep" />
          <span className="readout"><span className="k">MODE</span> <span className="v">{target.mode === "single" ? "SOLO" : "GROUP"}</span></span>
          <span className="readout"><span className="k">STATE</span> <span className={`v${running ? " v-live" : ""}`}>{running ? "RUNNING" : "IDLE"}</span></span>

          {usage && (
            <span className="readout console-usage">
              <span className="k">TOKENS</span>
              {/* 字符进度条：比数字更直观地表达「还剩多少预算」；实测用量不带 ~ 前缀 */}
              <AsciiMeter value={usage.estimate} total={usage.unlimited ? 0 : usage.budget} cells={14} />
              <span className="v">{usage.measured ? usage.estimate : `~${usage.estimate}`}</span>
            </span>
          )}
          {/* 竖屏：返回会话列表页（列表页与本页是两个页面，不是叠加抽屉） */}
          {narrow && (
            <button className="rail-back" onClick={() => setScreen("list")} aria-label={t("chat.sessions")}>
              <ArrowLeftOutlined />
              <span className="rail-back-label">{t("chat.sessionsShort")}</span>
            </button>
          )}
          <span className={`status-led ${wsConnected ? "ok" : "bad"}`} style={{ marginLeft: "auto" }} />
        </div>
        {!wsConnected && <Alert type="warning" message={t("chat.wsDown")} showIcon className="ws-alert app-banner" />}
        {/* 内联审批：命令被拦截时在消息流中直接裁决——批准后输出回灌，模型继续干活 */}
        {approvals.map((a) => (
          <Card key={a.approvalId} size="small" className="approval-card">
            <div className="approval-head">
              <span className="approval-title">⏸ {t("chat.approvalTitle")}</span>
              <span className="approval-agent mono">{a.agentId}</span>
            </div>
            <pre className="approval-cmd">{a.command}</pre>
            <div className="approval-ops">
              <Button
                type="primary"
                size="small"
                icon={<CheckOutlined />}
                onClick={() => void decideApproval(a.approvalId, true)}
              >
                {t("chat.approvalApprove")}
              </Button>
              <Button size="small" danger onClick={() => void decideApproval(a.approvalId, false)}>
                {t("chat.approvalDeny")}
              </Button>
              {a.sessionId && a.sessionId !== sessionId && (
                <Button size="small" type="link" onClick={() => void selectSession(a.sessionId!)}>
                  {t("chat.openSubSession")}
                </Button>
              )}
              <span className="approval-hint">{t("chat.approvalHint")}</span>
            </div>
          </Card>
        ))}
        <div className="chat-scroll">
          {/* 空态：当前会话尚无消息且未在运行 —— 目标捕获盘（与桌面端空态同源母题） */}
          {renderMessages.length === 0 && !running && (
            <div className="chat-acquire">
              <span className="capture-frame chat-acquire-figure">
                <Emblem size={224} />
              </span>
              <div className="chat-acquire-title">{t("chat.acquireTitle")}</div>
              <div className="chat-acquire-desc">{t("chat.acquireDesc")}</div>
              <div className="chat-acquire-readouts">
                <span className="readout"><span className="k">TGT</span><span className="v">{orchLabel}</span></span>
                <span className="readout"><span className="k">GROUP</span><span className="v">{activeMeta?.name ?? activeGroup}</span></span>
                <span className="readout"><span className="k">LINK</span><span className={`v ${wsConnected ? "ok" : "err"}`}>{wsConnected ? "ONLINE" : "RECONN"}</span></span>
              </div>
            </div>
          )}
          {renderMessages.map(({ m, turn }) => (
            <MessageBubble
              key={m.id}
              m={m}
              turn={turn}
              orchLabel={orchLabel}
              ops={
                turn != null && !running ? (
                  <TurnOps turn={turn} onEdit={openEdit} onUndo={doUndo} onFork={doFork} />
                ) : undefined
              }
            />
          ))}

          {/* 等待首个输出：加载卡（一旦有流式输出/子个体产出即让位，避免与输出卡并存） */}
          {running && !liveOrch && liveUnitEntries.length === 0 && (
            <Card size="small" className="msg-bubble msg-agent">
              <Space direction="vertical" className="full-width">
                <div>
                  <Spin size="small" /> <b>{`${orchLabel} …`}</b>
                </div>
                {(liveThinking.trim() || timeline.length > 0 || runToolCalls.length > 0) && (
                  <div className="msg-process">
                    {liveThinking.trim() && (
                      <UsageBar label={t("chat.usedThinking", { n: String(liveThinking.length) })} live>
                        <pre className="usage-think">{liveThinking}</pre>
                      </UsageBar>
                    )}
                    {(timeline.length > 0 || runToolCalls.length > 0) && (
                      <UsageBar label={t("chat.usedTools", { n: runToolCalls.length + timeline.length })} live>
                        <TimelineRows items={timeline} tb={t} onOpenSub={(id) => void selectSession(id)} />
                        {runToolCalls.length > 0 && <ToolCallList calls={runToolCalls} />}
                      </UsageBar>
                    )}
                  </div>
                )}
              </Space>
            </Card>
          )}

          {/* 本轮调度轨迹（收束后保留）：派发/回流条目 + 子会话入口，直到下一次发送或切会话 */}
          {!running && timeline.length > 0 && timelineSession === sessionId && (
            <Card size="small" className="msg-bubble msg-agent turn-timeline-card">
              <div className="msg-process">
                <UsageBar label={t("chat.turnTimeline", { n: String(timeline.length) })} defaultOpen>
                  <TimelineRows items={timeline} tb={t} onOpenSub={(id) => void selectSession(id)} />
                </UsageBar>
              </div>
            </Card>
          )}

          {liveUnitEntries.map(([agent, parts]) => (
            <Card key={agent} size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>{agent}</b> <Tag color="processing">{t("chat.executing")}</Tag>
              </div>
              {/* 思考/工具/输出按发生顺序交错展示（连续工具调用聚合为同一列表） */}
              <ProcessSteps
                parts={parts}
                toolCalls={runToolCalls}
                live
                labelThinking={(n) => t("chat.usedThinking", { n: String(n) })}
                labelTools={(n) => t("chat.usedTools", { n: String(n) })}
              />
            </Card>
          ))}

          {liveOrch && (
            <Card size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>{orchLabel}</b> <Tag color="processing">{t("chat.streaming")}</Tag>
              </div>
              {(liveThinking.trim() || timeline.length > 0 || runToolCalls.length > 0) && (
                <div className="msg-process">
                  {liveThinking.trim() && (
                    <UsageBar label={t("chat.usedThinking", { n: String(liveThinking.length) })} live>
                      <pre className="usage-think">{liveThinking}</pre>
                    </UsageBar>
                  )}
                  {(timeline.length > 0 || runToolCalls.length > 0) && (
                    <UsageBar label={t("chat.usedTools", { n: runToolCalls.length + timeline.length })} live>
                      <TimelineRows items={timeline} tb={t} onOpenSub={(id) => void selectSession(id)} />
                      {runToolCalls.length > 0 && <ToolCallList calls={runToolCalls} />}
                    </UsageBar>
                  )}
                </div>
              )}
              <pre className="live-pre">{liveOrch}</pre>
            </Card>
          )}
          {showBottomBtn && (
            <button
              className="scroll-bottom-btn"
              aria-label={t("chat.scrollToBottom")}
              onClick={() => bottomRef.current?.scrollIntoView({ behavior: "smooth" })}
            >
              ↓
            </button>
          )}
          <div ref={bottomRef} />
        </div>

        <div className="chat-input">
          {images.length > 0 && (
            <div className="attach-row">
              {images.map((src, i) => (
                <span key={i} className="attach-thumb">
                  <img src={src} alt={`附件${i + 1}`} />
                  <Button
                    size="small"
                    type="text"
                    className="attach-del"
                    icon={<CloseOutlined />}
                    onClick={() => setImages(images.filter((_, j) => j !== i))}
                  />
                </span>
              ))}
            </div>
          )}
          <Input.TextArea
            value={text}
            disabled={isSubSession}
            onChange={(e) => {
              setText(e.target.value);
              if (sessionId) useExm.getState().setDraft(sessionId, e.target.value);
            }}
            placeholder={`> ${isSubSession ? t("chat.subSessionReadonly") : narrow ? t("chat.inputPlaceholderNarrow") : t("chat.inputPlaceholder")}`}
            autoSize={{ minRows: 1, maxRows: 6 }}
            onPressEnter={(e) => {
              if (!e.shiftKey) {
                e.preventDefault();
                if (text.trim() && !running && !isSubSession) {
                  doSend();
                }
              }
            }}
            onKeyDown={(e) => {
              // 上/下箭头召回历史用户消息（chat-ergonomics）：仅在空输入时接管
              if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
              if (text.trim()) return;
              const userMsgs = messages.filter((m) => m.role === "user");
              if (userMsgs.length === 0) return;
              e.preventDefault();
              let idx = recallIdxRef.current;
              if (e.key === "ArrowUp") {
                idx = idx === null ? userMsgs.length - 1 : Math.max(0, idx - 1);
              } else {
                if (idx === null) return;
                idx = idx + 1;
                if (idx >= userMsgs.length) {
                  recallIdxRef.current = null;
                  setText("");
                  return;
                }
              }
              recallIdxRef.current = idx;
              const recalled = userMsgs[idx]?.statements.map((st) => st.text).join("\n") ?? "";
              setText(recalled);
            }}
          />
          <label className="attach-btn" title={t("chat.attachTip")}>
            <PaperClipOutlined />
            <input
              type="file"
              accept="image/*"
              multiple
              style={{ display: "none" }}
              onChange={(e) => {
                Array.from(e.target.files ?? []).slice(0, 4).forEach(addImage);
                e.target.value = "";
              }}
            />
          </label>
          <Button
            type="primary"
            icon={<SendOutlined />}
            loading={running}
            disabled={isSubSession}
            onClick={doSend}
          >
            {t("chat.send")}
          </Button>
          {running && sessionId && (
            <Popconfirm
              title={t("chat.stopConfirm")}
              onConfirm={() => {
                if (!sessionId) return;
                api.stopSession(sessionId)
                  .then((r) => message.success(t("chat.stopDone", { n: String(r.toolsExecuted) })))
                  .catch((e) => message.error(t("chat.stopFail", { err: String(e).slice(0, 120) })));
              }}
            >
              <Button danger icon={<PauseCircleOutlined />}>{t("chat.stop")}</Button>
            </Popconfirm>
          )}
          {renaming && (
            <Modal
              open
              title={t("chat.renameTitle")}
              onCancel={() => setRenaming(null)}
              onOk={async () => {
                if (!renaming.title.trim()) return;
                await api.renameSession(renaming.id, renaming.title);
                const store = useExm.getState();
                useExm.setState({ sessions: store.sessions.map((s) => s.id === renaming.id ? { ...s, title: renaming.title } : s) });
                setRenaming(null);
              }}
              okText={t("common.rename")}
            >
              <Input
                value={renaming.title}
                onChange={(e) => setRenaming({ ...renaming, title: e.target.value })}
                placeholder={t("chat.newTitle")}
              />
            </Modal>
          )}
          {editing && (
            <Modal
              open
              title={t("chat.turn.editTitle", { turn: String(editing.turn) })}
              onCancel={() => setEditing(null)}
              onOk={() => void doEdit()}
              okText={t("chat.turn.resend")}
              confirmLoading={historyBusy}
            >
              <Alert
                type="warning"
                showIcon
                message={t("chat.turn.editWarn")}
                className="edit-warn"
              />
              <Input.TextArea
                value={editing.text}
                onChange={(e) => setEditing({ ...editing, text: e.target.value })}
                autoSize={{ minRows: 3, maxRows: 10 }}
              />
            </Modal>
          )}
        </div>
      </div>
    </div>
  );
}