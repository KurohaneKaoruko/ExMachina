/** 主题与界面偏好注册表 —— EXMACHINA 控制台
 *  强调色：5 套，选择持久化在 localStorage，写入 <html data-theme> 供 CSS 覆盖变量。
 *  字符数据流：界面级开关（exm.stream），写入 <html data-stream> 供 CSS 隐藏画布。
 *  两者都是本地偏好，不进后端 config（设置页的 schema 面板只管网关配置）。
 *  强调色的显示名走 i18n（theme.accent.<key>），本文件只持 key 与色值。 */
import { create } from "zustand";

export interface AccentTheme {
  key: string;
  color: string;
  rgb: string;
}

export const ACCENTS: AccentTheme[] = [
  { key: "cyan", color: "#35e0c8", rgb: "53, 224, 200" },
  { key: "matrix", color: "#4ade80", rgb: "74, 222, 128" },
  { key: "amber", color: "#ffb454", rgb: "255, 180, 84" },
  { key: "violet", color: "#b18cff", rgb: "177, 140, 255" },
  { key: "ice", color: "#5ec8ff", rgb: "94, 200, 255" },
];

export type StreamPref = "on" | "off";

const savedAccent = localStorage.getItem("exm.accent") ?? "cyan";
/** 启动即应用数据流偏好（模块加载先于首帧渲染，避免闪烁） */
const savedStream: StreamPref = localStorage.getItem("exm.stream") === "off" ? "off" : "on";
document.documentElement.dataset.stream = savedStream;

interface ThemeState {
  accent: string;
  setAccent: (key: string) => void;
  stream: StreamPref;
  setStream: (v: StreamPref) => void;
}

export const useTheme = create<ThemeState>((set) => ({
  accent: ACCENTS.some((a) => a.key === savedAccent) ? savedAccent : "cyan",
  setAccent: (key) => {
    if (!ACCENTS.some((a) => a.key === key)) return;
    localStorage.setItem("exm.accent", key);
    set({ accent: key });
  },
  stream: savedStream,
  setStream: (v) => {
    localStorage.setItem("exm.stream", v);
    document.documentElement.dataset.stream = v;
    set({ stream: v });
  },
}));

export function accentOf(key: string): AccentTheme {
  return ACCENTS.find((a) => a.key === key) ?? ACCENTS[0];
}
