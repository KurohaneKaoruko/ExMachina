/** 轻量 i18n：壳层与常用文案双语（zh/en），localStorage 持久化；面板内容翻译增量覆盖 */
import { create } from "zustand";

export type Lang = "zh" | "en";
const LS_KEY = "exm.lang";

type Entry = [zh: string, en: string];
const DICT: Record<string, Entry> = {
  "nav.chat": ["对话", "Chat"],
  "nav.agent": ["智能体", "Agents"],
  "nav.groups": ["智能体组", "Groups"],
  "nav.units": ["子个体", "Units"],
  "nav.skills": ["技能", "Skills"],
  "nav.providers": ["提供商", "Providers"],
  "nav.dag": ["任务图", "Task Graph"],
  "nav.cron": ["自动化", "Automations"],
  "nav.approvals": ["审批", "Approvals"],
  "nav.channels": ["通道", "Channels"],
  "nav.ledger": ["三账", "Ledger"],
  "nav.memory": ["记忆", "Memory"],
  "nav.events": ["事件", "Events"],
  "nav.settings": ["设置", "Settings"],
  "brand.sub": ["智械体集群 // 控制台", "Agent Cluster // Console"],
  "status.connected": ["网关已连结", "Gateway connected"],
  "status.reconnecting": ["通道重连中", "Reconnecting…"],
  "status.link": ["通道就绪", "READY"],
  "status.unlinked": ["未接入", "NOT SET"],
  "theme.label": ["主题 [THEME]", "Theme"],
  "lang.label": ["语言 [LANGUAGE]", "Language"],
  "session.new": ["新会话", "New Session"],
  "settings.title": ["设置", "Settings"],
  "settings.sub": [
    "模型接入在「提供商」页维护",
    "Model access lives on the Providers page",
  ],
  "settings.save": ["保存全部", "Save All"],
  "sec.runtime": ["运行时", "Runtime"],
  "sec.runtime.desc": ["并发与资源上限", "Concurrency & limits"],
  "sec.memory": ["记忆系统", "Memory"],
  "sec.memory.desc": ["召回范围与生命周期", "Recall scope & lifecycle"],
  "sec.security": ["安全与审批", "Security & Approvals"],
  "sec.security.desc": ["命令闸门与白名单", "Command gate & allowlist"],
  "sec.automation": ["自动化", "Automation"],
  "sec.automation.desc": ["心跳巡检与自优化", "Heartbeat & self-tuning"],
  "common.save": ["保存", "Save"],
  "common.new": ["新建", "New"],
  "common.delete": ["删除", "Delete"],
  "common.refresh": ["刷新", "Refresh"],
};

interface LangState {
  lang: Lang;
  setLang: (l: Lang) => void;
}

export const useLang = create<LangState>((set) => ({
  lang: ((localStorage.getItem(LS_KEY) as Lang) || "zh") as Lang,
  setLang: (l) => {
    localStorage.setItem(LS_KEY, l);
    set({ lang: l });
  },
}));

/** 翻译函数：未收录的 key 原样返回 */
export function useT(): (key: string) => string {
  const lang = useLang((s) => s.lang);
  return (key: string) => {
    const e = DICT[key];
    if (!e) return key;
    return lang === "en" ? e[1] : e[0];
  };
}
