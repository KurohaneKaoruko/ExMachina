/**
 * 设置页：按「运行时 / 记忆 / 安全 / 自动化」分类的分区卡片布局。
 * 字段定义仍来自后端 config_schema()；LLM 接入在「模型提供商」页维护。
 */
import React, { useEffect, useMemo, useState } from "react";
import { Button, Card, Input, InputNumber, message, Switch, Tag, Tooltip } from "antd";
import { AimOutlined, DatabaseOutlined, FieldTimeOutlined, SafetyCertificateOutlined, SettingOutlined } from "@ant-design/icons";
import { useExm } from "../store";
import { api, type ConfigSchema, type ConfigSchemaField, type GatewayConfig } from "../api";

/** 分组元数据：图标 + 一句话说明 */
const SECTIONS: Record<string, { icon: React.ReactNode; title: string; desc: string }> = {
  runtime: { icon: <SettingOutlined />, title: "运行时", desc: "并发与资源上限" },
  memory: { icon: <DatabaseOutlined />, title: "记忆系统", desc: "召回范围与生命周期" },
  security: { icon: <SafetyCertificateOutlined />, title: "安全与审批", desc: "命令闸门与白名单" },
  automation: { icon: <FieldTimeOutlined />, title: "自动化与心跳", desc: "定时巡检与自优化" },
};

/** 布尔/长文本控件的字段偏好（按 key 匹配） */
const LONG_TEXT_KEYS = new Set(["automation.heartbeatPrompt", "security.execAllowlist"]);

function readCurrent(cfg: GatewayConfig | null, key: string): unknown {
  if (!cfg) return undefined;
  switch (key) {
    case "maxConcurrency":
      return cfg.maxConcurrency;
    case "maxSessionTokens":
      return cfg.maxSessionTokens;
    default:
      return undefined;
  }
}

function FieldRow({
  field, value, onChange,
}: {
  field: ConfigSchemaField; value: unknown; onChange: (v: unknown) => void;
}) {
  const isBool = field.kind === "boolean";
  const isLong = LONG_TEXT_KEYS.has(field.key);
  return (
    <div className="cfg-row">
      <div className="cfg-main">
        <div className="cfg-label">
          {field.label}
          {field.required && <Tag color="orange" className="cfg-tag">必填</Tag>}
        </div>
        <div className="cfg-help">{field.help}</div>
      </div>
      <div className="cfg-ctrl">
        {isBool ? (
          <Switch checked={value === undefined ? field.default === "true" : Boolean(value)} onChange={onChange} />
        ) : isLong ? (
          <Input.TextArea rows={2} value={String(value ?? field.default)} onChange={(e) => onChange(e.target.value)} />
        ) : field.kind === "number" ? (
          <InputNumber
            value={value === undefined || value === "" ? Number(field.default) : Number(value)}
            min={field.min} max={field.max}
            style={{ width: 180 }}
            onChange={(v) => onChange(v)}
          />
        ) : (
          <Input style={{ width: 260 }} value={String(value ?? field.default)} onChange={(e) => onChange(e.target.value)} />
        )}
      </div>
    </div>
  );
}

export function SettingsView(): React.ReactElement {
  const { config, saveConfig } = useExm();
  const [schema, setSchema] = useState<ConfigSchema | null>(null);
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void (async () => {
      const s = await api.configSchema();
      // LLM 接入组由「模型提供商」页承担
      setSchema({ ...s, groups: s.groups.filter((g) => g.key !== "llm") });
      const init: Record<string, unknown> = {};
      for (const g of s.groups) {
        for (const f of g.fields) init[f.key] = readCurrent(config, f.key);
      }
      setValues(init);
    })();
  }, [config]);

  const grouped = useMemo(() => {
    if (!schema) return [];
    return schema.groups.map((g) => ({
      ...g,
      meta: SECTIONS[g.key] ?? { icon: <SettingOutlined />, title: g.label, desc: "" },
      fields: g.fields.map((f) => ({ ...f, value: values[f.key] })),
    }));
  }, [schema, values]);

  const setField = (key: string, v: unknown) => setValues((prev) => ({ ...prev, [key]: v }));

  const save = async () => {
    setSaving(true);
    try {
      const body: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(values)) {
        if (v === undefined || v === "") continue;
        const [group, field] = k.split(".");
        if (group === "memory") {
          body.memory = { ...((body.memory as Record<string, unknown>) ?? {}), [field]: v };
        } else if (group === "security") {
          body.security = { ...((body.security as Record<string, unknown>) ?? {}), [field]: v };
        } else if (group === "automation") {
          body.automation = { ...((body.automation as Record<string, unknown>) ?? {}), [field]: v };
        } else {
          body[k] = v;
        }
      }
      await saveConfig(body);
      message.success("配置已保存并热生效");
    } catch (e) {
      message.error(`保存失败：${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  if (!schema) return <div className="pane-loading"><i /></div>;

  return (
    <div className="settings-wrap">
      <div className="settings-head">
        <span className="settings-title">设置</span>
        <span className="settings-sub">模型接入在「模型提供商」页维护</span>
        <Button type="primary" loading={saving} onClick={save} style={{ marginLeft: "auto" }}>
          保存全部
        </Button>
      </div>

      {grouped.map((g) => (
        <Card
          key={g.key}
          size="small"
          className="settings-card"
          title={
            <span>
              {g.meta.icon} <b>{g.meta.title}</b>
              {g.meta.desc && <span className="cfg-help" style={{ marginLeft: 10 }}>{g.meta.desc}</span>}
            </span>
          }
        >
          {g.fields.map((f) => (
            <FieldRow key={f.key} field={f} value={f.value} onChange={(v) => setField(f.key, v)} />
          ))}
        </Card>
      ))}
    </div>
  );
}
