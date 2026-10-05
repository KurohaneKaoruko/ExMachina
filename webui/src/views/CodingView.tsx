/** 编码工作台 —— 对标主流编码 Agent 的页面形态：
 *  左栏会话 ｜ 中栏对话流（思维链 / 工具卡 / diff / 审批） ｜ 右栏工作区（文件树 + 变更清单 + 代码预览）
 *  与聊天页共用同一会话与 WS 引擎，但以「写代码」为中心呈现：工具轨迹优先、文件视角常驻。 */
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button, Card, Empty, Input, Spin, Tag, Tree } from "antd";
import {
  CheckOutlined, CodeOutlined, FileOutlined, FolderOutlined, PlusOutlined,
  ReloadOutlined, SendOutlined, StopOutlined,
} from "@ant-design/icons";
import { fsFile, fsList, type FsEntry } from "../api";
import { useExm } from "../store";
import { useT } from "../i18n/core";
import { ToolCallList } from "../components/ToolCall";
import { Markdown } from "../components/Markdown";
import type { ChatMessage, ToolCallItem } from "../types";

/** 从工具轨迹中提取「本轮动过的文件」：edit / filesystem write + mkdir / terminal 里的路径尽力提取 */
function touchedFiles(calls: ToolCallItem[]): { path: string; op: string }[] {
  const out = new Map<string, string>();
  for (const c of calls) {
    const a = c.args as Record<string, string | undefined>;
    if ((c.tool === "edit" || c.tool === "read") && a.path) {
      if (c.tool === "edit") out.set(a.path, "edit");
    } else if (c.tool === "filesystem" && a.path) {
      if (a.op === "write" || a.op === "mkdir") out.set(a.path, a.op);
    } else if (c.tool === "terminal" && a.command) {
      // 尽力从命令里挑出 .xxx 文件路径（回灌展示够用，不求完备）
      for (const m of String(a.command).matchAll(/[\w./\\-]+\.\w{1,6}\b/g)) {
        const p = m[0].replace(/\\/g, "/");
        if (!out.has(p) && !p.startsWith("http")) out.set(p, "cmd");
      }
    }
  }
  return [...out].map(([path, op]) => ({ path, op }));
}

/** 会话的全部工具轨迹：当前运行中的 + 已收束消息附带的 */
function sessionToolCalls(messages: ChatMessage[], runToolCalls: ToolCallItem[]): ToolCallItem[] {
  const finished = messages.flatMap((m) => m.toolCalls ?? []);
  return [...finished, ...runToolCalls];
}

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

/** 代码预览（只读，带行号） */
function CodePreview({ path }: { path: string }): React.ReactElement {
  const t = useT();
  const [state, setState] = useState<{ loading: boolean; content: string; truncated: boolean }>({
    loading: true,
    content: "",
    truncated: false,
  });
  useEffect(() => {
    let alive = true;
    setState((s) => ({ ...s, loading: true }));
    void fsFile(path)
      .then((r) => {
        if (!alive) return;
        setState({ loading: false, content: r.content, truncated: r.truncated });
      })
      .catch(() => alive && setState({ loading: false, content: "", truncated: false }));
    return () => {
      alive = false;
    };
  }, [path]);
  return (
    <div className="code-preview">
      <div className="panel-head">
        <span className="panel-title mono">{path}</span>
      </div>
      <div className="panel-body">
        {state.loading ? (
          <Spin size="small" />
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

export function CodingView(): React.ReactElement {
  const t = useT();
  const {
    messages, liveOrch, liveThinking, liveUnits, runToolCalls, approvals, timeline, running,
    send, wsConnected, sessions, sessionId, newSession, selectSession, decideApproval,
  } = useExm();
  const [text, setText] = useState("");
  const [previewPath, setPreviewPath] = useState<string | null>(null);
  const [panel, setPanel] = useState<"files" | "changes">("files");
  const bottomRef = useRef<HTMLDivElement>(null);

  const changes = useMemo(() => touchedFiles(sessionToolCalls(messages, runToolCalls)), [messages, runToolCalls]);
  const liveUnitEntries = Object.entries(liveUnits).filter(([, v]) => v);

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
          <span className="readout"><span className="k">CHANGES</span> <span className="v">{changes.length}</span></span>
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

      {/* 右栏：工作区（文件树 / 变更清单 + 代码预览） */}
      <aside className="code-side">
        <div className="code-side-tabs">
          <button className={`side-tab ${panel === "files" ? "active" : ""}`} onClick={() => setPanel("files")}>
            {t("code.files")}
          </button>
          <button className={`side-tab ${panel === "changes" ? "active" : ""}`} onClick={() => setPanel("changes")}>
            {t("code.changes")} {changes.length > 0 && <Tag className="changes-count">{changes.length}</Tag>}
          </button>
        </div>
        {panel === "files" ? (
          <FileTree onOpenFile={(p) => setPreviewPath(p)} />
        ) : (
          <div className="fs-tree">
            <div className="panel-head"><span className="panel-title">{t("code.changesTitle")}</span></div>
            <div className="panel-body">
              {changes.length === 0 ? (
                <Empty description={t("code.noChanges")} image={Empty.PRESENTED_IMAGE_SIMPLE} />
              ) : (
                changes.map((c) => (
                  <div key={c.path} className="change-row" onClick={() => setPreviewPath(c.path)}>
                    <span className={`change-op change-${c.op}`}>{c.op}</span>
                    <span className="mono change-path">{c.path}</span>
                  </div>
                ))
              )}
            </div>
          </div>
        )}
        {previewPath && <CodePreview path={previewPath} />}
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
