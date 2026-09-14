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
          colorPrimary: color,
          colorInfo: color,
          colorBgBase: "#0a0e13",
          colorBgContainer: "#0f151c",
          colorBgElevated: "#131c25",
          colorBgLayout: "#0a0e13",
          colorBorder: "#22323f",
          colorBorderSecondary: "#182430",
          colorText: "#c9d7e2",
          // 与 styles.css 的 --muted 保持一致（antd token 无法直接读 CSS 变量）
          colorTextSecondary: "#6d8296",
          colorTextTertiary: "#4a5c6c",
          colorLink: color,
          colorSuccess: "#3fd68f",
          colorWarning: "#ffb454",
          colorError: "#ff5f66",
          borderRadius: 2,
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
