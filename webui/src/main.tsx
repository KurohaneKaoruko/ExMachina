import React from "react";
import { createRoot } from "react-dom/client";
import { ConfigProvider, theme as antdTheme } from "antd";
import zhCN from "antd/locale/zh_CN";
import App from "./App";
import { accentOf, useTheme } from "./theme";
import "./styles.css";

function Root(): React.ReactElement {
  const accent = useTheme((s) => s.accent);
  const color = accentOf(accent).color;
  document.documentElement.dataset.theme = accent;
  return (
    <ConfigProvider
      locale={zhCN}
      theme={{
        algorithm: antdTheme.darkAlgorithm,
        token: {
          // 扁平化科技感：纯黑底 + 纯色强调 —— 全部纯色，零渐变零辉光
          // （bg/text 与 styles.css 的 HUD 变量同源，避免 antd 件与面板底色两种黑）
          colorPrimary: color,
          colorInfo: color,
          colorBgBase: "#000000",
          colorBgContainer: "#020c0e",
          colorBgElevated: "#04161a",
          colorBgLayout: "#000000",
          colorBorder: "#0a4a4d",
          colorBorderSecondary: "#0a4a4d",
          colorText: "#d9e4e7",
          // 与 styles.css 的 --muted 保持一致（antd token 无法直接读 CSS 变量）
          colorTextSecondary: "#8a9aa0",
          colorTextTertiary: "#5a6a72",
          colorLink: color,
          colorSuccess: "#00e676",
          colorWarning: "#ffea00",
          colorError: "#ff1744",
          // 直角纪律：所有 antd 组件圆角归零，精致感交给 1px 边框/明度差/动效
          borderRadius: 0,
          borderRadiusLG: 0,
          borderRadiusSM: 0,
          borderRadiusXS: 0,
          borderRadiusOuter: 0,
          // 动效令牌：快进慢出（仪表指针质感），全局统一节奏
          motionDurationFast: "0.12s",
          motionDurationMid: "0.2s",
          motionDurationSlow: "0.32s",
          motionEaseOut: "cubic-bezier(0.22, 1, 0.36, 1)",
          motionEaseInOut: "cubic-bezier(0.65, 0, 0.35, 1)",
          fontFamily:
            '"Segoe UI", "PingFang SC", "Microsoft YaHei", system-ui, sans-serif',
        },
      }}
    >
      <App />
    </ConfigProvider>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);

// PWA：Service Worker（离线壳缓存；API/WS 永远走网络）
if ("serviceWorker" in navigator && location.protocol !== "devtools:") {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch(() => {
      /* SW 不可用（如 http:// 局域网 IP 下部分浏览器限制）时静默降级为普通网页 */
    });
  });
}
