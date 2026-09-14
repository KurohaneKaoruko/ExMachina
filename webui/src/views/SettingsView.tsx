/**
 * 设置页：左侧分类导航 + 右侧单分类面板（一屏只看一类，杜绝堆叠杂乱）。
 * 字段定义来自后端 config_schema()；LLM 接入在「提供商」页维护。
 */
import React, { useEffect, useMemo, useState } from "react";
import { Button, Input, InputNumber, message, Select, Switch } from "antd";
import { DatabaseOutlined, FieldTimeOutlined, SafetyCertificateOutlined, SettingOutlined } from "@ant-design/icons";
import { useExm } from "../store";
import { useT } from "../i18n";
import { api, type ConfigSchema, type ConfigSchemaField, type GatewayConfig } from "../api";

/** 分组元数据 */
const SECTIONS: Record<string, { icon: React.ReactNode; titleKey: string; descKey: string }> = {
  runtime: { icon: <SettingOutlined />, titleKey: "sec.runtime", descKey: "sec.runtime.desc" },
  memory: { icon: <DatabaseOutlined />, titleKey: "sec.memory", descKey: "sec.memory.desc" },
  security: { icon: <SafetyCertificateOutlined />, titleKey: "sec.security", descKey: "sec.security.desc" },
  automation: { icon: <FieldTimeOutlined />, titleKey: "sec.automation", descKey: "sec.automation.desc" },
};

/** 特定字段用选择器而非自由文本 */
const ENUM_OVERRIDES: Record<string, { value: string; label: string }[]> = {
  "security.execApproval": [
    { value: "off", label: "关闭（不拦截）" },
    { value: "risky", label: "仅高危命令" },
    { value: "always", label: "全部命令" },
  ],
};

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
    case "security.execApproval":
      return cfg.security.execApproval;
    case "security.execAllowlist":
      return cfg.security.execAllowlist;
    case "automation.heartbeatEnabled":
      return cfg.automation.heartbeatEnabled;
    case "automation.heartbeatIntervalMinutes":
      return cfg.automation.heartbeatIntervalMinutes;
    case "automation.heartbeatPrompt":
      return cfg.automation.heartbeatPrompt;
    case "automation.autoAdapt":
      return cfg.automation.autoAdapt;
    default:
      return undefined;
  }
}

export function SettingsView(): React.ReactElement {
  const { config, saveConfig } = useExm();
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
      for (const [k, v] of Object.entries(values)) {
        if (v === undefined || v === "") continue;
        const [g, f] = k.split(".");
        if (g === "memory") body.memory = { ...((body.memory as Record<string, unknown>) ?? {}), [f]: v };
        else if (g === "security") body.security = { ...((body.security as Record<string, unknown>) ?? {}), [f]: v };
        else if (g === "automation") body.automation = { ...((body.automation as Record<string, unknown>) ?? {}), [f]: v };
        else body[k] = v;
      }
      await saveConfig(body);
      message.success("配置已保存并热生效");
    } catch (e) {
      message.error(`保存失败：${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="settings2">
      {/* 左侧分类导航 */}
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
      </nav>

      {/* 右侧面板：仅当前分类 */}
      <section className="cfg-panel">
        {current && meta ? (
          <>
            <header className="cfg-panel-head">
              <h3>{t(meta.titleKey)}</h3>
              <p>{t(meta.descKey)}。改动保存后热生效，无需重启。</p>
            </header>
            <div className="cfg-list">
              {current.fields.map((f) => {
                const value = values[f.key];
                const ctrl = ENUM_OVERRIDES[f.key] ? (
                  <Select
                    style={{ width: 220 }}
                    value={String(value ?? f.default)}
                    options={ENUM_OVERRIDES[f.key]}
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
                ) : f.key === "automation.heartbeatPrompt" || f.key === "security.execAllowlist" ? (
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
                      <div className="cfg2-name">{f.label}</div>
                      {f.help && <div className="cfg2-help">{f.help}</div>}
                    </div>
                    <div className="cfg2-ctrl">{ctrl}</div>
                  </div>
                );
              })}
            </div>
            <div className="cfg2-foot">
              <Button type="primary" loading={saving} onClick={save}>
                保存{t(meta.titleKey)}
              </Button>
              <span className="dim">保存后热生效，无需重启</span>
            </div>
          </>
        ) : (
          <div className="pane-loading"><i /></div>
        )}
      </section>
    </div>
  );
}
