/** 字符画零件 —— 把「框线 / 刻度 / 标签」从纯 CSS 换成真实的框线字符。
 *
 *  为什么用字符而不是 CSS 边框：
 *  CSS 边框是连续的实线，读起来是「图形」；框线字符是离散的格子，
 *  读起来是「终端输出」。本项目的语汇是后者 —— 界面应该像一份被打印出来的机器记录。
 *
 *  全部组件都是纯装饰（aria-hidden），不承载语义，避免给读屏软件添噪音。
 *  只保留实际在用的三个：AsciiBar（页头通栏线）、AsciiDivider（分节线）、
 *  AsciiMeter（字符进度条）。不做大而全的字符零件库 —— 用不到的先不写。
 */
import React from "react";

/** 通栏字符带：``═══════════════════════════``
 *  用于页头底部，替代 1px 实线。密集的 ═ 读起来像双线，比单线更有「机械铭牌」感。 */
export function AsciiBar({
  className,
  glyph = "═",
  count = 200,
}: {
  className?: string;
  glyph?: string;
  count?: number;
}): React.ReactElement {
  return (
    <div className={`ascii-bar ${className ?? ""}`} aria-hidden="true">
      {glyph.repeat(count)}
    </div>
  );
}

/** 字符流分节线：``├─ LABEL ─────────────────┤``
 *  用在面板顶部或小节之间，替代纯 CSS 的 border-top。 */
export function AsciiDivider({
  label,
  className,
}: {
  label?: string;
  className?: string;
}): React.ReactElement {
  return (
    <div className={`ascii-divider ${className ?? ""}`} aria-hidden="true">
      <span className="ad-cap">├─</span>
      {label ? (
        <>
          <span className="ad-label">{label}</span>
          <span className="ad-cap">─</span>
        </>
      ) : null}
      <span className="ad-fill" />
      <span className="ad-cap">┤</span>
    </div>
  );
}

/** 字符进度条：``████████░░░░░░``  用真实字符画进度，比 antd Progress 更贴语汇 */
export function AsciiMeter({
  value,
  total,
  cells = 16,
  className,
}: {
  /** 当前值 */
  value: number;
  /** 总量；为 0 时视为无限（此时只画空槽） */
  total: number;
  cells?: number;
  className?: string;
}): React.ReactElement {
  const ratio = total > 0 ? Math.max(0, Math.min(1, value / total)) : 0;
  // 有值但不足一格时至少亮 1 格 —— 否则 0.6% 这种占比会让整条 bar 看起来像坏了
  const filled = value > 0 ? Math.max(1, Math.round(ratio * cells)) : 0;
  return (
    <span className={`ascii-meter ${className ?? ""}`} aria-hidden="true">
      <span className="am-on">{"█".repeat(Math.min(filled, cells))}</span>
      <span className="am-off">{"░".repeat(Math.max(0, cells - filled))}</span>
    </span>
  );
}
