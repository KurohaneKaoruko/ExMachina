/** 主框架：纯导航侧栏——组切换在对话页内，组管理独立成页 */
import React, { lazy, Suspense, useEffect, useState } from "react";
import { Menu, message, Tag } from "antd";
import {
  ApartmentOutlined,
  BellOutlined,
  ClusterOutlined,
  CloseOutlined,
  CodeOutlined,
  DatabaseOutlined,
  FileTextOutlined,
  MenuOutlined,
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
import { LANGS, useLang, useT, type TKey } from "./i18n/core";
import { ACCENTS, accentOf, useTheme } from "./theme";
import { CharStream } from "./components/CharStream";
import { AsciiDivider } from "./components/Ascii";
import { Emblem } from "./components/Emblem";
import { TelemetryBar } from "./components/TelemetryBar";
/* 视图懒加载（React.lazy + 动态 import）：按视图切分 chunk，首屏只拉 shell + 当前视图，
 * 其余视图在导航到时才加载（vendor 三方库另行在 vite.config.ts 的 manualChunks 分组缓存） */
const LoginView = lazy(() => import("./views/LoginView").then((m) => ({ default: m.LoginView })));
const ChatView = lazy(() => import("./views/ChatView").then((m) => ({ default: m.ChatView })));
const ModelsView = lazy(() => import("./views/ModelsView").then((m) => ({ default: m.ModelsView })));
const GraphView = lazy(() => import("./views/GraphView").then((m) => ({ default: m.GraphView })));
const LedgerView = lazy(() => import("./views/LedgerView").then((m) => ({ default: m.LedgerView })));
const MemoryView = lazy(() => import("./views/MemoryView").then((m) => ({ default: m.MemoryView })));
const SettingsView = lazy(() => import("./views/SettingsView").then((m) => ({ default: m.SettingsView })));
const GroupsView = lazy(() => import("./views/GroupsView").then((m) => ({ default: m.GroupsView })));
const NexusView = lazy(() => import("./views/NexusView").then((m) => ({ default: m.NexusView })));
const SinglesView = lazy(() => import("./views/SinglesView").then((m) => ({ default: m.SinglesView })));
const SkillsView = lazy(() => import("./views/SkillsView").then((m) => ({ default: m.SkillsView })));
const AutomationsView = lazy(() => import("./views/AutomationsView").then((m) => ({ default: m.AutomationsView })));
const ApprovalsView = lazy(() => import("./views/ApprovalsView").then((m) => ({ default: m.ApprovalsView })));
const ChannelsView = lazy(() => import("./views/ChannelsView").then((m) => ({ default: m.ChannelsView })));
const ActivityView = lazy(() => import("./views/ActivityView").then((m) => ({ default: m.ActivityView })));

/** 懒加载 chunk 未就绪时的兜底：品牌徽记 + 等宽字状态行（替代裸 Spin） */
function ViewSuspense({ children }: { children: React.ReactNode }): React.ReactElement {
  return (
    <Suspense
      fallback={
        <div className="view-loading">
          <Emblem size={56} charging />
          <span className="view-loading-text">LOADING</span>
        </div>
      }
    >
      {children}
    </Suspense>
  );
}

type ViewKey =
  | "chat"
  | "graph"
  | "agent"
  | "groups"
  | "nexus"
  | "skills"
  | "models"
  | "automations"
  | "approvals"
  | "channels"
  | "ledger"
  | "memory"
  | "activity"
  | "settings";

/** 合法视图键（hash 路由白名单校验） */
const VIEW_KEYS: ViewKey[] = [
  "chat", "agent", "groups", "nexus", "graph", "automations",
  "skills", "models", "approvals", "channels", "ledger", "memory", "activity", "settings",
];

/** 从 hash 解析视图：#/chat/<sessionId> → { view: chat, session }; 非法回落 chat */
function parseHash(): { view: ViewKey; session?: string } {
  const h = location.hash.replace(/^#\/?/, "");
  if (!h) return { view: "chat" };
  const [head, sub] = h.split("/");
  const view = VIEW_KEYS.find((k) => k === head);
  if (!view) return { view: "chat" };
  return { view, session: sub };
}

/** 侧栏清单 */
export const NAV_ITEMS: {
  key: ViewKey;
  en: string;
  labelKey: string;
  icon: React.ReactNode;
  /** 实验性功能标记：侧栏显示「实验」角标 */
  experimental?: boolean;
}[] = [
  { key: "chat", en: "CHAT", labelKey: "nav.chat", icon: <MessageOutlined /> },
  { key: "agent", en: "AGENTS", labelKey: "nav.agent", icon: <UserOutlined /> },
  { key: "groups", en: "GROUPS", labelKey: "nav.groups", icon: <TeamOutlined /> },
  { key: "nexus", en: "NEXUS", labelKey: "nav.nexus", icon: <ClusterOutlined />, experimental: true },
  { key: "graph", en: "DAG", labelKey: "nav.dag", icon: <ApartmentOutlined /> },
  { key: "automations", en: "CRON", labelKey: "nav.cron", icon: <ClockCircleOutlined /> },
  { key: "skills", en: "SKILLS", labelKey: "nav.skills", icon: <DeploymentUnitOutlined /> },
  { key: "models", en: "MODELS", labelKey: "nav.models", icon: <ApiOutlined /> },
  { key: "approvals", en: "APPROVALS", labelKey: "nav.approvals", icon: <BellOutlined /> },
  { key: "channels", en: "CHANNELS", labelKey: "nav.channels", icon: <CloudUploadOutlined /> },
  { key: "ledger", en: "LEDGER", labelKey: "nav.ledger", icon: <FundOutlined /> },
  { key: "memory", en: "MEMORY", labelKey: "nav.memory", icon: <DatabaseOutlined /> },
  { key: "activity", en: "EVENTS", labelKey: "nav.events", icon: <FileTextOutlined /> },
  { key: "settings", en: "CONFIG", labelKey: "nav.settings", icon: <SettingOutlined /> },
];

export default function App(): React.ReactElement {
  const config = useExm((s) => s.config);
  const wsConnected = useExm((s) => s.wsConnected);
  const init = useExm((s) => s.init);
  const accent = useTheme((s) => s.accent);
  const setAccent = useTheme((s) => s.setAccent);
  const initial = parseHash();
  const [view, setViewState] = useState<ViewKey>(initial.view);
  /** 切视图并同步 hash（app-shell-routing） */
  const setView = (v: ViewKey) => {
    setViewState(v);
    const target = "#/" + v + (location.hash.startsWith("#/chat/") && v === "chat" ? "" : "");
    if (location.hash !== target) {
      history.replaceState(null, "", target);
    }
  };
  const navRequest = useExm((st) => st.navRequest);
  useEffect(() => {
    if (!navRequest) return;
    if (navRequest.view === "chat" && navRequest.sessionId) {
      void useExm.getState().selectSession(navRequest.sessionId);
    }
    setView(navRequest.view as ViewKey);
    useExm.setState({ navRequest: null });
  }, [navRequest]);
  // hashchange：前进/后退导航（hashchange 时不回写 hash，避免回环）
  useEffect(() => {
    const onHash = () => {
      const { view: v } = parseHash();
      setViewState(v);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const [themeOpen, setThemeOpen] = useState(false);
  const [langOpen, setLangOpen] = useState(false);
  /** 移动端侧栏抽屉（≤860px 时侧栏覆盖呈现） */
  const [navOpen, setNavOpen] = useState(false);
  const t = useT();
  const lang = useLang((s) => s.lang);
  const setLang = useLang((s) => s.setLang);
  // 登录门：null = 鉴权中；false = 未解锁；true = 已进入
  const [authed, setAuthed] = useState<boolean | null>(null);
  /** 通道就绪 = 全局端点已配置密钥（apiKey 由服务端掩码回显，非空即已配置） */
  const llmReady = !!config?.llm?.apiKey;

  useEffect(() => {
    void (async () => {
      try {
        const key = localStorage.getItem("exm.key") ?? "";
        const r = await api.verifyAuth(key);
        setAuthed(r.ok);
        if (r.ok) {
          await init();
          // 会话深链（app-shell-routing）：#/chat/<sessionId> → 选中该会话
          const { view, session } = parseHash();
          if (view === "chat" && session) {
            const exists = useExm.getState().sessions.some((s) => s.id === session);
            if (exists) {
              await useExm.getState().selectSession(session);
            } else {
              message.info(t("route.sessionGone"));
            }
          }
        }
      } catch {
        setAuthed(false);
      }
    })();
  }, [init]);

  if (authed === null) {
    return (
      <div className="login-wrap">
        <div className="login-core">
          <div className="login-figure">
            <Emblem size={92} charging />
          </div>
          <span className="brand-name login-brand">EX·MACHINA</span>
          <div className="title-rule" style={{ marginTop: 12 }} />
          <div className="login-sub" style={{ textAlign: "center" }}>
            {t("status.connecting")} <span className="mono">[CONNECTING]</span>
          </div>
        </div>
      </div>
    );
  }
  if (!authed) {
    return (
      <ViewSuspense>
        <LoginView
          onUnlock={() => {
            setAuthed(true);
            void init();
          }}
        />
      </ViewSuspense>
    );
  }

  return (
    <div className="app-shell">
      {/* 移动端抽屉开关（桌面隐藏） */}
      <button className="mobile-nav-btn" onClick={() => setNavOpen(!navOpen)} aria-label="menu">
        {navOpen ? <CloseOutlined /> : <MenuOutlined />}
      </button>
      {navOpen && <div className="nav-mask" onClick={() => setNavOpen(false)} />}
      <aside className={`app-sider ${navOpen ? "open" : ""}`}>
        {/* 字符流放在侧栏背景里（见 styles.css 的层级说明） */}
        <CharStream />
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
          items={NAV_ITEMS.map((it) => ({
            key: it.key,
            icon: it.icon,
            label: (
              <>
                <span>{t(it.labelKey)}</span> <span className="label-en">[{it.en}]</span>
              </>
            ),
          }))}
          onClick={({ key }) => {
            setView(key as ViewKey);
            history.replaceState(null, "", "#/" + key);
            setNavOpen(false); // 移动端：选中即收起抽屉
          }}
        />
        <AsciiDivider className="sider-divider" />
        <div className={`theme-switcher ${themeOpen ? "open" : ""}`}>
          <button className="theme-toggle" onClick={() => setThemeOpen(!themeOpen)}>
            <span className="theme-label">{t("theme.label")}</span>
            <span className="theme-current" style={{ color: accentOf(accent).color }}>
              {t(`theme.accent.${accent}` as TKey)}
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
                <span>{t(`theme.accent.${a.key}` as TKey)}</span>
                <span className="label-en">[{a.key.toUpperCase()}]</span>
              </button>
            ))}
          </div>
        </div>
        <div className={`theme-switcher ${langOpen ? "open" : ""}`}>
          <button className="theme-toggle" onClick={() => setLangOpen(!langOpen)}>
            <span className="theme-label">{t("lang.label")}</span>
            <span className="theme-current">{LANGS.find((l) => l.code === lang)?.name ?? lang}</span>
            <span className={`theme-caret ${langOpen ? "up" : ""}`}>▸</span>
          </button>
          <div className="theme-panel">
            {LANGS.map((l) => (
              <button
                key={l.code}
                className={`theme-option ${lang === l.code ? "active" : ""}`}
                onClick={() => {
                  setLang(l.code);
                  setLangOpen(false);
                }}
              >
                <span>{l.name}</span>
              </button>
            ))}
          </div>
        </div>
        {/* 界面偏好（数据流等）在「设置」页维护，侧栏只留主题与语言 */}
        <div className="nav-footer">
          <span className={`status-led ${wsConnected ? "ok" : "bad"}`} />
          <span className="status-text">{wsConnected ? t("status.connected") : t("status.reconnecting")}</span>
          {/* 通道状态：是否已配置模型端点。未配置即无法推理，提示去「模型设置」页补齐。 */}
          <Tag color={llmReady ? "success" : "warning"} className="brand-tag">
            {llmReady ? t("status.link") : t("status.unlinked")}
          </Tag>
        </div>
      </aside>
      <main className="app-content">
        <ViewSuspense>
          {/* key 驱动：切视图即重挂载 → 入场动效（淡入 + 上移）每次都完整播放 */}
          <div className="view-stage" key={view}>
            {view === "chat" && <ChatView />}
            {view === "graph" && <GraphView />}
            {view === "agent" && <SinglesView />}
            {view === "groups" && <GroupsView />}
            {view === "nexus" && <NexusView />}
            {view === "skills" && <SkillsView />}
            {view === "models" && <ModelsView />}
            {view === "automations" && <AutomationsView />}
            {view === "approvals" && <ApprovalsView />}
            {view === "channels" && <ChannelsView />}
            {view === "ledger" && <LedgerView />}
            {view === "memory" && <MemoryView />}
            {view === "activity" && <ActivityView />}
            {view === "settings" && <SettingsView />}
          </div>
        </ViewSuspense>
      </main>
      {/* 底边遥测条：position:fixed 通栏贴窗口底，不参与 .app-shell 的 flex 布局 */}
      <TelemetryBar />
    </div>
  );
}
