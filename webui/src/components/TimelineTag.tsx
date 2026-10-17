/** 时间线徽标：kind → 颜色 + 文案。
 *  旧实现用三元链兜底「非派发/回流/裁决 ≈ 错误」，memory（记忆召回）/ cron 因此被误标红。
 *  未知 kind 也走中性色 +「事件」，不再谎报错误。 */
import React from "react";
import { Tag } from "antd";
import { useT } from "../i18n/core";

const TL_COLOR: Record<string, string> = {
  dispatch: "blue",
  sync: "green",
  arbitration: "volcano",
  memory: "purple",
  cron: "cyan",
  error: "red",
};

export function TimelineTag({ kind, ns }: { kind: string; ns: "chat" | "activity" }): React.ReactElement {
  const t = useT();
  const known = Object.prototype.hasOwnProperty.call(TL_COLOR, kind);
  return <Tag color={known ? TL_COLOR[kind] : "default"}>{t(`${ns}.tl.${known ? kind : "event"}`)}</Tag>;
}
