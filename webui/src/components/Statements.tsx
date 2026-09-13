/** 陈述渲染：句式前缀着色 + 证据等级徽标 */
import React from "react";
import { Tag } from "antd";
import type { SpeechTag, Statement } from "../types";

const TAG_COLORS: Record<SpeechTag, string> = {
  肯定: "green",
  否定: "red",
  疑问: "gold",
  报告: "cyan",
  提案: "purple",
  警告: "volcano",
  要求: "geekblue",
  观测: "default",
};

const LEVEL_COLORS: Record<string, string> = { A: "red", B: "orange", C: "blue", D: "default" };

export function StatementLine({ s }: { s: Statement }): React.ReactElement {
  return (
    <div className="stmt-line">
      <Tag color={TAG_COLORS[s.tag]} className="stmt-tag">
        {s.tag}
      </Tag>
      <span>{s.text}</span>
      {s.evidenceLevel && (
        <Tag color={LEVEL_COLORS[s.evidenceLevel]} className="stmt-level">
          证据{s.evidenceLevel}
        </Tag>
      )}
    </div>
  );
}

export function StatementList({ statements }: { statements: Statement[] }): React.ReactElement {
  return (
    <div>
      {statements.map((s, i) => (
        <StatementLine key={i} s={s} />
      ))}
    </div>
  );
}
