/** 字符画数据流 —— 全局氛围层（克制版）
 *
 *  设计意图：让界面像一台「正在被读写的机器」，但**不干扰任何一个字的阅读**。
 *
 *  ⚠️ 关键教训（踩过的坑）：最初把 canvas 做成 position:fixed 的全屏底层，结果
 *  要么透到正文底下与文字打架，要么给 .app-shell 铺不透明底之后被完全遮住 —— 两头不讨好。
 *  正确做法是**把字符流放进界面自己的留白区域**（侧栏背后、内容区外围），
 *  让它成为「纸张的纹理」而不是「桌面壁纸」。
 *
 *  因此本组件的定位是 absolute（相对最近的定位祖先），由外层容器决定它出现在哪。
 *  目前挂载点：.app-sider（侧栏背景）。
 *
 *  三条克制原则：
 *  1) 极低频 —— 约 12 格/秒；
 *  2) 极低对比 —— 头部亮度约 0.16，尾部衰减到 0.02；
 *  3) 可关闭 —— 尊重 prefers-reduced-motion，并支持 :root[data-stream="off"]。
 */
import React, { useEffect, useRef } from "react";

/** 字符池：等宽 ASCII + 少量框线字符，都是「程序里真实会出现」的符号。
 *  刻意不掺入日文片假名 —— 那是 matrix 视觉的刻板印象，与本项目的工程图语汇不符。 */
const GLYPHS = "0123456789ABCDEF{}[]()<>/\\|=+-*#%$&_:;.,?!@^~";

const FONT_SIZE = 13;
const FONT_STACK =
  'ui-monospace, "JetBrains Mono", "Cascadia Code", "SF Mono", Consolas, monospace';

/** 每秒下落的行数 */
const ROWS_PER_SEC = 12;

/** 列间距倍数：1.35 倍字宽，留出呼吸感，像点阵屏而非实心色块 */
const COL_SPACING = 1.35;

/** 拖尾长度（格） */
const TAIL = 9;

/** 头部亮度。克制版取 0.16 —— 高于此值在正文旁边会开始抢注意力 */
const HEAD_ALPHA = 0.16;

interface CharStreamProps {
  /** 额外的 class，由挂载点决定尺寸与定位 */
  className?: string;
}

export function CharStream({ className }: CharStreamProps): React.ReactElement {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;

    const mq = matchMedia("(prefers-reduced-motion: reduce)");

    let raf = 0;
    let cols = 0;
    let rows = 0;
    let cellW = 0;
    let cellH = 0;
    let drops: number[] = [];
    let speeds: number[] = [];
    let acc = 0;
    let last = 0;
    let running = false;
    let w = 0;
    let h = 0;

    const ctx = canvas.getContext("2d", { alpha: true });
    if (!ctx) return;

    /** 从 CSS 变量取当前强调色的 RGB 三元组，实现换色联动 */
    const accentRgb = (): string =>
      getComputedStyle(document.documentElement).getPropertyValue("--accent-rgb").trim() ||
      "53, 224, 200";

    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      // 用容器实际尺寸，而不是视口尺寸 —— 这是「放进留白区」的关键
      const rect = wrap.getBoundingClientRect();
      w = Math.max(1, Math.floor(rect.width));
      h = Math.max(1, Math.floor(rect.height));
      canvas.width = Math.floor(w * dpr);
      canvas.height = Math.floor(h * dpr);
      canvas.style.width = `${w}px`;
      canvas.style.height = `${h}px`;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.font = `${FONT_SIZE}px ${FONT_STACK}`;
      ctx.textBaseline = "top";

      cellW = FONT_SIZE * 0.6 * COL_SPACING;
      cellH = FONT_SIZE;
      cols = Math.ceil(w / cellW) + 1;
      rows = Math.ceil(h / cellH) + 2;

      // 每列独立的下落进度与速度，让雨幕不成一整排
      drops = Array.from({ length: cols }, () => Math.random() * rows);
      speeds = Array.from({ length: cols }, () => 0.6 + Math.random() * 0.8);
    };

    const draw = (dt: number) => {
      const rgb = accentRgb();
      ctx.clearRect(0, 0, w, h);

      for (let c = 0; c < cols; c++) {
        drops[c] += ROWS_PER_SEC * speeds[c] * dt;
        if (drops[c] > rows + 6) {
          // 归零时重新随机速度，避免长跑后各列速度趋同
          drops[c] = -Math.random() * 8;
          speeds[c] = 0.6 + Math.random() * 0.8;
        }

        const head = Math.floor(drops[c]);
        for (let i = 0; i < TAIL; i++) {
          const r = head - i;
          if (r < 0 || r >= rows) continue;
          // 亮度沿拖尾线性衰减
          const a = i === 0 ? HEAD_ALPHA : HEAD_ALPHA * 0.9 * (1 - i / TAIL);
          if (a <= 0.012) break;
          const ch = GLYPHS[(Math.random() * GLYPHS.length) | 0];
          ctx.fillStyle = `rgba(${rgb}, ${a.toFixed(3)})`;
          ctx.fillText(ch, c * cellW, r * cellH);
        }
      }
    };

    const loop = (t: number) => {
      if (!running) return;
      // 首帧 last=0，挡掉一次性大 dt（否则会瞬间跳一整屏）
      const dt = last ? Math.min((t - last) / 1000, 0.05) : 0;
      last = t;
      acc += dt;
      // 限 30fps：字符雨不需要 60fps，省电且观感更接近终端
      if (acc >= 1 / 30) {
        draw(acc);
        acc = 0;
      }
      raf = requestAnimationFrame(loop);
    };

    const start = () => {
      if (running) return;
      running = true;
      last = 0;
      acc = 0;
      raf = requestAnimationFrame(loop);
    };
    const stop = () => {
      running = false;
      cancelAnimationFrame(raf);
      ctx.clearRect(0, 0, w, h);
    };
    /** 关停条件二选一：系统减少动效，或界面偏好 data-stream="off"。
     *  停的是 rAF 本身，不是 CSS 藏起来了还在空转。 */
    const applyPref = () => {
      const off = document.documentElement.dataset.stream === "off";
      if (off || mq.matches) stop();
      else start();
    };

    resize();
    applyPref();

    // 容器尺寸变化（窗口缩放、侧栏折叠）时重建列阵
    const ro = new ResizeObserver(() => resize());
    ro.observe(wrap);
    mq.addEventListener("change", applyPref);
    // 设置页的特效开关改 data-stream 时热启停
    const mo = new MutationObserver(applyPref);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-stream"] });

    return () => {
      stop();
      ro.disconnect();
      mo.disconnect();
      mq.removeEventListener("change", applyPref);
    };
  }, []);

  return (
    <div ref={wrapRef} className={`char-stream ${className ?? ""}`} aria-hidden="true">
      <canvas ref={canvasRef} />
    </div>
  );
}
