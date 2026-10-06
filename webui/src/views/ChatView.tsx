/** 对话视图（整合控制台）：左栏 = 组切换 + 会话列表；右侧 = 消息流 + 实时流 + 输入 */
import React, { useCallback, useEffect, useRef, useState } from "react";
import { Alert, Button, Card, Input, Modal, Popconfirm, Select, Space, Spin, Tag, Tooltip, message } from "antd";
import { AudioOutlined, CheckOutlined, CloseOutlined, EditOutlined, MessageOutlined, PaperClipOutlined, PauseCircleOutlined, PlusOutlined, RobotOutlined, SendOutlined, TeamOutlined, UserOutlined } from "@ant-design/icons";
import { api } from "../api";
import { agentIdEn } from "../models";
import { useExm } from "../store";
import { useT } from "../i18n/core";

interface TargetInfo { mode: "group" | "single"; id: string; name?: string }
import { StatementList } from "../components/Statements";
import { Markdown } from "../components/Markdown";
import { AsciiMeter } from "../components/Ascii";
import { ToolCallList } from "../components/ToolCall";
import { TurnOps } from "../components/HistoryOps";
import type { ChatMessage } from "../types";

function MessageBubble({ m, turn, ops, orchLabel, showTools }: { m: ChatMessage; turn?: number; ops?: React.ReactNode; orchLabel?: string; showTools?: boolean }): React.ReactElement {
  const tb = useT();
  const isUser = m.role === "user";
  const title = isUser ? tb("chat.role.user") : m.role === "orchestrator" ? (orchLabel ?? tb("chat.role.orchestrator")) : (m.agentId ?? tb("chat.role.system"));
  return (
    <Card size="small" className={`msg-bubble ${isUser ? "msg-user" : "msg-agent"}`}>
      <div className="msg-head">
        {isUser ? <UserOutlined /> : <RobotOutlined />} <b>{title}</b>
        {turn != null && <span className="msg-turn mono">#{turn}</span>}
        {ops && <span className="msg-ops">{ops}</span>}
      </div>
      {showTools && m.toolCalls && m.toolCalls.length > 0 && <ToolCallList calls={m.toolCalls} />}
      <Markdown text={m.statements.map((s) => s.text).join("\n\n")} />
    </Card>
  );
}

/** 折叠思维流：分轨呈现「思考」，默认收起、流式时显示最新一行预览 */
function ThinkingBlock({ text, label }: { text: string; label: string }): React.ReactElement {
  const [open, setOpen] = useState(false);
  if (!text.trim()) return <></>;
  const preview = text.trimEnd().split("\n").pop() ?? "";
  return (
    <div className="thinking-block" onClick={() => setOpen(!open)}>
      <div className="thinking-head">
        <span className="thinking-dot" /> {label}
        {!open && <span className="thinking-preview">{preview.slice(-60)}</span>}
      </div>
      {open && <pre className="thinking-body">{text}</pre>}
    </div>
  );
}

export function ChatView(): React.ReactElement {
  const t = useT();
  const {
    messages, liveOrch, liveThinking, liveUnits, liveUnitThinking, runToolCalls, approvals,
    timeline, running, send, wsConnected, decideApproval,
    sessions, sessionId, selectSession, newSession,
    groups, activeGroup, setTarget,
    refreshAgents,
  } = useExm();
  const [target, setTargetInfo] = useState<TargetInfo>({ mode: "group", id: activeGroup });
  const [singles, setSinglesList] = useState<{ identifier: string; name: string }[]>([]);

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
  /** 编码模式：展开思维链与文件/命令操作轨迹；关闭 = 普通聊天（只看结论输出） */
  const [codeMode, setCodeMode] = useState(() => localStorage.getItem("exm.codeMode") === "1");
  const toggleCodeMode = (v: boolean) => {
    setCodeMode(v);
    localStorage.setItem("exm.codeMode", v ? "1" : "0");
  };
  const [images, setImages] = useState<string[]>([]);
  const [sessionFilter, setSessionFilter] = useState("");
  const [renaming, setRenaming] = useState<{ id: string; title: string } | null>(null);
  const [recording, setRecording] = useState(false);
  const [usage, setUsage] = useState<{
    estimate: number;
    budget: number;
    unlimited: boolean;
    /** provider 上报的真实用量可用（否则为字符估算） */
    measured?: boolean;
  } | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  const chunksRef = useRef<Blob[]>([]);

  // 语音输入：MediaRecorder → 网关转写 → 文本入输入框
  const toggleRecord = async () => {
    if (recording) {
      recorderRef.current?.stop();
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const rec = new MediaRecorder(stream, { mimeType: "audio/webm" });
      rec.ondataavailable = (ev) => {
        if (ev.data.size > 0) chunksRef.current.push(ev.data);
      };
      rec.onstop = async () => {
        stream.getTracks().forEach((t) => t.stop());
        setRecording(false);
        const blob = new Blob(chunksRef.current, { type: "audio/webm" });
        chunksRef.current = [];
        if (blob.size === 0) return;
        try {
          const tr = await api.transcribe(blob);
          if (tr) setText((prev) => (prev ? `${prev} ${tr}` : tr));
          message.success(t("chat.transcribed"));
        } catch (e) {
          message.error(t("chat.transcribeFailed", { err: String(e) }));
        }
      };
      chunksRef.current = [];
      rec.start();
      recorderRef.current = rec;
      setRecording(true);
    } catch {
      message.error(t("chat.micDenied"));
    }
  };

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

  const doSend = () => {
    if (text.trim()) {
      void send(text, images);
      setText("");
      setImages([]);
    }
  };

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, liveOrch, liveUnits, runToolCalls, timeline]);

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

  const liveUnitEntries = Object.entries(liveUnits).filter(([, v]) => v);
  const activeMeta = groups.find((g) => g.id === activeGroup);
  // 指挥体显示名（10）：单体模式显示该智能体名；组模式显示「指挥体 [组名]」
  const orchLabel = target.mode === "single"
    ? (singles.find((x) => x.identifier === target.id)?.name ?? target.id)
    : t("chat.orchGroup", { group: activeMeta?.name ?? target.id });

  const removeSession = async (id: string) => {
    try {
      await api.deleteSession(id);
      const rest = sessions.filter((s) => s.id !== id);
      if (id === sessionId) {
        useExm.setState({ sessionId: undefined, messages: [], graph: null });
        if (rest.length > 0) {
          await selectSession(rest[0].id);
        } else {
          await newSession();
        }
      } else {
        useExm.setState({ sessions: rest });
      }
    } catch {
      // 删除失败静默：会话可能在运行中被网关拒绝
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
    <div className="chat-shell">
      {/* 左栏：目标切换 + 会话列表 */}
      <aside className="chat-rail">
        <div className="rail-section">
          <div className="rail-label">
            <span className="rail-no">01</span> {t("chat.target")} [TARGET]
          </div>
          <Select
            value={target.mode === "single" ? `single:${target.id}` : `group:${target.id}`}
            style={{ width: "100%" }}
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
          <div className="rail-label">
            <span className="rail-no">02</span> {t("chat.sessions")} [SESSIONS]
          </div>
          <Button
            block
            icon={<PlusOutlined />}
            className="new-session-btn"
            onClick={() => void newSession()}
          >
            {t("session.new")}
          </Button>
          <Input
            size="small"
            allowClear
            placeholder={t("chat.searchSessions")}
            className="session-search"
            value={sessionFilter}
            onChange={(e) => setSessionFilter(e.target.value)}
          />
          <div className="session-list">
            {sessions
              .filter((s) => !sessionFilter || s.title.toLowerCase().includes(sessionFilter.toLowerCase()))
              .map((s) => (
              <div
                key={s.id}
                className={`session-row ${s.id === sessionId ? "active" : ""}`}
                onClick={() => void selectSession(s.id)}
              >
                <MessageOutlined className="session-row-icon" />
                <span className="session-title">{s.title}</span>
                {s.id === sessionId && running && <span className="session-live-dot" />}
                <span className="session-ops" onClick={(e) => e.stopPropagation()}>
                  <Button
                    size="small"
                    type="text"
                    className="session-edit"
                    icon={<EditOutlined />}
                    onClick={(e) => {
                      e.stopPropagation();
                      setRenaming({ id: s.id, title: s.title });
                    }}
                  />
                  <Popconfirm
                    title={t("chat.deleteConfirm")}
                    onConfirm={(e) => {
                      e?.stopPropagation();
                      void removeSession(s.id);
                    }}
                    onCancel={(e) => e?.stopPropagation()}
                  >
                    <Button
                      size="small"
                      type="text"
                      className="session-del"
                      icon={<CloseOutlined />}
                      onClick={(e) => e.stopPropagation()}
                    />
                  </Popconfirm>
                </span>
              </div>
            ))}
            {sessions.filter((s) => !sessionFilter || s.title.toLowerCase().includes(sessionFilter.toLowerCase())).length === 0 && (
              <div className="dim" style={{ padding: "12px 8px", fontSize: 12 }}>
                {sessionFilter ? t("chat.noMatch") : t("chat.noSessions")}
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
          <span className="readout"><span className="k">STATE</span> <span className="v">{running ? "RUNNING" : "IDLE"}</span></span>
          <Tooltip title={t("chat.mode.codeTip")}>
            <button
              className={`mode-toggle ${codeMode ? "on" : ""}`}
              onClick={() => toggleCodeMode(!codeMode)}
              title={t("chat.mode.codeTip")}
            >
              {t("chat.mode.code")}
            </button>
          </Tooltip>
          {usage && (
            <span className="readout console-usage">
              <span className="k">TOKENS</span>
              {/* 字符进度条：比数字更直观地表达「还剩多少预算」；实测用量不带 ~ 前缀 */}
              <AsciiMeter value={usage.estimate} total={usage.unlimited ? 0 : usage.budget} cells={14} />
              <span className="v">{usage.measured ? usage.estimate : `~${usage.estimate}`}</span>
            </span>
          )}
          <span className={`status-led ${wsConnected ? "ok" : "bad"}`} style={{ marginLeft: "auto" }} />
        </div>
        {!wsConnected && <Alert type="warning" message={t("chat.wsDown")} showIcon className="ws-alert" />}
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
              <span className="approval-hint">{t("chat.approvalHint")}</span>
            </div>
          </Card>
        ))}
        <div className="chat-scroll">
          {renderMessages.map(({ m, turn }) => (
            <MessageBubble
              key={m.id}
              m={m}
              turn={turn}
              orchLabel={orchLabel}
              showTools={codeMode}
              ops={
                turn != null && !running ? (
                  <TurnOps turn={turn} onEdit={openEdit} onUndo={doUndo} onFork={doFork} />
                ) : undefined
              }
            />
          ))}

          {running && (
            <Card size="small" className="msg-bubble msg-agent">
              <Space direction="vertical" className="full-width">
                <div>
                  <Spin size="small" /> <b>{codeMode ? t("chat.orchRunning") : `${orchLabel} …`}</b>
                </div>
                {codeMode && timeline.map((tl, i) => (
                  <div key={i} className={`timeline-item tl-${tl.kind}`}>
                    <Tag color={tl.kind === "dispatch" ? "blue" : tl.kind === "sync" ? "green" : tl.kind === "arbitration" ? "volcano" : "red"}>
                      {tl.kind === "dispatch" ? t("chat.tl.dispatch") : tl.kind === "sync" ? t("chat.tl.sync") : tl.kind === "arbitration" ? t("chat.tl.arbitration") : t("chat.tl.error")}
                    </Tag>
                    <span className="tl-text">{tl.text}</span>
                  </div>
                ))}
                {/* 工具执行轨迹：编码模式可见（文件操作 / 终端命令 / 截屏） */}
                {codeMode && <ToolCallList calls={runToolCalls} />}
              </Space>
            </Card>
          )}

          {liveUnitEntries.map(([agent, content]) => (
            <Card key={agent} size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>{agent}</b> <Tag color="processing">{t("chat.executing")}</Tag>
              </div>
              {codeMode && <ThinkingBlock text={liveUnitThinking[agent] ?? ""} label={t("chat.thinking")} />}
              <pre className="live-pre">{content}</pre>
            </Card>
          ))}

          {liveOrch && (
            <Card size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>{orchLabel}</b> <Tag color="processing">{t("chat.streaming")}</Tag>
              </div>
              {codeMode && <ThinkingBlock text={liveThinking} label={t("chat.thinking")} />}
              <pre className="live-pre">{liveOrch}</pre>
            </Card>
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
            onChange={(e) => setText(e.target.value)}
            placeholder={`> ${t("chat.inputPlaceholder")}`}
            autoSize={{ minRows: 1, maxRows: 6 }}
            onPressEnter={(e) => {
              if (!e.shiftKey) {
                e.preventDefault();
                if (text.trim() && !running) {
                  doSend();
                }
              }
            }}
          />
          <Button
            size="small"
            type={recording ? "primary" : "text"}
            className="mic-btn"
            danger={recording}
            title={recording ? t("chat.stopRecord") : t("chat.startRecord")}
            icon={recording ? <PauseCircleOutlined /> : <AudioOutlined />}
            onClick={() => void toggleRecord()}
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
            onClick={doSend}
          >
            {t("chat.send")}
          </Button>
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