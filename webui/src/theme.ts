/** 主题色（强调色）注册表 —— EXMACHINA 控制台的换色系统
 *  每套主题 = 强调色 + RGB 三元组（供 rgba(var(--accent-rgb), α) 组合）。
 *  选择持久化在 localStorage，并同时写入 <html data-theme> 供 CSS 覆盖变量。 */
import { create } from "zustand";

export interface AccentTheme {
  key: string;
  name: string;
  color: string;
  rgb: string;
}

export const ACCENTS: AccentTheme[] = [
  { key: "cyan", name: "荧光青", color: "#35e0c8", rgb: "53, 224, 200" },
  { key: "matrix", name: "矩阵绿", color: "#4ade80", rgb: "74, 222, 128" },
  { key: "amber", name: "琥珀", color: "#ffb454", rgb: "255, 180, 84" },
  { key: "violet", name: "紫芒", color: "#b18cff", rgb: "177, 140, 255" },
  { key: "ice", name: "冰蓝", color: "#5ec8ff", rgb: "94, 200, 255" },
];

interface ThemeState {
  accent: string;
  setAccent: (key: string) => void;
}

const saved = localStorage.getItem("exm.accent") ?? "cyan";

export const useTheme = create<ThemeState>((set) => ({
  accent: ACCENTS.some((a) => a.key === saved) ? saved : "cyan",
  setAccent: (key) => {
    if (!ACCENTS.some((a) => a.key === key)) return;
    localStorage.setItem("exm.accent", key);
    set({ accent: key });
  },
}));

export function accentOf(key: string): AccentTheme {
  return ACCENTS.find((a) => a.key === key) ?? ACCENTS[0];
}
