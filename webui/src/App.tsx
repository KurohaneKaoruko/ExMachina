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
import { useLang, useT } from "./i18n";
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
  const [langOpen, setLangOpen] = useState(false);
  const t = useT();
  const lang = useLang((s) => s.lang);
  const setLang = useLang((s) => s.setLang);
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
            {t("brand.sub")}<span className="cursor-blink">▊</span>
          </div>
        </div>
        <Menu
          mode="inline"
          selectedKeys={[view]}
          className="nav-menu"
          items={[
            { key: "chat", icon: <MessageOutlined />, label: <><span>{t("nav.chat")}</span> <span className="label-en">[CHAT]</span></> },
            { key: "agent", icon: <UserOutlined />, label: <><span>{t("nav.units")}</span> <span className="label-en">[UNITS]</span></> },
            { key: "groups", icon: <TeamOutlined />, label: <><span>{t("nav.groups")}</span> <span className="label-en">[GROUPS]</span></> },
            { key: "agents", icon: <RobotOutlined />, label: <><span>{t("nav.units")}</span> <span className="label-en">[UNITS]</span></> },
            { key: "skills", icon: <DeploymentUnitOutlined />, label: <><span>{t("nav.skills")}</span> <span className="label-en">[SKILLS]</span></> },
            { key: "models", icon: <ApiOutlined />, label: <><span>{t("nav.providers")}</span> <span className="label-en">[PROVIDERS]</span></> },
            { key: "graph", icon: <ApartmentOutlined />, label: <><span>{t("nav.dag")}</span> <span className="label-en">[DAG]</span></> },
            { key: "automations", icon: <ClockCircleOutlined />, label: <><span>{t("nav.cron")}</span> <span className="label-en">[CRON]</span></> },
            { key: "approvals", icon: <BellOutlined />, label: <><span>{t("nav.approvals")}</span> <span className="label-en">[APPROVALS]</span></> },
            { key: "channels", icon: <CloudUploadOutlined />, label: <><span>{t("nav.channels")}</span> <span className="label-en">[CHANNELS]</span></> },
            { key: "ledger", icon: <FundOutlined />, label: <><span>{t("nav.ledger")}</span> <span className="label-en">[LEDGER]</span></> },
            { key: "memory", icon: <DatabaseOutlined />, label: <><span>{t("nav.memory")}</span> <span className="label-en">[MEMORY]</span></> },
            { key: "activity", icon: <FileTextOutlined />, label: <><span>{t("nav.events")}</span> <span className="label-en">[EVENTS]</span></> },
            { key: "settings", icon: <SettingOutlined />, label: <><span>{t("nav.settings")}</span> <span className="label-en">[CONFIG]</span></> },
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
        <div className={`theme-switcher ${langOpen ? "open" : ""}`}>
          <button className="theme-toggle" onClick={() => setLangOpen(!langOpen)}>
            <span className="theme-label">{t("lang.label")}</span>
            <span className="theme-current">{lang === "zh" ? "中文" : "English"}</span>
            <span className={`theme-caret ${langOpen ? "up" : ""}`}>▸</span>
          </button>
          <div className="theme-panel">
            {([
              { key: "zh" as const, name: "中文" },
              { key: "en" as const, name: "English" },
            ]).map((l) => (
              <button
                key={l.key}
                className={`theme-option ${lang === l.key ? "active" : ""}`}
                onClick={() => {
                  setLang(l.key);
                  setLangOpen(false);
                }}
              >
                <span>{l.name}</span>
              </button>
            ))}
          </div>
        </div>
        <div className="nav-footer">
          <span className={`status-led ${wsConnected ? "ok" : "bad"}`} />
          <span className="status-text">{wsConnected ? t("status.connected") : t("status.reconnecting")}</span>
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
