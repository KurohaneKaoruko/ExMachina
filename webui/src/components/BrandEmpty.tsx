/** 品牌空态 —— 统一替换 antd Empty 默认插画：四弧连结纹淡影 + 引导文案。
 *  徽记用幽灵态（低透明 + 慢旋），给出「系统在线、此处暂无数据」的生命感而非死寂的灰块；
 *  外层再套一圈缓慢自转的径向刻度（.capture-frame），与桌面端空态的「目标捕获」母题同源。 */
import React from "react";
import { Emblem } from "./Emblem";

export function BrandEmpty({
  description,
  className,
}: {
  description?: React.ReactNode;
  className?: string;
}): React.ReactElement {
  return (
    <div className={`brand-empty${className ? ` ${className}` : ""}`}>
      <span className="capture-frame brand-empty-figure">
        {/* 84px：让徽记填到捕获框刻度环的 1.25 倍关系上；72px 时徽记半径小于刻度环半径，读成「小点套大圈」 */}
        <Emblem size={84} faint className="brand-empty-emblem" />
      </span>
      <div className="brand-empty-desc">{description}</div>
    </div>
  );
}
