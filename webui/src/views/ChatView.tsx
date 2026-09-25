/** 对话视图（整合控制台）：左栏 = 组切换 + 会话列表；右侧 = 消息流 + 实时流 + 输入 */
import React, { useCallback, useEffect, useRef, useState } from "react";
import { Alert, Button, Card, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message } from "antd";
import { AudioOutlined, CloseOutlined, EditOutlined, MessageOutlined, PaperClipOutlined, PauseCircleOutlined, PlusOutlined, RobotOutlined, SendOutlined, TeamOutlined, UserOutlined } from "@ant-design/icons";
import { api } from "../api";
import { agentIdEn } from "../models";
import { useExm } from "../store";
import { useT } from "../i18n/core";

interface TargetInfo { mode: "group" | "single"; id: string; name?: string }
import { StatementList } from "../components/Statements";
import { Markdown } from "../components/Markdown";
import { AsciiMeter } from "../components/Ascii";
import type { ChatMessage } from "../types";

function MessageBubble({ m }: { m: ChatMessage }): React.ReactElement {
  const tb = useT();
  const isUser = m.role === "user";
  const title = isUser ? tb("chat.role.user") : m.role === "orchestrator" ? tb("chat.role.orchestrator") : (m.agentId ?? tb("chat.role.system"));
  return (
    <Card size="small" className={`msg-bubble ${isUser ? "msg-user" : "msg-agent"}`}>
      <div className="msg-head">
        {isUser ? <UserOutlined /> : <RobotOutlined />} <b>{title}</b>
      </div>
      <Markdown text={m.statements.map((s) => s.text).join("\n\n")} />
    </Card>
  );
}

export function ChatView(): React.ReactElement {
  const t = useT();
  const {
    messages, liveOrch, liveUnits, timeline, running, send, wsConnected,
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
  }, [messages, liveOrch, liveUnits, timeline]);

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
        <div className="chat-scroll">
          {messages.map((m) => (
            <MessageBubble key={m.id} m={m} />
          ))}

          {running && (
            <Card size="small" className="msg-bubble msg-agent">
              <Space direction="vertical" className="full-width">
                <div>
                  <Spin size="small" /> <b>{t("chat.orchRunning")}</b>
                </div>
                {timeline.map((tl, i) => (
                  <div key={i} className={`timeline-item tl-${tl.kind}`}>
                    <Tag color={tl.kind === "dispatch" ? "blue" : tl.kind === "sync" ? "green" : tl.kind === "arbitration" ? "volcano" : "red"}>
                      {tl.kind === "dispatch" ? t("chat.tl.dispatch") : tl.kind === "sync" ? t("chat.tl.sync") : tl.kind === "arbitration" ? t("chat.tl.arbitration") : t("chat.tl.error")}
                    </Tag>
                    <span className="tl-text">{tl.text}</span>
                  </div>
                ))}
              </Space>
            </Card>
          )}

          {liveUnitEntries.map(([agent, content]) => (
            <Card key={agent} size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>{agent}</b> <Tag color="processing">{t("chat.executing")}</Tag>
              </div>
              <pre className="live-pre">{content}</pre>
            </Card>
          ))}

          {liveOrch && (
            <Card size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>{t("chat.role.orchestrator")}</b> <Tag color="processing">{t("chat.streaming")}</Tag>
              </div>
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
        </div>
      </div>
    </div>
  );
}