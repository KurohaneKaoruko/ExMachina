/** 主框架：纯导航侧栏——组切换在对话页内，组管理独立成页 */
import React, { useEffect, useState } from "react";
import { Menu, Tag } from "antd";
import {
  ApartmentOutlined,
  BellOutlined,
  DatabaseOutlined,
  FileTextOutlined,
  MessageOutlined,
  ApiOutlined,
  RobotOutlined,
  SettingOutlined,
  UserOutlined,
  FundOutlined,
  ClockCircleOutlined,
  TeamOutlined,
  CloudUploadOutlined,
  DeploymentUnitOutlined,
} from "@ant-design/icons";
import { api } from "./api";
import { useExm } from "./store";
import { ACCENTS, accentOf, useTheme } from "./theme";
import { LoginView } from "./views/LoginView";
import { ChatView } from "./views/ChatView";
import { ModelsView } from "./views/ModelsView";
import { GraphView } from "./views/GraphView";
import { AgentsView } from "./views/AgentsView";
import { LedgerView } from "./views/LedgerView";
import { MemoryView } from "./views/MemoryView";
import { SettingsView } from "./views/SettingsView";
import { GroupsView } from "./views/GroupsView";
import { SinglesView } from "./views/SinglesView";
import { SkillsView } from "./views/SkillsView";
import { AutomationsView } from "./views/AutomationsView";
import { ApprovalsView } from "./views/ApprovalsView";
import { ChannelsView } from "./views/ChannelsView";
import { ActivityView } from "./views/ActivityView";

type ViewKey =
  | "chat"
  | "graph"
  | "agent"
  | "groups"
  | "agents"
  | "skills"
  | "models"
  | "automations"
  | "approvals"
  | "channels"
  | "ledger"
  | "memory"
  | "activity"
  | "settings";

export default function App(): React.ReactElement {
  const config = useExm((s) => s.config);
  const wsConnected = useExm((s) => s.wsConnected);
  const init = useExm((s) => s.init);
  const accent = useTheme((s) => s.accent);
  const setAccent = useTheme((s) => s.setAccent);
  const [view, setView] = useState<ViewKey>("chat");
  const [themeOpen, setThemeOpen] = useState(false);
  // 登录门：null = 鉴权中；false = 未解锁；true = 已进入
  const [authed, setAuthed] = useState<boolean | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const key = localStorage.getItem("exm.key") ?? "";
        const r = await api.verifyAuth(key);
        setAuthed(r.ok);
        if (r.ok) void init();
      } catch {
        setAuthed(false);
      }
    })();
  }, [init]);

  if (authed === null) {
    return (
      <div className="login-wrap">
        <div className="login-core">
          <div className="brand-row" style={{ justifyContent: "center" }}>
            <span className="brand-led" />
            <span className="brand-name">EX·MACHINA</span>
          </div>
          <div className="login-sub" style={{ textAlign: "center" }}>
            连结中 <span className="mono">[CONNECTING]</span>
          </div>
        </div>
      </div>
    );
  }
  if (!authed) {
    return (
      <LoginView
        onUnlock={() => {
          setAuthed(true);
          void init();
        }}
      />
    );
  }

  return (
    <div className="app-shell">
      <aside className="app-sider">
        <div className="brand">
          <div className="brand-row">
            <span className="brand-led" />
            <span className="brand-name">EX·MACHINA</span>
          </div>
          <div className="brand-sub">
            智械体集群 // 控制台<span className="cursor-blink">▊</span>
          </div>
        </div>
        <Menu
          mode="inline"
          selectedKeys={[view]}
          className="nav-menu"
          items={[
            { key: "chat", icon: <MessageOutlined />, label: <><span>对话</span> <span className="label-en">[CHAT]</span></> },
            { key: "agent", icon: <UserOutlined />, label: <><span>智能体</span> <span className="label-en">[AGENT]</span></> },
            { key: "groups", icon: <TeamOutlined />, label: <><span>智能体组</span> <span className="label-en">[GROUPS]</span></> },
            { key: "agents", icon: <RobotOutlined />, label: <><span>子个体</span> <span className="label-en">[UNITS]</span></> },
            { key: "skills", icon: <DeploymentUnitOutlined />, label: <><span>技能</span> <span className="label-en">[SKILLS]</span></> },
            { key: "models", icon: <ApiOutlined />, label: <><span>模型提供商</span> <span className="label-en">[PROVIDERS]</span></> },
            { key: "graph", icon: <ApartmentOutlined />, label: <><span>任务图</span> <span className="label-en">[DAG]</span></> },
            { key: "automations", icon: <ClockCircleOutlined />, label: <><span>自动化</span> <span className="label-en">[CRON]</span></> },
            { key: "approvals", icon: <BellOutlined />, label: <><span>审批</span> <span className="label-en">[APPROVALS]</span></> },
            { key: "channels", icon: <CloudUploadOutlined />, label: <><span>通道</span> <span className="label-en">[CHANNELS]</span></> },
            { key: "ledger", icon: <FundOutlined />, label: <><span>三账</span> <span className="label-en">[LEDGER]</span></> },
            { key: "memory", icon: <DatabaseOutlined />, label: <><span>记忆</span> <span className="label-en">[MEMORY]</span></> },
            { key: "activity", icon: <FileTextOutlined />, label: <><span>事件</span> <span className="label-en">[EVENTS]</span></> },
            { key: "settings", icon: <SettingOutlined />, label: <><span>设置</span> <span className="label-en">[CONFIG]</span></> },
          ]}
          onClick={({ key }) => setView(key as ViewKey)}
        />
        <div className="sider-stream" />
        <div className={`theme-switcher ${themeOpen ? "open" : ""}`}>
          <button className="theme-toggle" onClick={() => setThemeOpen(!themeOpen)}>
            <span className="theme-label">主题 [THEME]</span>
            <span className="theme-current" style={{ color: accentOf(accent).color }}>
              {accentOf(accent).name}
            </span>
            <span className={`theme-caret ${themeOpen ? "up" : ""}`}>▸</span>
          </button>
          <div className="theme-panel">
            {ACCENTS.map((a) => (
              <button
                key={a.key}
                className={`theme-option ${accent === a.key ? "active" : ""}`}
                onClick={() => {
                  setAccent(a.key);
                  setThemeOpen(false);
                }}
              >
                <span className="theme-swatch" style={{ background: a.color }} />
                <span>{a.name}</span>
                <span className="label-en">[{a.key.toUpperCase()}]</span>
              </button>
            ))}
          </div>
        </div>
        <div className="nav-footer">
          <span className={`status-led ${wsConnected ? "ok" : "bad"}`} />
          <span className="status-text">{wsConnected ? "网关已连结" : "通道重连中"}</span>
          <Tag color={config?.mock ? "warning" : "success"} className="brand-tag">
            {config?.mock ? "MOCK" : "LINK"}
          </Tag>
        </div>
      </aside>
      <main className="app-content">
        {view === "chat" && <ChatView />}
        {view === "graph" && <GraphView />}
        {view === "agent" && <SinglesView />}
        {view === "groups" && <GroupsView />}
        {view === "agents" && <AgentsView />}
        {view === "skills" && <SkillsView />}
        {view === "models" && <ModelsView />}
        {view === "automations" && <AutomationsView />}
        {view === "approvals" && <ApprovalsView />}
        {view === "channels" && <ChannelsView />}
        {view === "ledger" && <LedgerView />}
        {view === "memory" && <MemoryView />}
        {view === "activity" && <ActivityView />}
        {view === "settings" && <SettingsView />}
      </main>
    </div>
  );
}
