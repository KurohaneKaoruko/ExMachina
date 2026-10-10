/** 主题与界面偏好注册表 —— EXMACHINA 控制台
 *  强调色：5 套纯色（扁平化科技感：无渐变、无辉光），选择持久化在 localStorage，
 *  写入 <html data-theme> 供 CSS 覆盖变量。
 *  字符数据流：界面级开关（exm.stream），写入 <html data-stream> 供 CSS 隐藏画布。
 *  两者都是本地偏好，不进后端 config（设置页的 schema 面板只管网关配置）。
 *  强调色的显示名走 i18n（theme.accent.<key>），本文件只持 key 与色值。 */
import { create } from "zustand";

export interface AccentTheme {
  key: string;
  color: string;
  rgb: string;
}

/** 纯色硬朗口径：青=默认主色 / 蓝=次级 / 绿=成功 / 红=危险 / 黄=警示，按语义取用 */
export const ACCENTS: AccentTheme[] = [
  { key: "cyan", color: "#00e5ff", rgb: "0, 229, 255" },
  { key: "blue", color: "#7c8cff", rgb: "124, 140, 255" },
  { key: "green", color: "#00e676", rgb: "0, 230, 118" },
  { key: "red", color: "#ff1744", rgb: "255, 23, 68" },
  { key: "yellow", color: "#ffea00", rgb: "255, 234, 0" },
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
