/** 品牌徽记 —— 中枢环 + 四向连结弧 + 核心方块（智能体环绕中枢运转）。
 *
 *  纯 SVG + CSS 动画（transform/opacity only）：
 *  - 四弧组慢旋（emblem-rotor），虚线轨道反向慢旋（emblem-orbit）
 *  - 中枢环呼吸（emblem-breathe），核心方块 LED 脉冲（复用 led-pulse）
 *  - animated=false 时全部静止；prefers-reduced-motion 下全局降级
 *
 *  颜色走 currentColor：父级给 color 即可联动强调色体系。
 *
 *  ── 几何口径（与 public/icon.svg 同一套比例，只是 512 口径并补了中枢环） ──
 *  四弧：r=150，跨 70°，缺口居中于正交轴（弧心 45/135/225/315）；
 *        线宽 30 —— 即 线宽:半径 = 0.20，与 icon.svg 的 7:38 = 0.184 同量级。
 *  中枢环：r=78，线宽 22（外缘 89，与四弧内缘 135 之间留 46 的呼吸，即一个弧宽以上）。
 *  核心方块：56（icon.svg 的 14:128 等比放大），落在中枢环内孔（r=67）里。
 *  弧的不透明度自右上起顺时针递减 0.95 → 0.35：方向感来自明度阶梯（同 icon.svg）。
 *
 *  ⚠ 历史坑：此处曾用 r=150 / 线宽 44（0.293）。比值放大 1.6 倍后负空间被吃掉，
 *  缺口宽度只剩线宽的 1.2 倍，四弧粘连成「一块带缺口的圆饼」，小尺寸下更糊成一团。
 *  改比值时务必回到 icon.svg 量一遍，别再凭手感加粗。
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
  // 四弧：弧心 45/135/225/315（缺口正对上下左右），明度自右上起顺时针递减
  const arcs: Array<[number, number]> = [
    [45, 0.95],
    [135, 0.75],
    [225, 0.55],
    [315, 0.35],
  ];
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
        {arcs.map(([mid, opacity]) => (
          <path key={mid} d={arcPath(150, mid - 35, mid + 35)} strokeWidth={30} opacity={opacity} />
        ))}
      </g>
      {/* 中枢环 + 核心 LED（呼吸） */}
      <g className="emblem-core">
        <circle cx={256} cy={256} r={78} fill="none" stroke="currentColor" strokeWidth={22} opacity={0.95} />
        <rect className="emblem-core-dot" x={228} y={228} width={56} height={56} fill="currentColor" />
      </g>
    </svg>
  );
}
