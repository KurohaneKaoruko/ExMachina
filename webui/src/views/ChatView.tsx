/** 对话视图（整合控制台）：左栏 = 组切换 + 会话列表；右侧 = 消息流 + 实时流 + 输入 */
import React, { useCallback, useEffect, useRef, useState } from "react";
import { Alert, Button, Card, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message } from "antd";
import { AudioOutlined, CloseOutlined, EditOutlined, MessageOutlined, PaperClipOutlined, PauseCircleOutlined, PlusOutlined, RobotOutlined, SendOutlined, TeamOutlined, UserOutlined } from "@ant-design/icons";
import { api } from "../api";
import { useExm } from "../store";

interface TargetInfo { mode: "group" | "single"; id: string; name?: string }
import { StatementList } from "../components/Statements";
import { Markdown } from "../components/Markdown";
import type { ChatMessage } from "../types";

function MessageBubble({ m }: { m: ChatMessage }): React.ReactElement {
  const isUser = m.role === "user";
  const title = isUser ? "用户" : m.role === "orchestrator" ? "指挥体" : (m.agentId ?? "系统");
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
    message.success(mode === "single" ? `已切换到智能体：${id}` : `已切换到组：${id}`);
    await loadTarget();
  };
  const [text, setText] = useState("");
  const [images, setImages] = useState<string[]>([]);
  const [sessionFilter, setSessionFilter] = useState("");
  const [renaming, setRenaming] = useState<{ id: string; title: string } | null>(null);
  const [recording, setRecording] = useState(false);
  const [usage, setUsage] = useState<{ estimate: number; budget: number; unlimited: boolean } | null>(null);
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
          const t = await api.transcribe(blob);
          if (t) setText((prev) => (prev ? `${prev} ${t}` : t));
          message.success("语音已转写");
        } catch (e) {
          message.error(`转写失败：${String(e)}`);
        }
      };
      chunksRef.current = [];
      rec.start();
      recorderRef.current = rec;
      setRecording(true);
    } catch {
      message.error("无法访问麦克风");
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
            <span className="rail-no">01</span> 交互目标 [TARGET]
          </div>
          <Select
            value={target.mode === "single" ? `single:${target.id}` : `group:${target.id}`}
            style={{ width: "100%" }}
            onChange={(v) => void changeTarget(v)}
            options={[
              {
                label: "智能体组",
                options: groups.map((g) => ({
                  value: `group:${g.id}`,
                  label: `${g.name}（${g.id}${g.builtin ? " · 内置" : ""}）`,
                })),
              },
              {
                label: "智能体",
                options: singles.map((s) => ({ value: `single:${s.identifier}`, label: s.name })),
              },
            ]}
          />
          {target.mode === "single" ? (
            <div className="rail-hint">智能体模式 · <span className="mono">{target.id}</span></div>
          ) : (
            activeMeta && (
              <div className="rail-hint">主智能体：<span className="mono">{activeMeta.primary ?? "未设"}</span></div>
            )
          )}
        </div>
        <div className="rail-section grow">
          <div className="rail-label">
            <span className="rail-no">02</span> 会话 [SESSIONS]
          </div>
          <Button
            block
            icon={<PlusOutlined />}
            className="new-session-btn"
            onClick={() => void newSession()}
          >
            新会话
          </Button>
          <Input
            size="small"
            allowClear
            placeholder="搜索会话…"
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
                    title="删除该会话及其全部记录？"
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
                {sessionFilter ? "无匹配会话" : "尚无会话"}
              </div>
            )}
          </div>
        </div>
      </aside>

      {/* 右侧：消息流 + 输入 */}
      <div className="chat-wrap">
        <div className="console-bar">
          <span className="console-title">对话</span>
          <span className="page-en">CHAT</span>
          <span className="console-sep" />
          <span className="readout"><span className="k">MODE</span> <span className="v">{target.mode === "single" ? "SOLO" : "GROUP"}</span></span>
          <span className="readout"><span className="k">STATE</span> <span className="v">{running ? "RUNNING" : "IDLE"}</span></span>
          {usage && <span className="readout"><span className="k">TOKENS</span> <span className="v">~{usage.estimate}</span></span>}
          <span className={`status-led ${wsConnected ? "ok" : "bad"}`} style={{ marginLeft: "auto" }} />
        </div>
        {!wsConnected && <Alert type="warning" message="与网关的实时通道断开，重连中…" showIcon className="ws-alert" />}
        <div className="chat-scroll">
          {messages.map((m) => (
            <MessageBubble key={m.id} m={m} />
          ))}

          {running && (
            <Card size="small" className="msg-bubble msg-agent">
              <Space direction="vertical" className="full-width">
                <div>
                  <Spin size="small" /> <b>指挥体运行中</b>
                </div>
                {timeline.map((t, i) => (
                  <div key={i} className={`timeline-item tl-${t.kind}`}>
                    <Tag color={t.kind === "dispatch" ? "blue" : t.kind === "sync" ? "green" : t.kind === "arbitration" ? "volcano" : "red"}>
                      {t.kind === "dispatch" ? "派发" : t.kind === "sync" ? "回流" : t.kind === "arbitration" ? "裁决" : "错误"}
                    </Tag>
                    <span className="tl-text">{t.text}</span>
                  </div>
                ))}
              </Space>
            </Card>
          )}

          {liveUnitEntries.map(([agent, content]) => (
            <Card key={agent} size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>{agent}</b> <Tag color="processing">执行中</Tag>
              </div>
              <pre className="live-pre">{content}</pre>
            </Card>
          ))}

          {liveOrch && (
            <Card size="small" className="msg-bubble msg-agent live-card">
              <div className="msg-head">
                <RobotOutlined /> <b>指挥体</b> <Tag color="processing">输出中</Tag>
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
            placeholder="> 向指挥体下达任务…（Enter 发送，Shift+Enter 换行）"
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
            title={recording ? "停止录音并转写" : "语音输入（转写为文字）"}
            icon={recording ? <PauseCircleOutlined /> : <AudioOutlined />}
            onClick={() => void toggleRecord()}
          />
          <label className="attach-btn" title="附加图片（多模态输入，最多 4 张）">
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
            发送
          </Button>
          {renaming && (
            <Modal
              open
              title="重命名会话"
              onCancel={() => setRenaming(null)}
              onOk={async () => {
                if (!renaming.title.trim()) return;
                await api.renameSession(renaming.id, renaming.title);
                const store = useExm.getState();
                useExm.setState({ sessions: store.sessions.map((s) => s.id === renaming.id ? { ...s, title: renaming.title } : s) });
                setRenaming(null);
              }}
              okText="重命名"
            >
              <Input
                value={renaming.title}
                onChange={(e) => setRenaming({ ...renaming, title: e.target.value })}
                placeholder="新标题"
              />
            </Modal>
          )}
        </div>
      </div>
    </div>
  );
}