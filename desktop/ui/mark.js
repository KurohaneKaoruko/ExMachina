/* ─────────────────────────────────────────────────────────────────────────────
 * EXMACHINA 徽记 —— 几何唯一定义（400 口径）
 *
 * ⚠ 全项目只在 mark.js 里定义一次，其它位置一律引用 markBody() / brandMark()。
 *   历史上 app.js 抄过两份、index.html 手抄过一份，三份互相追不上，于是出现过
 *   「四弧整体偏 10°」「第 3 条弧终点抄错导致顶部缺口整段消失」两次事故。
 *
 * ── 几何口径 ────────────────────────────────────────────────────────────────
 *   画布 400×400，圆心 (200,200)，全部线宽相同 = 24.5（线宽:半径 ≈ 0.186）。
 *   中枢环：r = 56.5（内缘 44.25 / 外缘 68.75）。
 *   外圈共 8 个 45° 槽位，边界落在 22.5° + k·45°：
 *       0° / 180°  → 远弧 r = 131.5
 *       45/135/225/315° → 近弧 r = 107
 *       90° / 270° → 空槽（上下各一个缺口，四段外观弧读起来是「左右为大、斜向为小」）
 *   远弧 − 近弧 = 24.5 = 一个线宽，因此**近弧外缘与远弧内缘恰好共线于 r = 119.25**，
 *   相邻槽位在 22.5° 处共用径向切边，连成阶梯状的整块。
 *   全部弧与中枢环同亮度（无明度阶梯），线端为 butt（源图为平切）。
 *
 *   改比值前后都量一遍：_exm_preview 的缺口量测应报「上下各一处缺口（中心角 90/270）」，
 *   且远弧外缘 = 143.75。
 * ──────────────────────────────────────────────────────────────────────────── */
(function () {
  "use strict";

  var MARK = {
    vb: 400,                       // 源图量测口径
    c: 200,
    ring: { r: 56.5, w: 24.5 },
    arcW: 24.5,
    span: 45,
    // 8 个 45° 槽位，按角度递增排列；r = null 即空槽。
    // 序号 --mk-i 作为槽位身份注入（备用相位）；当前动效整枚同拍，不用它错开延迟。
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
  };

  /** 极坐标 → 400 画布坐标（0° = 正右，逆时针为正；SVG 的 y 向下故取负） */
  function markPt(deg, r) {
    var a = (deg * Math.PI) / 180;
    return [MARK.c + r * Math.cos(a), MARK.c - r * Math.sin(a)];
  }

  /** 单个槽位的弧 path（以槽心角 at、半径 r 展开 span 度） */
  function markArcD(r, at) {
    var p0 = markPt(at - MARK.span / 2, r);
    var p1 = markPt(at + MARK.span / 2, r);
    return (
      "M " + p0[0].toFixed(2) + " " + p0[1].toFixed(2) +
      " A " + r + " " + r + " 0 0 0 " + p1[0].toFixed(2) + " " + p1[1].toFixed(2)
    );
  }

  /** 纹样本体（400 口径，无外层 svg）。槽位带 .mk-slot 与 --mk-i，供 CSS 编排充能动效。 */
  function markBody() {
    var slots = MARK.slots
      .map(function (s, i) {
        if (!s.r) return "";
        return '<path class="mk-slot" style="--mk-i:' + i + '" d="' + markArcD(s.r, s.at) + '"/>';
      })
      .join("");
    return (
      '<circle class="mk-ring" cx="' + MARK.c + '" cy="' + MARK.c + '" r="' + MARK.ring.r +
      '" fill="none" stroke="currentColor" stroke-width="' + MARK.ring.w + '"/>' +
      '<g class="mk-arcs" fill="none" stroke="currentColor" stroke-linecap="butt" stroke-width="' +
      MARK.arcW + '">' + slots + "</g>"
    );
  }

  /** 完整徽记 svg（默认 400 口径，尺寸由 size 决定） */
  function brandMark(cls, size) {
    cls = cls || "";
    size = size || 16;
    return (
      '<svg class="mark-svg ' + cls + '" width="' + size + '" height="' + size +
      '" viewBox="0 0 ' + MARK.vb + " " + MARK.vb + '" aria-hidden="true">' + markBody() + "</svg>"
    );
  }

  /** 把 400 口径的纹样嵌进 512 口径的复合图形（空态捕获盘等） */
  function markBodyIn512() {
    var k = 512 / MARK.vb;
    return (
      '<g transform="translate(256 256) scale(' + k + ') translate(' + -MARK.c + " " + -MARK.c +
      ')">' + markBody() + "</g>"
    );
  }

  /** HTML 用 <span data-mark="56"> 声明徽记位，统一在此注入 */
  function fillMarks(root) {
    (root || document).querySelectorAll("[data-mark]").forEach(function (el) {
      el.innerHTML = brandMark("", Number(el.dataset.mark) || 16);
    });
  }

  window.EXM_MARK = MARK;
  window.markPt = markPt;
  window.markArcD = markArcD;
  window.markBody = markBody;
  window.brandMark = brandMark;
  window.markBodyIn512 = markBodyIn512;
  window.fillMarks = fillMarks;

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      fillMarks();
    });
  } else {
    fillMarks();
  }
})();
