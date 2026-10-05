/** 工具执行轨迹卡：编程软件式过程透明——文件操作 / 终端命令 / 截屏全程可见 */
import React, { useState } from "react";
import { Tag } from "antd";
import {
  BugOutlined, CheckCircleOutlined, CloseCircleOutlined, CodeOutlined,
  FileTextOutlined, FolderOutlined, GlobalOutlined, Loading3QuartersOutlined,
  SearchOutlined, DesktopOutlined, ClockCircleOutlined, ThunderboltOutlined,
} from "@ant-design/icons";
import { assetUrl } from "../api";
import type { ToolCallItem } from "../types";

const TOOL_ICON: Record<string, React.ReactNode> = {
  read: <FileTextOutlined />,
  edit: <CodeOutlined />,
  grep: <SearchOutlined />,
  glob: <SearchOutlined />,
  filesystem: <FolderOutlined />,
  terminal: <ThunderboltOutlined />,
  web_search: <GlobalOutlined />,
  web_fetch: <GlobalOutlined />,
  browser: <GlobalOutlined />,
  computer: <DesktopOutlined />,
};

/** 工具调用的单行摘要（对齐 OpenCode / Claude Code 的过程行） */
function callTitle(c: ToolCallItem): string {
  const a = c.args as Record<string, string | undefined>;
  switch (c.tool) {
    case "read":
      return `${a.path ?? ""}`;
    case "edit":
      return `${a.path ?? ""}`;
    case "grep":
      return `${a.pattern ?? ""}`;
    case "glob":
      return `${a.pattern ?? ""}`;
    case "filesystem":
      return `${a.op ?? ""} ${a.path ?? ""}`;
    case "terminal":
      return `${a.command ?? a.taskId ?? a.op ?? ""}`;
    case "web_search":
      return `${a.query ?? ""}`;
    case "web_fetch":
      return `${a.url ?? ""}`;
    case "browser":
      return `${a.op ?? ""} ${a.url ?? ""}`;
    case "computer":
      return opTitle(a);
    default:
      return Object.values(a)[0] !== undefined ? String(Object.values(a)[0]).slice(0, 80) : "";
  }
}

function opTitle(a: Record<string, string | undefined>): string {
  const op = a.op ?? "screenshot";
  switch (op) {
    case "click": return `click ${a.double ? "×2 " : ""}${a.button ?? "left"} @ (${a.x ?? "?"}, ${a.y ?? "?"})`;
    case "move": return `move → (${a.x ?? "?"}, ${a.y ?? "?"})`;
    case "scroll": return `scroll dx=${a.dx ?? 0} dy=${a.dy ?? 0}`;
    case "type": return `type「${String(a.text ?? "").slice(0, 30)}」`;
    case "key": return `key ${a.combo ?? ""}`;
    default: return `screenshot${a.monitor ? ` #${a.monitor}` : ""}`;
  }
}

/** edit 的简易 diff 视图（old_string → new_string，按行上色） */
function DiffView({ c }: { c: ToolCallItem }): React.ReactElement | null {
  const a = c.args as Record<string, string | undefined>;
  const oldS = typeof a.old_string === "string" ? a.old_string : "";
  const newS = typeof a.new_string === "string" ? a.new_string : "";
  if (!c.args.old_string && !c.args.new_string) return null;
  const maxLines = 400;
  const oldLines = oldS.split("\n").slice(0, maxLines);
  const newLines = newS.split("\n").slice(0, maxLines);
  return (
    <div className="tool-diff">
      {oldLines.map((l, i) => (
        <div key={`o${i}`} className="diff-line diff-del">- {l}</div>
      ))}
      {newLines.map((l, i) => (
        <div key={`n${i}`} className="diff-line diff-add">+ {l}</div>
      ))}
    </div>
  );
}

/** 单条工具卡：状态图标 + 摘要行 + 可展开详情（参数 / 结果 / diff / 截图） */
export function ToolCallCard({ c }: { c: ToolCallItem }): React.ReactElement {
  const [open, setOpen] = useState(false);
  const icon = TOOL_ICON[c.tool] ?? <BugOutlined />;
  const statusNode =
    c.status === "running" ? (
      <Loading3QuartersOutlined spin className="tool-spin" />
    ) : c.status === "ok" ? (
      <CheckCircleOutlined className="tool-ok" />
    ) : (
      <CloseCircleOutlined className="tool-err" />
    );
  const title = callTitle(c);
  const hasDetail =
    !!c.summary || Object.keys(c.args ?? {}).length > 0 || (c.images?.length ?? 0) > 0;
  const isEdit = c.tool === "edit" || (c.tool === "filesystem" && c.args.op === "write");
  return (
    <div className={`tool-card tool-${c.status}`} onClick={() => hasDetail && setOpen(!open)}>
      <div className="tool-row">
        {statusNode}
        <span className="tool-icon">{icon}</span>
        <span className="tool-name">{c.tool}</span>
        <span className="tool-title">{title}</span>
        {typeof c.durationMs === "number" && c.status !== "running" && (
          <Tag className="tool-ms" icon={<ClockCircleOutlined />}>{c.durationMs}ms</Tag>
        )}
      </div>
      {open && (
        <div className="tool-detail">
          {isEdit && <DiffView c={c} />}
          {c.images && c.images.length > 0 && (
            <div className="tool-shots">
              {c.images.map((p, i) => (
                <img key={i} src={assetUrl(p)} alt={`screenshot-${i + 1}`} className="tool-shot" loading="lazy" />
              ))}
            </div>
          )}
          {Object.keys(c.args ?? {}).length > 0 && !isEdit && (
            <pre className="tool-args">{JSON.stringify(c.args, null, 2).slice(0, 2000)}</pre>
          )}
          {c.summary && <pre className="tool-summary">{c.summary}</pre>}
        </div>
      )}
    </div>
  );
}

/** 工具轨迹列表 */
export function ToolCallList({ calls }: { calls: ToolCallItem[] }): React.ReactElement | null {
  if (!calls.length) return null;
  return (
    <div className="tool-list">
      {calls.map((c) => (
        <ToolCallCard key={c.callId} c={c} />
      ))}
    </div>
  );
}
