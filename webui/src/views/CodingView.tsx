/** 编码工作台 —— 对标主流编码 Agent 的页面形态：
 *  左栏会话 ｜ 中栏对话流（思维链 / 工具卡 / diff / 审批） ｜ 右栏工作区（文件树 + 变更清单 + 代码预览）
 *  与聊天页共用同一会话与 WS 引擎，但以「写代码」为中心呈现：工具轨迹优先、文件视角常驻。 */
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button, Card, Empty, Input, Spin, Tag, Tree, message } from "antd";
import {
  CheckOutlined, CodeOutlined, EditOutlined, FileOutlined, FolderOutlined, PlusOutlined,
  ReloadOutlined, SaveOutlined, SendOutlined, StopOutlined,
} from "@ant-design/icons";
import { fsFile, fsList, fsSave, gitOp, gitOverview, workspaceChanges, type FsEntry, type GitOverview, type WorkspaceChanges } from "../api";
import { useExm } from "../store";
import { useT } from "../i18n/core";
import { ToolCallList } from "../components/ToolCall";
import { Markdown } from "../components/Markdown";
import type { ChatMessage } from "../types";

/** 文件树节点（antd Tree） */
interface TreeNode {
  key: string;
  title: string;
  isLeaf: boolean;
  children?: TreeNode[];
}

/** 懒加载文件树：按目录逐层拉取 */
function FileTree({ onOpenFile }: { onOpenFile: (path: string) => void }): React.ReactElement {
  const t = useT();
  const [tree, setTree] = useState<TreeNode[]>([]);
  const [loaded, setLoaded] = useState<Set<string>>(new Set([""]));
  const [rootName, setRootName] = useState("");

  const loadDir = useCallback(async (path: string): Promise<FsEntry[]> => {
    try {
      const r = await fsList(path || "");
      return r.entries;
    } catch {
      return [];
    }
  }, []);

  const buildNode = useCallback(
    (e: FsEntry): TreeNode => ({
      key: e.path,
      title: e.name,
      isLeaf: !e.dir,
    }),
    [],
  );

  const loadRoot = useCallback(async () => {
    const entries = await loadDir("");
    setRootName(new URLSearchParams(location.search).get("ws") ?? ".");
    setTree(entries.map(buildNode));
  }, [loadDir, buildNode]);

  useEffect(() => {
    void loadRoot();
  }, [loadRoot]);

  const loadChildren = useCallback(
    async (dirPath: string): Promise<TreeNode[]> => {
      const entries = await loadDir(dirPath);
      return entries.map(buildNode);
    },
    [loadDir, buildNode],
  );

  return (
    <div className="fs-tree">
      <div className="panel-head">
        <span className="panel-title">
          <FolderOutlined /> {t("code.workspace")}
        </span>
        <Button size="small" type="text" icon={<ReloadOutlined />} onClick={() => void loadRoot()} />
      </div>
      <div className="panel-body">
        <div className="fs-root mono">{rootName}</div>
        {tree.length === 0 ? (
          <Empty description={t("code.emptyWs")} image={Empty.PRESENTED_IMAGE_SIMPLE} />
        ) : (
          <Tree
            showIcon
            blockNode
            treeData={tree}
            loadData={async (node) => {
              if (loaded.has(String(node.key))) return;
              const children = await loadChildren(String(node.key));
              setLoaded(new Set(loaded).add(String(node.key)));
              setTree((prev) => {
                const walk = (nodes: TreeNode[]): TreeNode[] =>
                  nodes.map((n) =>
                    n.key === node.key ? { ...n, children: children.length ? children : undefined } : { ...n, children: n.children ? walk(n.children) : undefined },
                  );
                return walk(prev);
              });
            }}
            onSelect={(keys) => {
              const k = keys[0];
              if (!k) return;
              const node = tree.find((n) => n.key === k);
              if (node?.isLeaf ?? true) onOpenFile(String(k));
            }}
            icon={({ isLeaf }) => (isLeaf ? <FileOutlined /> : <FolderOutlined />)}
          />
        )}
      </div>
    </div>
  );
}

/** 统一 diff 渲染（着色按行） */
function UnifiedDiff({ text, truncated }: { text: string; truncated?: boolean }): React.ReactElement {
  const t = useT();
  return (
    <pre className="code-body unified-diff">
      {text.split("\n").map((l, i) => {
        const cls = l.startsWith("+") && !l.startsWith("+++")
          ? "diff-add"
          : l.startsWith("-") && !l.startsWith("---")
            ? "diff-del"
            : l.startsWith("@@")
              ? "diff-hunk"
              : "";
        return (
          <div key={i} className={`diff-line ${cls}`}>{l}</div>
        );
      })}
      {truncated && <div className="dim code-trunc">{t("code.truncated")}</div>}
    </pre>
  );
}

/** 代码面板：只读预览 + 可切换编辑保存（保存走服务端检查点/审计同口径） */
function CodePane({ path, onChanged }: { path: string; onChanged?: () => void }): React.ReactElement {
  const t = useT();
  const [state, setState] = useState<{ loading: boolean; content: string; truncated: boolean }>({
    loading: true,
    content: "",
    truncated: false,
  });
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);

  const load = useCallback((p: string) => {
    let alive = true;
    setState((s) => ({ ...s, loading: true }));
    void fsFile(p)
      .then((r) => {
        if (!alive) return;
        setState({ loading: false, content: r.content, truncated: r.truncated });
        setDraft(r.content);
      })
      .catch(() => alive && setState({ loading: false, content: "", truncated: false }));
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    setEditing(false);
    return load(path);
  }, [path, load]);

  const doSave = async () => {
    setSaving(true);
    try {
      await fsSave(path, draft);
      message.success(t("code.saveOk"));
      setEditing(false);
      load(path);
      onChanged?.();
    } catch (e) {
      message.error(`${t("code.saveFail")}: ${String(e).slice(0, 160)}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="code-preview">
      <div className="panel-head">
        <span className="panel-title mono">{path}</span>
        <span className="panel-ops">
          {editing ? (
            <Button size="small" type="primary" icon={<SaveOutlined />} loading={saving} onClick={() => void doSave()}>
              {t("code.save")}
            </Button>
          ) : (
            <Button size="small" type="text" icon={<EditOutlined />} onClick={() => { setDraft(state.content); setEditing(true); }}>
              {t("code.edit")}
            </Button>
          )}
        </span>
      </div>
      <div className="panel-body">
        {state.loading ? (
          <Spin size="small" />
        ) : editing ? (
          <Input.TextArea
            className="code-editor mono"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            autoSize={{ minRows: 16, maxRows: 40 }}
            spellCheck={false}
          />
        ) : (
          <pre className="code-body">
            {state.content.split("\n").map((l, i) => (
              <div key={i} className="code-line">
                <span className="code-ln">{i + 1}</span>
                <span className="code-text">{l}</span>
              </div>
            ))}
            {state.truncated && <div className="dim code-trunc">{t("code.truncated")}</div>}
          </pre>
        )}
      </div>
    </div>
  );
}

/** Git 面板：分支切换 / 提交 / 破坏性操作（走审批单） / 最近提交 */
function GitPanel({ onChanged }: { onChanged: () => void }): React.ReactElement {
  const t = useT();
  const [ov, setOv] = useState<GitOverview | null>(null);
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setOv(await gitOverview());
    } catch {
      setOv(null);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const run = useCallback(
    async (op: string, extra?: { path?: string; message?: string; branch?: string }) => {
      setBusy(true);
      try {
        const r = await gitOp(op, extra);
        if (r.pending) message.info(t("git.approvalQueued"));
        else if (r.error) message.error(r.error);
        else message.success(r.output?.slice(0, 120) || t("code.saveOk"));
        await load();
        onChanged();
      } catch (e) {
        message.error(String(e).slice(0, 160));
      } finally {
        setBusy(false);
      }
    },
    [load, onChanged, t],
  );

  if (ov && !ov.repo) {
    return (
      <div className="fs-tree">
        <div className="panel-body">
          <Empty description={t("git.noRepo")} image={Empty.PRESENTED_IMAGE_SIMPLE} />
        </div>
      </div>
    );
  }
  return (
    <div className="fs-tree">
      <div className="panel-body git-panel">
        <div className="git-row">
          <select
            className="git-branch-select mono"
            value={ov?.branch ?? ""}
            disabled={busy || !ov?.branches?.length}
            onChange={(e) => void run("switch", { branch: e.target.value })}
          >
            {(ov?.branches ?? []).map((b) => (
              <option key={b} value={b}>{b}</option>
            ))}
          </select>
        </div>
        <div className="git-row git-commit">
          <Input
            size="small"
            value={msg}
            placeholder={t("git.commitMsg")}
            onChange={(e) => setMsg(e.target.value)}
            onPressEnter={(e) => {
              if (msg.trim()) {
                e.preventDefault();
                void run("commit", { message: msg }).then(() => setMsg(""));
              }
            }}
          />
          <Button size="small" type="primary" disabled={busy || !msg.trim()} onClick={() => void run("commit", { message: msg }).then(() => setMsg(""))}>
            {t("git.commit")}
          </Button>
        </div>
        <div className="git-row git-danger">
          <Button size="small" danger disabled={busy} onClick={() => void run("push")}>{t("git.push")}</Button>
          <Button size="small" danger disabled={busy} onClick={() => void run("reset_hard")}>{t("git.resetHard")}</Button>
        </div>
        <div className="git-log-title dim">{t("git.log")}</div>
        <div className="git-log mono">
          {(ov?.log ?? []).map((l, i) => (
            <div key={i} className="git-log-line">{l}</div>
          ))}
        </div>
      </div>
    </div>
  );
}

export function CodingView(): React.ReactElement {
  const t = useT();
  const {
    messages, liveOrch, liveThinking, liveUnits, runToolCalls, approvals, timeline, running,
    send, wsConnected, sessions, sessionId, newSession, selectSession, decideApproval,
  } = useExm();
  const [text, setText] = useState("");
  const [previewPath, setPreviewPath] = useState<string | null>(null);
  const [diffPath, setDiffPath] = useState<string | null>(null);
  const [panel, setPanel] = useState<"files" | "changes" | "git">("files");
  const [wsChanges, setWsChanges] = useState<WorkspaceChanges | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  /** 变更清单：工作区实况口径（git HEAD 对比 / 检查点回退），非命令轨迹推断 */
  const refreshChanges = useCallback(async () => {
    try {
      setWsChanges(await workspaceChanges());
    } catch {
      /* 离线/鉴权失效时静默：面板保持上次数据 */
    }
  }, []);
  useEffect(() => {
    void refreshChanges();
  }, [refreshChanges]);
  // 运行收束沿（true→false）自动刷新：智能体改完文件后清单即时跟上
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running) void refreshChanges();
    wasRunning.current = running;
  }, [running, refreshChanges]);

  const liveUnitEntries = Object.entries(liveUnits).filter(([, v]) => v);
  const diffData = useMemo(
    () => wsChanges?.files.find((f) => f.path === diffPath)?.diff ?? null,
    [wsChanges, diffPath],
  );
  const changeCount = wsChanges?.files.length ?? 0;

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, liveOrch, runToolCalls, timeline]);

  const doSend = () => {
    if (!text.trim() || running) return;
    void send(text);
    setText("");
  };

  return (
    <div className="code-shell">
      {/* 左栏：会话 */}
      <aside className="code-rail">
        <div className="panel-head">
          <span className="panel-title">
            <CodeOutlined /> {t("code.sessions")}
          </span>
          <Button size="small" type="text" icon={<PlusOutlined />} onClick={() => void newSession()} title={t("session.new")} />
        </div>
        <div className="panel-body session-list">
          {sessions.map((s) => (
            <div
              key={s.id}
              className={`session-row ${s.id === sessionId ? "active" : ""}`}
              onClick={() => void selectSession(s.id)}
            >
              <span className="session-title">{s.title}</span>
              {s.id === sessionId && running && <span className="session-live-dot" />}
            </div>
          ))}
          {sessions.length === 0 && <Empty description={t("chat.noSessions")} image={Empty.PRESENTED_IMAGE_SIMPLE} />}
        </div>
        <div className="code-rail-foot">
          <span className={`status-led ${wsConnected ? "ok" : "bad"}`} />
          <span className="mono dim">{wsConnected ? "LINKED" : "OFFLINE"}</span>
        </div>
      </aside>

      {/* 中栏：对话流（工具优先呈现） */}
      <section className="code-center">
        <div className="console-bar">
          <span className="console-title">{t("nav.code")}</span>
          <span className="page-en">CODE</span>
          <span className="console-sep" />
          <span className="readout"><span className="k">STATE</span> <span className="v">{running ? "RUNNING" : "IDLE"}</span></span>
          <span className="readout"><span className="k">CHANGES</span> <span className="v">{changeCount}</span></span>
        </div>

        {/* 内联审批 */}
        {approvals.map((a) => (
          <Card key={a.approvalId} size="small" className="approval-card">
            <div className="approval-head">
              <span className="approval-title">⏸ {t("chat.approvalTitle")}</span>
              <span className="approval-agent mono">{a.agentId}</span>
            </div>
            <pre className="approval-cmd">{a.command}</pre>
            <div className="approval-ops">
              <Button type="primary" size="small" icon={<CheckOutlined />} onClick={() => void decideApproval(a.approvalId, true)}>
                {t("chat.approvalApprove")}
              </Button>
              <Button size="small" danger icon={<StopOutlined />} onClick={() => void decideApproval(a.approvalId, false)}>
                {t("chat.approvalDeny")}
              </Button>
              <span className="approval-hint">{t("chat.approvalHint")}</span>
            </div>
          </Card>
        ))}

        <div className="code-scroll">
          {messages.map((m) => (
            <div key={m.id} className={`code-msg ${m.role === "user" ? "code-msg-user" : "code-msg-agent"}`}>
              {m.toolCalls && m.toolCalls.length > 0 && <ToolCallList calls={m.toolCalls} />}
              <Markdown text={m.statements.map((s) => s.text).join("\n\n")} />
            </div>
          ))}

          {running && (
            <div className="code-msg code-msg-agent">
              <div className="code-running">
                <Spin size="small" /> <b>{t("chat.orchRunning")}</b>
              </div>
              <ToolCallList calls={runToolCalls} />
              <ThinkingBlock text={liveThinking} label={t("chat.thinking")} />
              {liveOrch && <pre className="live-pre">{liveOrch}</pre>}
              {liveUnitEntries.map(([agent, content]) => (
                <div key={agent} className="code-unit-live">
                  <Tag color="processing">{agent}</Tag>
                  <pre className="live-pre">{content.slice(-1200)}</pre>
                </div>
              ))}
              {timeline.map((tl, i) => (
                <div key={i} className={`timeline-item tl-${tl.kind}`}>
                  <span className="tl-text dim">{tl.text}</span>
                </div>
              ))}
            </div>
          )}
          <div ref={bottomRef} />
        </div>

        <div className="code-input">
          <Input.TextArea
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={`> ${t("code.placeholder")}`}
            autoSize={{ minRows: 1, maxRows: 8 }}
            onPressEnter={(e) => {
              if (!e.shiftKey) {
                e.preventDefault();
                doSend();
              }
            }}
          />
          <Button type="primary" icon={<SendOutlined />} loading={running} onClick={doSend}>
            {t("chat.send")}
          </Button>
        </div>
      </section>

      {/* 右栏：工作区（文件树 / 变更清单 + 代码面板） */}
      <aside className="code-side">
        <div className="code-side-tabs">
          <button className={`side-tab ${panel === "files" ? "active" : ""}`} onClick={() => setPanel("files")}>
            {t("code.files")}
          </button>
          <button className={`side-tab ${panel === "changes" ? "active" : ""}`} onClick={() => setPanel("changes")}>
            {t("code.changes")} {changeCount > 0 && <Tag className="changes-count">{changeCount}</Tag>}
          </button>
          <button className={`side-tab ${panel === "git" ? "active" : ""}`} onClick={() => setPanel("git")}>
            Git
          </button>
        </div>
        {panel === "git" && <GitPanel onChanged={() => void refreshChanges()} />}
        {panel === "files" ? (
          <FileTree onOpenFile={(p) => { setDiffPath(null); setPreviewPath(p); }} />
        ) : (
          <div className="fs-tree">
            <div className="panel-head">
              <span className="panel-title">
                {t("code.changesTitle")}
                {wsChanges?.source === "git" && wsChanges.branch && (
                  <Tag className="changes-branch mono">{wsChanges.branch}</Tag>
                )}
                {wsChanges?.source === "checkpoint" && changeCount > 0 && (
                  <Tag className="changes-branch" title={t("code.checkpointHint")}>checkpoint</Tag>
                )}
              </span>
              <Button size="small" type="text" icon={<ReloadOutlined />} onClick={() => void refreshChanges()} title={t("code.refresh")} />
            </div>
            <div className="panel-body">
              {changeCount === 0 ? (
                <Empty description={t("code.noChanges")} image={Empty.PRESENTED_IMAGE_SIMPLE} />
              ) : (
                wsChanges!.files.map((f) => (
                  <div
                    key={f.path}
                    className={`change-row ${diffPath === f.path ? "active" : ""}`}
                    onClick={() => setDiffPath(diffPath === f.path ? null : f.path)}
                  >
                    <span className={`change-op change-${f.status.toLowerCase()}`}>{f.status}</span>
                    <span className="mono change-path">{f.path}</span>
                    <span className="change-ops" onClick={(e) => e.stopPropagation()}>
                      {f.staged ? (
                        <Button size="small" type="text" onClick={() => void gitOp("unstage", { path: f.path }).then(refreshChanges)}>
                          {t("git.unstage")}
                        </Button>
                      ) : (
                        <Button size="small" type="text" onClick={() => void gitOp("stage", { path: f.path }).then(refreshChanges)}>
                          {t("git.stage")}
                        </Button>
                      )}
                      <Button
                        size="small"
                        type="text"
                        danger
                        onClick={() => void gitOp("discard", { path: f.path }).then(refreshChanges)}
                      >
                        {t("git.discard")}
                      </Button>
                    </span>
                  </div>
                ))
              )}
              {wsChanges?.source === "checkpoint" && changeCount > 0 && (
                <div className="dim checkpoint-hint">{t("code.checkpointHint")}</div>
              )}
            </div>
            {diffData && <UnifiedDiff text={diffData.text} truncated={diffData.truncated} />}
          </div>
        )}
        {previewPath && <CodePane path={previewPath} onChanged={() => void refreshChanges()} />}
      </aside>
    </div>
  );
}

/** 折叠思维流（与 ChatView 同款，编码页内联副本以保持布局独立） */
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
