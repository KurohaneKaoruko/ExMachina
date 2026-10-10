/** 品牌徽记 —— 中枢环 + 四向连结弧（几何同 icons/icon.svg：智能体环绕中枢运转）。
 *
 *  纯 SVG + CSS 动画（transform/opacity only）：
 *  - 四弧组慢旋（emblem-rotor），虚线轨道反向慢旋（emblem-orbit）
 *  - 中枢环呼吸（emblem-breathe），核心方块 LED 脉冲（复用 led-pulse）
 *  - animated=false 时全部静止；prefers-reduced-motion 下全局降级
 *
 *  颜色走 currentColor：父级给 color 即可联动强调色体系。
 */
import React from "react";

/** 极坐标弧段（0° = 正上，顺时针）。EXMACHINA 图标口径：四段 70° 弧，正交方向留 20° 缺口 */
function arcPath(r: number, a0: number, a1: number): string {
  const rad = (d: number) => ((d - 90) * Math.PI) / 180;
  const x0 = 256 + r * Math.cos(rad(a0));
  const y0 = 256 + r * Math.sin(rad(a0));
  const x1 = 256 + r * Math.cos(rad(a1));
  const y1 = 256 + r * Math.sin(rad(a1));
  return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${r} ${r} 0 0 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}

export interface EmblemProps {
  /** 渲染边长（px） */
  size?: number;
  /** 开启旋转 / 呼吸动效（默认开） */
  animated?: boolean;
  /** 幽灵态：空态淡纹，整体压暗且不吸引视线 */
  faint?: boolean;
  className?: string;
}

export function Emblem({
  size = 96,
  animated = true,
  faint = false,
  className,
}: EmblemProps): React.ReactElement {
  // 四弧：对角线方向各一段 70° 弧（45/135/225/315 为弧心，正交向留缺口）
  const arcs = [315, 45, 135, 225].map((mid) => arcPath(150, mid - 35, mid + 35));
  // 外圈刻度：每 7.5° 一刻，每 30° 一长刻（量度感）
  const ticks = Array.from({ length: 48 }, (_, i) => i * 7.5);
  return (
    <svg
      className={`emblem${animated ? " emblem-animated" : ""}${faint ? " emblem-faint" : ""}${className ? ` ${className}` : ""}`}
      viewBox="0 0 512 512"
      width={size}
      height={size}
      aria-hidden="true"
      focusable="false"
    >
      {/* 外圈刻度环（静）：长刻主线 + 短刻辅线 */}
      <g stroke="currentColor" fill="none">
        {ticks.map((deg) => {
          const major = deg % 30 === 0;
          const rad = ((deg - 90) * Math.PI) / 180;
          const r1 = 210;
          const r2 = major ? 226 : 218;
          return (
            <line
              key={deg}
              x1={256 + Math.cos(rad) * r1}
              y1={256 + Math.sin(rad) * r1}
              x2={256 + Math.cos(rad) * r2}
              y2={256 + Math.sin(rad) * r2}
              strokeWidth={major ? 3 : 1.6}
              opacity={major ? 0.34 : 0.16}
            />
          );
        })}
      </g>
      {/* 虚线轨道（反向慢旋）：表示「连结在持续建立」 */}
      <circle
        className="emblem-orbit"
        cx={256}
        cy={256}
        r={188}
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeDasharray="3 14"
        opacity={0.3}
      />
      {/* 四向连结弧（正向慢旋）：智能体 */}
      <g className="emblem-rotor" fill="none" stroke="currentColor">
        {arcs.map((d, i) => (
          <path key={i} d={d} strokeWidth={44} opacity={0.92 - i * 0.14} />
        ))}
      </g>
      {/* 中枢环 + 核心 LED（呼吸） */}
      <g className="emblem-core">
        <circle cx={256} cy={256} r={72} fill="none" stroke="currentColor" strokeWidth={46} opacity={0.95} />
        <rect className="emblem-core-dot" x={242} y={242} width={28} height={28} fill="currentColor" />
      </g>
    </svg>
  );
}
