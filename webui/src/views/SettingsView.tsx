/**
 * 设置页（Schema 驱动，运行时/记忆/安全/自动化）
 * 字段来自后端 `config_schema()` ⇒ 新增配置项时本文件无需改动（docs/07 §4.2）。
 * LLM 接入（端点/密钥/模型）不在本页：请到「模型提供商」页维护。
 */
import React, { useEffect, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Form,
  Input,
  InputNumber,
  message,
  Space,
  Spin,
  Switch,
  Tag,
} from "antd";
import { useExm } from "../store";
import { api, type ConfigSchema, type ConfigSchemaField, type GatewayConfig } from "../api";

/** 从运行配置中按 schema key 取值（未覆盖的键由 schema default 兜底） */
function readCurrent(cfg: GatewayConfig | null, key: string): unknown {
  if (!cfg) return undefined;
  switch (key) {
    case "maxConcurrency":
      return cfg.maxConcurrency;
    default:
      return undefined;
  }
}

function FieldControl({
  field,
  value,
  onChange,
}: {
  field: ConfigSchemaField;
  value: unknown;
  onChange: (v: unknown) => void;
}): React.ReactElement {
  switch (field.kind) {
    case "password":
      return (
        <Input.Password
          value={String(value ?? "")}
          placeholder="留空保持不变（当前密钥不回显）"
          autoComplete="off"
          onChange={(e) => onChange(e.target.value)}
        />
      );
    case "number":
      return (
        <InputNumber
          value={value === undefined || value === "" ? Number(field.default) : Number(value)}
          min={field.min}
          max={field.max}
          style={{ width: 220 }}
          onChange={(v) => onChange(v)}
        />
      );
    case "boolean":
      return (
        <Switch
          checked={value === undefined ? field.default === "true" : Boolean(value)}
          onChange={(v) => onChange(v)}
        />
      );
    default:
      return (
        <Input
          value={String(value ?? field.default)}
          onChange={(e) => onChange(e.target.value)}
        />
      );
  }
}

/** 扁平 key（llm.baseUrl / memory.recallLimit）→ PUT body 分组结构 */
function buildBody(values: Record<string, unknown>): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  for (const [key, v] of Object.entries(values)) {
    if (v === undefined || v === "") continue;
    const [group, field] = key.split(".");
    if (group === "memory") {
      body.memory = { ...((body.memory as Record<string, unknown>) ?? {}), [field]: v };
    } else {
      body[key] = v;
    }
  }
  return body;
}

export function SettingsView(): React.ReactElement {
  const { config, saveConfig } = useExm();
  const [schema, setSchema] = useState<ConfigSchema | null>(null);
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void (async () => {
      const s = await api.configSchema();
      // LLM 接入组由「模型提供商」页承担，设置页不重复渲染
      setSchema({ ...s, groups: s.groups.filter((g) => g.key !== "llm") });
      const init: Record<string, unknown> = {};
      for (const g of s.groups) {
        for (const f of g.fields) init[f.key] = readCurrent(config, f.key);
      }
      setValues(init);
    })();
  }, [config]);

  if (!schema) return <Spin />;

  return (
    <Card
      title={`网关设置（配置结构 v${schema.configVersion}；字段由后端 Schema 驱动，新增配置项无需改本页）`}
      className="settings-card"
    >
      <Space direction="vertical" className="full-width" size="middle">
        {config?.mock ? (
          <Alert
            type="info"
            showIcon
            message="当前为 Mock 模拟通道"
            description="尚未配置任何模型提供商密钥，系统以确定性模拟运行（可完整演示流程）。请到「模型提供商」页添加端点与 API Key；智能体/智能体组可在各自设置中选择默认模型。"
          />
        ) : (
          <Alert
            type="success"
            showIcon
            message="已接入真实 LLM 端点"
            description="端点与密钥在「模型提供商」页维护；各智能体/智能体组可在其设置中选择默认模型，未选择的跟随全局默认。"
          />
        )}

        <Form layout="vertical">
          {schema.groups.map((g) => (
            <Card key={g.key} size="small" title={g.label} className="settings-group">
              {g.fields.map((f) => (
                <Form.Item
                  key={f.key}
                  label={
                    <Space>
                      <span>{f.label}</span>
                      {f.required && <Tag color="red">必填</Tag>}
                      <span className="settings-key">{f.key}</span>
                    </Space>
                  }
                  help={f.help}
                >
                  <FieldControl
                    field={f}
                    value={values[f.key]}
                    onChange={(v) => setValues({ ...values, [f.key]: v })}
                  />
                </Form.Item>
              ))}
            </Card>
          ))}

          <Space>
            <Button
              type="primary"
              loading={saving}
              onClick={async () => {
                setSaving(true);
                try {
                  await saveConfig(buildBody(values));
                  message.success("配置已保存并热生效");
                } catch (e) {
                  message.error(`保存失败：${String(e)}`);
                } finally {
                  setSaving(false);
                }
              }}
            >
              保存并热生效
            </Button>
            <span className="settings-hint">
              {"与 CLI 共用同一份 .exmachina/config.json；亦可 exm config set <key> <value>"}
            </span>
          </Space>
        </Form>
      </Space>
    </Card>
  );
}
