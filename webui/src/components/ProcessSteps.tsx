/** 有序过程步骤渲染：「思考 / 工具 / 输出」按发生顺序交错逐次显示。
 *  连续执行的工具调用保持聚合列表样式（与既有 ToolCallList 一致）；
 *  思考段落与正文段落同类相邻时已由数据源合并，不做二次归并。
 *  数据来源：持久消息的 steps（服务端留存）/ 子会话实时流（store.liveParts）。 */
import React from "react";
import { UsageBar } from "./UsageBar";
import { ToolCallList } from "./ToolCall";
import type { ToolCallItem } from "../types";

export interface ProcessPart {
  kind: "thinking" | "text" | "tool";
  text?: string;
  callId?: string;
}

export interface ProcessStepsProps {
  parts: ProcessPart[];
  /** 工具记录（callId 关联）；缺失记录的步骤跳过渲染 */
  toolCalls: ToolCallItem[];
  /** 实时流式期间：摘要带呼吸点 */
  live?: boolean;
  labelThinking: (chars: number) => string;
  labelTools: (n: number) => string;
}

export function ProcessSteps({ parts, toolCalls, live, labelThinking, labelTools }: ProcessStepsProps): React.ReactElement | null {
  const byId = new Map(toolCalls.map((c) => [c.callId, c]));
  const rows: React.ReactNode[] = [];
  let i = 0;
  while (i < parts.length) {
    const p = parts[i];
    if (p.kind === "tool") {
      // 连续工具调用聚合为一个收起栏（保持既有列表样式）
      const group: ToolCallItem[] = [];
      while (i < parts.length && parts[i].kind === "tool") {
        const rec = byId.get(parts[i].callId ?? "");
        if (rec) group.push(rec);
        i += 1;
      }
      if (group.length > 0) {
        const running = group.some((c) => c.status === "running");
        rows.push(
          <UsageBar key={`tools-${i}`} label={labelTools(group.length)} live={Boolean(live && running)}>
            <ToolCallList calls={group} />
          </UsageBar>,
        );
      }
      continue;
    }
    if (p.kind === "thinking") {
      const text = p.text ?? "";
      if (text.trim()) {
        rows.push(
          <UsageBar key={`think-${i}`} label={labelThinking(text.length)} live={live}>
            <pre className="usage-think">{text}</pre>
          </UsageBar>,
        );
      }
      i += 1;
      continue;
    }
    // 正文段（实时流）：原样呈现，收束后由最终陈述替代
    const text = p.text ?? "";
    if (text) {
      rows.push(
        <pre key={`text-${i}`} className="live-pre">
          {text}
        </pre>,
      );
    }
    i += 1;
  }
  if (rows.length === 0) return null;
  return <div className="msg-process">{rows}</div>;
}
