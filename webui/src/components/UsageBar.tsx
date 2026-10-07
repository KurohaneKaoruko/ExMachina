/** 过程收起栏（OpenCode 模式）：「使用了 N 个思考 / 使用了 N 个工具」——
 *  默认收起只留一行摘要，点击展开明细；展开状态由调用方受控（流式期间记忆展开选择）。 */
import React, { useState } from "react";

export interface UsageBarProps {
  /** 摘要文案（如「使用了 1 个思考」） */
  label: string;
  /** 展开内容 */
  children: React.ReactNode;
  /** 流式进行中：摘要带呼吸点 */
  live?: boolean;
  /** 受控展开（可选；不传则组件自持状态） */
  defaultOpen?: boolean;
}

export function UsageBar({ label, children, live, defaultOpen = false }: UsageBarProps): React.ReactElement {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className={`usage-bar ${open ? "open" : ""}`}>
      <button
        className="usage-bar-head"
        onClick={(e) => {
          e.stopPropagation();
          setOpen(!open);
        }}
      >
        <span className={`usage-caret ${open ? "open" : ""}`}>▸</span>
        <span className="usage-label">
          {live && <span className="usage-live-dot" />} {label}
        </span>
      </button>
      {open && <div className="usage-body">{children}</div>}
    </div>
  );
}
