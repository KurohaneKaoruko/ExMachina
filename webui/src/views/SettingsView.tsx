/**
 * 设置页：左侧分类导航 + 右侧单分类面板（一屏只看一类，杜绝堆叠杂乱）。
 * 字段定义来自后端 config_schema()；LLM 接入在「提供商」页维护。
 */
import React, { useEffect, useMemo, useState } from "react";
import { Button, Input, InputNumber, message, Select, Switch } from "antd";
import { BgColorsOutlined, DatabaseOutlined, FieldTimeOutlined, GlobalOutlined, RobotOutlined, SafetyCertificateOutlined, SafetyOutlined, SettingOutlined, ThunderboltOutlined, ToolOutlined } from "@ant-design/icons";
import { useExm } from "../store";
import { useTheme } from "../theme";
import { useT, type TKey } from "../i18n/core";
import { PageHeader } from "../components/PageHeader";
import { api, type ConfigSchema, type ConfigSchemaField, type GatewayConfig } from "../api";

/** 分组元数据。ui 是本地偏好分区，不属于后端 config schema */
const SECTIONS: Record<string, { icon: React.ReactNode; titleKey: TKey; descKey: TKey }> = {
  runtime: { icon: <SettingOutlined />, titleKey: "sec.runtime", descKey: "sec.runtime.desc" },
  memory: { icon: <DatabaseOutlined />, titleKey: "sec.memory", descKey: "sec.memory.desc" },
  security: { icon: <SafetyCertificateOutlined />, titleKey: "sec.security", descKey: "sec.security.desc" },
  automation: { icon: <FieldTimeOutlined />, titleKey: "sec.automation", descKey: "sec.automation.desc" },
  search: { icon: <GlobalOutlined />, titleKey: "sec.search", descKey: "sec.search.desc" },
  sandbox: { icon: <SafetyOutlined />, titleKey: "sec.sandbox", descKey: "sec.sandbox.desc" },
  browser: { icon: <RobotOutlined />, titleKey: "sec.browser", descKey: "sec.browser.desc" },
  hooks: { icon: <ThunderboltOutlined />, titleKey: "sec.hooks", descKey: "sec.hooks.desc" },
  tools: { icon: <ToolOutlined />, titleKey: "sec.tools", descKey: "sec.tools.desc" },
  ui: { icon: <BgColorsOutlined />, titleKey: "sec.ui", descKey: "sec.ui.desc" },
};

/**
 * 后端 config_schema() 下发的 label/help 是中文常量，前端在此按字段 key 覆盖为 i18n 词条；
 * 未收录的字段回退后端原文（新增配置项零 UI 改动的契约不被破坏）。
 */
const FIELD_I18N: Record<string, { label: TKey; help?: TKey }> = {
  "maxConcurrency": { label: "cfg.maxConcurrency", help: "cfg.maxConcurrency.help" },
  "maxSessionTokens": { label: "cfg.maxSessionTokens", help: "cfg.maxSessionTokens.help" },
  "memory.enabled": { label: "cfg.memoryEnabled", help: "cfg.memoryEnabled.help" },
  "memory.mdMaxChars": { label: "cfg.mdMaxChars", help: "cfg.mdMaxChars.help" },
  "memory.recallLimit": { label: "cfg.recallLimit", help: "cfg.recallLimit.help" },
  "memory.halfLifeDays": { label: "cfg.halfLifeDays", help: "cfg.halfLifeDays.help" },
  "security.execApproval": { label: "cfg.execApproval", help: "cfg.execApproval.help" },
  "security.execAllowlist": { label: "cfg.execAllowlist", help: "cfg.execAllowlist.help" },
  "security.authKey": { label: "cfg.authKey", help: "cfg.authKey.help" },
  "automation.heartbeatEnabled": { label: "cfg.heartbeatEnabled", help: "cfg.heartbeatEnabled.help" },
  "automation.heartbeatIntervalMinutes": { label: "cfg.heartbeatInterval", help: "cfg.heartbeatInterval.help" },
  "automation.heartbeatPrompt": { label: "cfg.heartbeatPrompt", help: "cfg.heartbeatPrompt.help" },
  "automation.autoAdapt": { label: "cfg.autoAdapt", help: "cfg.autoAdapt.help" },
  "automation.unitMaxSteps": { label: "cfg.unitMaxSteps", help: "cfg.unitMaxSteps.help" },
  "security.terminalTimeoutSecs": { label: "cfg.terminalTimeout", help: "cfg.terminalTimeout.help" },
  "security.toolOutputSpillChars": { label: "cfg.spillChars", help: "cfg.spillChars.help" },
  "search.provider": { label: "cfg.searchProvider", help: "cfg.searchProvider.help" },
  "search.endpoint": { label: "cfg.searchEndpoint", help: "cfg.searchEndpoint.help" },
  "search.apiKey": { label: "cfg.searchApiKey", help: "cfg.searchApiKey.help" },
  "search.maxResults": { label: "cfg.searchMax", help: "cfg.searchMax.help" },
  "sandbox.mode": { label: "cfg.sandboxMode", help: "cfg.sandboxMode.help" },
  "sandbox.allowNetwork": { label: "cfg.sandboxNet", help: "cfg.sandboxNet.help" },
  "sandbox.useBwrap": { label: "cfg.sandboxBwrap", help: "cfg.sandboxBwrap.help" },
  "sandbox.memoryMb": { label: "cfg.sandboxMem", help: "cfg.sandboxMem.help" },
  "sandbox.maxProcesses": { label: "cfg.sandboxProcs", help: "cfg.sandboxProcs.help" },
  "browser.executable": { label: "cfg.browserExe", help: "cfg.browserExe.help" },
  "browser.headless": { label: "cfg.browserHeadless", help: "cfg.browserHeadless.help" },
  "browser.timeoutSecs": { label: "cfg.browserTimeout", help: "cfg.browserTimeout.help" },
  "browser.maxChars": { label: "cfg.browserMaxChars", help: "cfg.browserMaxChars.help" },
  "hooks.preTool": { label: "cfg.hookPre", help: "cfg.hookPre.help" },
  "hooks.postTool": { label: "cfg.hookPost", help: "cfg.hookPost.help" },
  "hooks.onRunEnd": { label: "cfg.hookRun", help: "cfg.hookRun.help" },
  "tools.custom": { label: "cfg.toolsCustom", help: "cfg.toolsCustom.help" },
};

/** 特定字段用选择器而非自由文本（显示名走 i18n，值是发给后端的枚举） */
const ENUM_OVERRIDES: Record<string, { value: string; labelKey: TKey }[]> = {
  "security.execApproval": [
    { value: "off", labelKey: "cfg.approval.off" },
    { value: "risky", labelKey: "cfg.approval.risky" },
    { value: "always", labelKey: "cfg.approval.always" },
  ],
  "sandbox.mode": [
    { value: "off", labelKey: "cfg.sandbox.off" },
    { value: "workspace", labelKey: "cfg.sandbox.workspace" },
    { value: "strict", labelKey: "cfg.sandbox.strict" },
  ],
};

/** 钩子/列表类字段在 UI 里以「逗号分隔字符串」编辑，后端亦接受数组 */
const listToText = (v: unknown): string => (Array.isArray(v) ? v.join(", ") : typeof v === "string" ? v : "");

function readCurrent(cfg: GatewayConfig | null, key: string): unknown {
  if (!cfg) return undefined;
  switch (key) {
    case "maxConcurrency":
      return cfg.maxConcurrency;
    case "maxSessionTokens":
      return cfg.maxSessionTokens;
    case "memory.enabled":
      return cfg.memory.enabled;
    case "memory.recallLimit":
      return cfg.memory.recallLimit;
    case "memory.halfLifeDays":
      return cfg.memory.halfLifeDays;
    case "memory.mdMaxChars":
      return cfg.memory.mdMaxChars;
    case "tools.custom":
      return JSON.stringify(cfg.tools?.custom ?? [], null, 2);
    case "security.execApproval":
      return cfg.security.execApproval;
    case "security.execAllowlist":
      return cfg.security.execAllowlist;
    case "security.authKey":
      return cfg.security.authKey;
    case "automation.heartbeatEnabled":
      return cfg.automation.heartbeatEnabled;
    case "automation.heartbeatIntervalMinutes":
      return cfg.automation.heartbeatIntervalMinutes;
    case "automation.heartbeatPrompt":
      return cfg.automation.heartbeatPrompt;
    case "automation.autoAdapt":
      return cfg.automation.autoAdapt;
    case "automation.unitMaxSteps":
      return cfg.automation.unitMaxSteps;
    case "security.terminalTimeoutSecs":
      return cfg.security.terminalTimeoutSecs;
    case "security.toolOutputSpillChars":
      return cfg.security.toolOutputSpillChars;
    case "search.provider":
      return cfg.search?.provider;
    case "search.endpoint":
      return cfg.search?.endpoint;
    case "search.apiKey":
      return cfg.search?.apiKey;
    case "search.maxResults":
      return cfg.search?.maxResults;
    case "sandbox.mode":
      return cfg.sandbox?.mode;
    case "sandbox.allowNetwork":
      return cfg.sandbox?.allowNetwork;
    case "sandbox.useBwrap":
      return cfg.sandbox?.useBwrap;
    case "sandbox.memoryMb":
      return cfg.sandbox?.memoryMb;
    case "sandbox.maxProcesses":
      return cfg.sandbox?.maxProcesses;
    case "browser.executable":
      return cfg.browser?.executable;
    case "browser.headless":
      return cfg.browser?.headless;
    case "browser.timeoutSecs":
      return cfg.browser?.timeoutSecs;
    case "browser.maxChars":
      return cfg.browser?.maxChars;
    case "hooks.preTool":
      return listToText(cfg.hooks?.preTool);
    case "hooks.postTool":
      return listToText(cfg.hooks?.postTool);
    case "hooks.onRunEnd":
      return listToText(cfg.hooks?.onRunEnd);
    default:
      return undefined;
  }
}

export function SettingsView(): React.ReactElement {
  const { config, saveConfig } = useExm();
  const stream = useTheme((s) => s.stream);
  const setStream = useTheme((s) => s.setStream);
  const t = useT();
  const [schema, setSchema] = useState<ConfigSchema | null>(null);
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [selected, setSelected] = useState<string>("runtime");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void (async () => {
      const s = await api.configSchema();
      setSchema({ ...s, groups: s.groups.filter((g) => g.key !== "llm") });
      const init: Record<string, unknown> = {};
      for (const g of s.groups) {
        for (const f of g.fields) init[f.key] = readCurrent(config ?? null, f.key);
      }
      setValues(init);
    })();
  }, [config]);

  const groups = useMemo(() => schema?.groups ?? [], [schema]);
  const current = groups.find((g) => g.key === selected) ?? groups[0];
  const meta = current ? SECTIONS[current.key] : undefined;

  const setField = (key: string, v: unknown) => setValues((p) => ({ ...p, [key]: v }));

  const save = async () => {
    setSaving(true);
    try {
      const body: Record<string, unknown> = {};
      const nested = ["memory", "security", "automation", "search", "sandbox", "browser", "hooks", "tools"];
      for (const [k, v] of Object.entries(values)) {
        if (v === undefined) continue;
        const dot = k.indexOf(".");
        if (dot < 0) {
          if (v !== "") body[k] = v;
          continue;
        }
        const g = k.slice(0, dot);
        const f = k.slice(dot + 1);
        if (!nested.includes(g)) {
          if (v !== "") body[k] = v;
          continue;
        }
        // 空串表示「清除该项」（hooks 用空串清列表）；搜索凭据留空 = 沿用旧值，由后端判定
        body[g] = { ...((body[g] as Record<string, unknown>) ?? {}), [f]: v };
      }
      await saveConfig(body);
      message.success(t("settings.savedToast"));
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="CONFIG"
        title={t("settings.title")}
        desc={t("settings.desc")}
      />
      <div className="settings2">
        {/* 左侧分类导航：后端 schema 分组 + 本地「界面」偏好 */}
        <nav className="cfg-nav">
          {groups.map((g) => (
            <button
              key={g.key}
              className={`cfg-nav-item ${g.key === current?.key ? "on" : ""}`}
              onClick={() => setSelected(g.key)}
            >
              <span className="cfg-nav-icon">{SECTIONS[g.key]?.icon ?? <SettingOutlined />}</span>
              <span>{SECTIONS[g.key] ? t(SECTIONS[g.key]!.titleKey) : g.label}</span>
            </button>
          ))}
          <button
            className={`cfg-nav-item ${selected === "ui" ? "on" : ""}`}
            onClick={() => setSelected("ui")}
          >
            <span className="cfg-nav-icon">{SECTIONS.ui.icon}</span>
            <span>{t("sec.ui")}</span>
          </button>
        </nav>
        {/* 右侧面板：仅当前分类 */}
        <section className="cfg-panel">
        {selected === "ui" ? (
          /* —— 本地界面偏好：即时生效，不参与「保存设置」 —— */
          <>
            <header className="cfg-panel-head">
              <h3>{t("sec.ui")}</h3>
              <p>{t("sec.ui.desc")}。</p>
            </header>
            <div className="cfg-list">
              <div className="cfg2-row">
                <div className="cfg2-label">
                  <div className="cfg2-name">{t("settings.ui.streamName")}</div>
                  <div className="cfg2-help">{t("settings.ui.streamHelp")}</div>
                </div>
                <div className="cfg2-ctrl">
                  <Switch checked={stream === "on"} onChange={(v) => setStream(v ? "on" : "off")} />
                </div>
              </div>
            </div>
            <div className="cfg2-foot">
              <span className="dim">{t("settings.localTip")}</span>
            </div>
          </>
        ) : current && meta ? (
          <>
            <header className="cfg-panel-head">
              <h3>{t(meta.titleKey)}</h3>
              <p>{t(meta.descKey)}。{t("settings.hotTip")}</p>
            </header>
            <div className="cfg-list">
              {current.fields.map((f) => {
                const value = values[f.key];
                const fi = FIELD_I18N[f.key];
                const ctrl = ENUM_OVERRIDES[f.key] ? (
                  <Select
                    style={{ width: 220 }}
                    value={String(value ?? f.default)}
                    options={ENUM_OVERRIDES[f.key].map((o) => ({ value: o.value, label: t(o.labelKey) }))}
                    onChange={(v) => setField(f.key, v)}
                  />
                ) : f.kind === "boolean" ? (
                  <Switch
                    checked={value === undefined ? f.default === "true" : Boolean(value)}
                    onChange={(v) => setField(f.key, v)}
                  />
                ) : f.kind === "number" ? (
                  <InputNumber
                    style={{ width: 180 }}
                    min={f.min}
                    max={f.max}
                    value={value === undefined || value === "" ? Number(f.default) : Number(value)}
                    onChange={(v) => setField(f.key, v)}
                  />
                ) : f.kind === "password" ? (
                  <Input.Password
                    style={{ width: 320 }}
                    value={String(value ?? f.default)}
                    onChange={(e) => setField(f.key, e.target.value)}
                  />
                ) : f.key === "automation.heartbeatPrompt" || f.key === "security.execAllowlist" || f.key === "tools.custom" ? (
                  <Input.TextArea
                    style={{ width: 320 }}
                    rows={2}
                    value={String(value ?? f.default)}
                    onChange={(e) => setField(f.key, e.target.value)}
                  />
                ) : (
                  <Input style={{ width: 320 }} value={String(value ?? f.default)} onChange={(e) => setField(f.key, e.target.value)} />
                );
                return (
                  <div className="cfg2-row" key={f.key}>
                    <div className="cfg2-label">
                      <div className="cfg2-name">{fi ? t(fi.label) : f.label}</div>
                      <div className="cfg2-help">{fi?.help ? t(fi.help) : f.help}</div>
                    </div>
                    <div className="cfg2-ctrl">{ctrl}</div>
                  </div>
                );
              })}
            </div>
            <div className="cfg2-foot">
              <Button type="primary" loading={saving} onClick={save}>
                {t("settings.save")}
              </Button>
              <span className="dim">{t("settings.hotTip")}</span>
            </div>
          </>
        ) : (
          <div className="pane-loading"><i /></div>
        )}
        </section>
      </div>
    </div>
  );
}
