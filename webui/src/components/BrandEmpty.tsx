/** 品牌空态 —— 统一替换 antd Empty 默认插画：四弧连结纹淡影 + 引导文案。
 *  徽记用幽灵态（低透明 + 慢旋），给出「系统在线、此处暂无数据」的生命感而非死寂的灰块。 */
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
      <Emblem size={72} faint className="brand-empty-emblem" />
      <div className="brand-empty-desc">{description}</div>
    </div>
  );
}
