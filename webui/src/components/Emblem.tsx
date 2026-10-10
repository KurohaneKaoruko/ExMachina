/** 品牌徽记 —— 中枢环 + 外观弧槽（与 desktop/ui/mark.js 同一套几何，400 口径）。
 *
 *  ── 几何口径（改之前先回 mark.js 对齐，别只改这一处） ──
 *  画布 400×400，圆心 (200,200)，全部线宽 = 24.5（线宽:半径 ≈ 0.186）。
 *  中枢环：r = 56.5（内缘 44.25 / 外缘 68.75）。
 *  外圈 8 个 45° 槽位，边界落在 22.5° + k·45°：
 *      0° / 180°       → 远弧 r = 131.5
 *      45/135/225/315° → 近弧 r = 107
 *      90° / 270°      → 空槽（上下各一个缺口）
 *  远弧 − 近弧 = 24.5 = 一个线宽 → **近弧外缘与远弧内缘共线于 r = 119.25**，
 *  相邻槽位在 22.5° 处共用径向切边，连成阶梯状的整块；全部弧与环同亮度（无明度阶梯）。
 *
 *  ── 动效（全部不旋转）──
 *  旋转会让徽记的缺口相位一直在变，读不成图形 —— 这是「加载页别自转」的由来。
 *    animated（常态）：整枚徽记缓慢呼吸，4.2s 一拍，用于空态 / 登录页等陪伴态。
 *    charging（加载态）：同一种呼吸，1.9s 一拍，用于开屏「连接中」与视图懒加载兜底。
 *  两种都**整枚同拍**（弧槽与中枢环共用同一条 keyframes、同周期、零延迟），
 *  因此任一时刻全部笔画灰度一致。曾经按槽位序号错开相位，静帧里八段弧的灰度
 *  会散成 61/158/199/208/254/255 —— 徽记在加载态读不出原形，已废弃。
 *  只走 transform / opacity；prefers-reduced-motion 下全局降级为静止。
 *
 *  颜色走 currentColor：父级给 color 即可联动强调色体系。
 */
import React from "react";

const MARK = {
  view: 400,
  c: 200,
  ring: { r: 56.5, w: 24.5 },
  arcW: 24.5,
  span: 45,
  /** 8 个 45° 槽位，按角度递增排列；r = null 即空槽。下标 --mk-i 保留为槽位序号（当前动效同拍，不用它错相位）。 */
  slots: [
    { at: 0, r: 131.5 },
    { at: 45, r: 107 },
    { at: 90, r: null },
    { at: 135, r: 107 },
    { at: 180, r: 131.5 },
    { at: 225, r: 107 },
    { at: 270, r: null },
    { at: 315, r: 107 },
  ],
} as const;

/** 极坐标 → 400 画布坐标（0° = 正右，逆时针为正；SVG 的 y 向下故取负） */
function pt(deg: number, r: number): [number, number] {
  const a = (deg * Math.PI) / 180;
  return [MARK.c + r * Math.cos(a), MARK.c - r * Math.sin(a)];
}

/** 单个槽位的弧 path（槽心角 at、半径 r，展开 MARK.span 度，线端平切） */
function arcPath(r: number, at: number): string {
  const [x0, y0] = pt(at - MARK.span / 2, r);
  const [x1, y1] = pt(at + MARK.span / 2, r);
  return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${r} ${r} 0 0 0 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}

export interface EmblemProps {
  /** 渲染边长（px） */
  size?: number;
  /** 常态动效：整枚徽记缓慢呼吸（默认开） */
  animated?: boolean;
  /** 加载态动效：八段弧槽按序充能，比常态更明显；传入时不再叠加 animated */
  charging?: boolean;
  /** 幽灵态：空态淡纹，整体压暗且不吸引视线 */
  faint?: boolean;
  className?: string;
}

export function Emblem({
  size = 96,
  animated = true,
  charging = false,
  faint = false,
  className,
}: EmblemProps): React.ReactElement {
  const cls =
    "emblem" +
    (animated && !charging ? " emblem-animated" : "") +
    (charging ? " emblem-charging" : "") +
    (faint ? " emblem-faint" : "") +
    (className ? ` ${className}` : "");
  return (
    <svg
      className={cls}
      viewBox={`0 0 ${MARK.view} ${MARK.view}`}
      width={size}
      height={size}
      aria-hidden="true"
      focusable="false"
    >
      <circle
        className="emblem-ring"
        cx={MARK.c}
        cy={MARK.c}
        r={MARK.ring.r}
        fill="none"
        stroke="currentColor"
        strokeWidth={MARK.ring.w}
      />
      <g fill="none" stroke="currentColor" strokeWidth={MARK.arcW} strokeLinecap="butt">
        {MARK.slots.map((s, i) =>
          s.r === null ? null : (
            <path
              key={s.at}
              className="emblem-slot"
              style={{ "--mk-i": i } as React.CSSProperties}
              d={arcPath(s.r, s.at)}
            />
          ),
        )}
      </g>
    </svg>
  );
}
