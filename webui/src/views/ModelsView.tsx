/**
 * 提供商页：API 端点与密钥的唯一下发处。
 *
 * 设计原则：**这一页只回答一件事——这家提供商的端点在哪、密钥是什么。**
 * 模型名只是「连上这个端点后默认用哪个模型」的附注（预设自动填，可改）。
 * 指挥体 / 子个体的模型差异属于编成，在「智能体组」与「子个体」页各自选择；
 * 嵌入模型属于记忆检索，在「记忆」页指定——都**不在这里**出现。
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Switch,
  Tag, Tooltip, message,
} from "antd";
import {
  ApiOutlined, CheckCircleOutlined, DeleteOutlined, EditOutlined, EyeOutlined, MinusCircleOutlined,
  PlusOutlined, ReloadOutlined, SoundOutlined,
} from "@ant-design/icons";
import { api, KEY_MASK, type LlmCapabilities, type LlmProfile, type LlmProfilesInfo, type ProfileModel } from "../api";
import { buildCapabilityOptions } from "../models";
import { PageHeader } from "../components/PageHeader";
import { useT, type TKey } from "../i18n/core";

/** 厂商预设：选中即填端点、协议与常用模型名（模型名可在表单里改） */
interface Preset {
  key: string;
  name: string;
  baseUrl: string;
  model: string;
  apiFormat: string;
}

const PRESETS: Preset[] = [
  { key: "openai", name: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4o", apiFormat: "openai" },
  { key: "anthropic", name: "Anthropic", baseUrl: "https://api.anthropic.com", model: "claude-sonnet-4-5", apiFormat: "anthropic" },
  { key: "gemini", name: "Google Gemini", baseUrl: "https://generativelanguage.googleapis.com/v1beta", model: "gemini-2.5-flash", apiFormat: "gemini" },
  { key: "azure", name: "Azure OpenAI", baseUrl: "https://<resource>.openai.azure.com", model: "<deployment>", apiFormat: "azure" },
  { key: "deepseek", name: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-chat", apiFormat: "openai" },
  { key: "moonshot", name: "Moonshot / Kimi", baseUrl: "https://api.moonshot.cn/v1", model: "moonshot-v1-32k", apiFormat: "openai" },
  { key: "qwen", name: "Qwen", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1", model: "qwen-plus", apiFormat: "openai" },
  { key: "zhipu", name: "Zhipu GLM", baseUrl: "https://open.bigmodel.cn/api/paas/v4", model: "glm-4-plus", apiFormat: "openai" },
  { key: "ollama", name: "Ollama", baseUrl: "http://127.0.0.1:11434/v1", model: "llama3.1", apiFormat: "openai" },
];

/** 预设的本地化显示名（覆盖 PRESETS.name 的只有中英写法不同的厂商） */
const PRESET_LABEL: Partial<Record<string, TKey>> = {
  qwen: "models.preset.qwen",
  zhipu: "models.preset.zhipu",
  ollama: "models.preset.ollama",
};

const CUSTOM = "__custom__";

/** API 协议选项的显示名走 i18n（值为协议标识，与后端契约） */
const API_FORMAT_KEYS: { value: string; key: TKey }[] = [
  { value: "openai", key: "models.fmt.openai" },
  { value: "anthropic", key: "models.fmt.anthropic" },
  { value: "gemini", key: "models.fmt.gemini" },
  { value: "azure", key: "models.fmt.azure" },
];

/** 密钥掩码哨兵：KEY_MASK 从 api.ts 引入（与后端契约一致：提交掩码位 = 服务端沿用旧值） */

/** 协议短标签（卡片上的 Tag 用），显示名走 i18n */
const FORMAT_LABEL_KEY: Record<string, TKey> = {
  openai: "models.fmtShort.openai",
  anthropic: "models.fmtShort.anthropic",
  gemini: "models.fmtShort.gemini",
  azure: "models.fmtShort.azure",
};

/** 表单字段：密钥为可增删的动态行 */
interface KeyForm {
  keys?: { value?: string }[];
}

/** 取有效密钥行（去空白）；掩码行保留原值语义 */
function liveKeys(rows: { value?: string }[] | undefined): string[] {
  return (rows ?? []).map((r) => (r?.value ?? "").trim()).filter(Boolean);
}

export function ModelsView(): React.ReactElement {
  const t = useT();
  const [info, setInfo] = useState<LlmProfilesInfo | null>(null);
  const [caps, setCaps] = useState<LlmCapabilities>({ speech: "", transcribe: "", visionRelay: "", embedding: "" });
  const [capSaving, setCapSaving] = useState(false);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [editing, setEditing] = useState<LlmProfile | null>(null);
  const [testing, setTesting] = useState("");
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setInfo(await api.llmProfiles());
      setCaps(await api.llmCapabilities());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const profiles = info?.profiles ?? [];

  const openCreate = () => {
    setEditing(null);
    form.resetFields();
    form.setFieldsValue({ preset: "openai", apiFormat: "openai", keys: [{}], models: [] });
    setModal(true);
  };

  const openEdit = (p: LlmProfile) => {
    setEditing(p);
    const preset = PRESETS.find((x) => x.baseUrl === p.baseUrl)?.key ?? CUSTOM;
    // 密钥合并为一行：主 Key + Key 池（保留原顺序；掩码行的原值在提交时由服务端沿用）
    const mask = p.apiKey ? [KEY_MASK] : [];
    const pool = (p.apiKeys ?? []).map((k) => (k ? KEY_MASK : ""));
    // 模型清单：空清单且默认模型存在时，先列出默认模型行（能力未标记，用户自行勾选）
    const rows: ProfileModel[] = p.models.length
      ? p.models.map((m) => ({ ...m }))
      : p.model
        ? [{ model: p.model, vision: false, audio: false }]
        : [];
    form.setFieldsValue({
      preset,
      id: p.id,
      name: p.name,
      baseUrl: p.baseUrl,
      apiFormat: p.apiFormat || "openai",
      model: p.model,
      models: rows,
      fallback: p.fallback ?? undefined,
      keys: [...mask, ...pool].map((v) => ({ value: v })),
    });
    setModal(true);
  };

  /** 选中预设：填「端点 / 协议 / 默认模型名」，密钥永远要自己贴 */
  const applyPreset = (key: string) => {
    const preset = PRESETS.find((p) => p.key === key);
    if (!preset) {
      form.setFieldsValue({ apiFormat: "openai" });
      return;
    }
    form.setFieldsValue({
      baseUrl: preset.baseUrl,
      apiFormat: preset.apiFormat,
      model: preset.model,
      name: form.getFieldValue("name") || preset.name,
    });
  };

  const submit = async () => {
    const v = await form.validateFields();
    const existing = editing;
    // 密钥行：掩码 = 沿用旧值（主 Key 走 apiKey，其余进 apiKeys）
    const rows = liveKeys((v as KeyForm).keys);
    const masks = rows.filter((k) => k === KEY_MASK);
    const fresh = rows.filter((k) => k !== KEY_MASK);
    let apiKey: string;
    let apiKeys: string[];
    if (existing) {
      const hadPrimary = Boolean(existing.apiKey);
      if (masks.length > 0) {
        // 保留原主 Key，其余掩码位沿用旧池
        apiKey = hadPrimary ? KEY_MASK : "";
        const poolMasks = masks.length - (hadPrimary ? 1 : 0);
        apiKeys = [...fresh, ...(existing.apiKeys ?? []).slice(0, Math.max(0, poolMasks))];
      } else {
        apiKey = "";
        apiKeys = fresh;
      }
    } else {
      [apiKey = "", ...apiKeys] = fresh;
      if (masks.length > 0) apiKey = "";
    }
    try {
      await api.saveLlmProfile({
        id: existing?.id ?? ((v.id ?? "").trim() || undefined),
        name: (v.name ?? "").trim() || t("models.unnamed"),
        baseUrl: (v.baseUrl ?? "").trim(),
        apiFormat: v.apiFormat ?? "openai",
        apiKey,
        apiKeys,
        model: (v.model ?? "").trim(),
        models: ((v.models ?? []) as ProfileModel[])
          .filter((m) => m.model && m.model.trim())
          .map((m) => ({ model: m.model.trim(), vision: !!m.vision, audio: !!m.audio })),
        fallback: v.fallback ?? "",
      });
      message.success(existing ? t("models.savedName", { name: v.name || existing.name }) : t("models.added", { name: v.name }));
      setModal(false);
      await load();
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  const activate = async (id: string) => {
    try {
      await api.activateLlmProfile(id);
      message.success(t("models.activated", { id }));
      await load();
    } catch (e) {
      message.error(t("models.switchFailed", { err: String(e) }));
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteLlmProfile(id);
      message.success(t("models.deletedName", { id }));
      await load();
    } catch (e) {
      message.error(t("models.deleteFailed", { err: String(e) }));
    }
  };

  const test = async (id?: string) => {
    const key = id ?? "__resolved__";
    setTesting(key);
    try {
      const r = await api.testLlmProfile(id);
      if (r.ok) {
        message.success(t("models.testOk", { status: r.status ?? 200 }));
      } else if (r.configured === false || r.mock) {
        message.warning(r.message ?? t("models.testNoKey"));
      } else {
        message.error(
          t("models.testFail", { detail: (r.error ?? `HTTP ${r.status ?? "?"} ${r.snippet ?? ""}`).slice(0, 160) }),
        );
      }
    } catch (e) {
      message.error(t("models.testFailed", { err: String(e) }));
    } finally {
      setTesting("");
    }
  };

  const fallbackOptions = useMemo(
    () =>
      profiles
        .filter((p) => p.id !== (editing?.id ?? ""))
        .map((p) => ({ value: p.id, label: `${p.name}（${p.id}）` })),
    [profiles, editing],
  );

  /** 能力槽位候选：每个档案的默认模型 + 清单里的每个模型（"档案ID" / "档案ID/模型名"） */
  const capOptions = useMemo(() => buildCapabilityOptions(profiles), [profiles]);

  const capField = (
    key: keyof LlmCapabilities,
    label: string,
    hint: string,
    placeholder: string,
  ) => (
    <div className="cap-row" key={key}>
      <div className="cap-label">{label}</div>
      <Select
        allowClear
        showSearch
        value={caps[key] || undefined}
        placeholder={placeholder}
        options={capOptions}
        onChange={(v) => setCaps((c) => ({ ...c, [key]: v ?? "" }))}
      />
      <div className="dim cap-hint">{hint}</div>
    </div>
  );

  const saveCaps = async () => {
    setCapSaving(true);
    try {
      setCaps(await api.saveLlmCapabilities(caps));
      message.success(t("models.capSaved"));
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    } finally {
      setCapSaving(false);
    }
  };

  const presetOptions = [
    ...PRESETS.map((p) => ({ value: p.key, label: PRESET_LABEL[p.key] ? t(PRESET_LABEL[p.key]!) : p.name })),
    { value: CUSTOM, label: t("models.customPreset") },
  ];

  const apiFormats = API_FORMAT_KEYS.map((f) => ({ value: f.value, label: t(f.key) }));

  return (
    <div className="pane-wrap">
      <PageHeader
        en="MODELS"
        title={t("nav.models")}
        desc={t("models.desc")}
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
              {t("models.add")}
            </Button>
            <Button icon={<ApiOutlined />} onClick={() => void test(undefined)} loading={testing === "__resolved__"}>
              {t("models.testGlobal")}
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              {t("common.refresh")}
            </Button>
          </>
        }
      />

      <Spin spinning={loading}>
        <div className="skill-grid">
          {profiles.map((p) => {
            const active = info?.active === p.id;
            const keyCount = (p.apiKeys?.length ?? 0) + (p.apiKey ? 1 : 0);
            const hasKey = keyCount > 0;
            return (
              <Card
                key={p.id}
                size="small"
                className="hud model-card"
                title={
                  <div className="model-head">
                    <span>{p.name}</span>
                    <span className="mono dim">{p.id}</span>
                  </div>
                }
                extra={
                  <Space size={4}>
                    {!active && (
                      <Tooltip title={t("models.setDefaultTip")}>
                        <Button size="small" icon={<CheckCircleOutlined />} onClick={() => void activate(p.id)}>
                          {t("models.setDefault")}
                        </Button>
                      </Tooltip>
                    )}
                    <Button size="small" icon={<EditOutlined />} onClick={() => openEdit(p)} />
                    <Button size="small" loading={testing === p.id} onClick={() => void test(p.id)}>
                      {t("models.test")}
                    </Button>
                    {profiles.length > 1 && (
                      <Popconfirm title={t("common.confirmDelete", { name: p.name })} onConfirm={() => void remove(p.id)}>
                        <Button size="small" danger icon={<DeleteOutlined />} />
                      </Popconfirm>
                    )}
                  </Space>
                }
              >
                <div className="model-kv">
                  <span className="k">{t("models.endpoint")}</span>
                  <span className="mono">{p.baseUrl}</span>
                  <Tag>{t(FORMAT_LABEL_KEY[p.apiFormat || "openai"] ?? "models.fmtShort.openai")}</Tag>
                </div>
                <div className="model-kv">
                  <span className="k">{t("models.modelName")}</span>
                  <Tag color="cyan">{p.model || t("models.unspecified")}</Tag>
                </div>
                <div className="model-kv">
                  <span className="k">{t("models.modelList")}</span>
                  <span className="model-caps">
                    {p.models.length ? (
                      p.models.map((m) => (
                        <Tag key={m.model} color={m.model === p.model ? "geekblue" : "default"}>
                          {m.model === p.model ? <CheckCircleOutlined /> : null}
                          {m.model}
                          {m.vision ? <EyeOutlined style={{ marginLeft: 4 }} /> : null}
                          {m.audio ? <SoundOutlined style={{ marginLeft: 4 }} /> : null}
                        </Tag>
                      ))
                    ) : (
                      <span className="dim">{t("models.capNone")}</span>
                    )}
                  </span>
                </div>
                <div className="model-kv">
                  <span className="k">{t("models.keyLabel")}</span>
                  {hasKey ? (
                    // 已配置是正常态，用中性标签；只有缺密钥才需要警示
                    <Tag className="tag-ok">{keyCount > 1 ? t("models.keyConfiguredN", { n: keyCount }) : t("models.keyConfigured")}</Tag>
                  ) : (
                    <Tag color="warning">{t("models.keyMissing")}</Tag>
                  )}
                  {p.fallback ? <Tag>{t("models.fallbackTo", { id: p.fallback })}</Tag> : null}
                </div>
                <div className="model-kv">
                  <span className="k">{t("models.statusLabel")}</span>
                  {active ? (
                    <Tag color="success">{t("models.globalDefault")}</Tag>
                  ) : (
                    <span className="dim">{t("models.followGlobal")}</span>
                  )}
                </div>
              </Card>
            );
          })}
        </div>
        {!loading && profiles.length === 0 && <Empty description={t("models.none")} className="pane-empty" />}
      </Spin>

      <Card
        size="small"
        className="hud cap-card"
        title={t("models.capTitle")}
        extra={
          <Button size="small" type="primary" loading={capSaving} onClick={() => void saveCaps()}>
            {t("common.save")}
          </Button>
        }
      >
        <div className="dim cap-hint" style={{ marginBottom: 10 }}>{t("models.capGlobalHint")}</div>
        <div className="cap-grid">
          {capField("speech", t("models.capSpeech"), t("models.capSpeechHint"), t("models.capUnset"))}
          {capField("transcribe", t("models.capStt"), t("models.capSttHint"), t("models.capUnset"))}
          {capField("visionRelay", t("models.capVisionRelay"), t("models.capVisionHint"), t("models.capUnset"))}
          {capField("embedding", t("models.capEmbed"), t("models.capEmbedHint"), t("models.capEmbedPh"))}
        </div>
      </Card>

      <Modal
        open={modal}
        title={editing ? t("models.editTitle", { name: editing.name }) : t("models.add")}
        onCancel={() => setModal(false)}
        onOk={() => void submit()}
        okText={t("common.save")}
        width={560}
      >
        <Form form={form} layout="vertical">
          <Form.Item name="preset" label={t("models.presetLabel")} rules={[{ required: true }]}>
            <Select options={presetOptions} onChange={(k) => applyPreset(k)} />
          </Form.Item>
          <Form.Item
            name="baseUrl"
            label="Base URL"
            rules={[{ required: true, whitespace: true, message: t("models.baseUrlRequired") }]}
          >
            <Input placeholder="https://api.deepseek.com/v1" />
          </Form.Item>

          <Form.Item label="API Key" style={{ marginBottom: 0 }}>
            <Form.List name="keys">
              {(fields, { add, remove }) => (
                <>
                  {fields.map(({ key, name, ...rest }) => (
                    <div key={key} className="key-row">
                      <Form.Item {...rest} name={name} noStyle>
                        <Input.Password placeholder={editing ? "******" : "sk-…"} />
                      </Form.Item>
                      {fields.length > 1 && (
                        <Button
                          type="text"
                          icon={<MinusCircleOutlined />}
                          onClick={() => remove(name)}
                          aria-label={t("models.removeKey")}
                        />
                      )}
                    </div>
                  ))}
                  <Button type="dashed" block icon={<PlusOutlined />} onClick={() => add("")}>
                    {t("models.addKey")}
                  </Button>
                  <div className="ant-form-item-extra" style={{ marginTop: 6 }}>
                    {t("models.keysExtra")}
                    {editing ? t("models.keysExtraEdit") : t("models.keysExtraNew")}
                  </div>
                </>
              )}
            </Form.List>
          </Form.Item>

          <Form.Item
            name="model"
            label={t("models.defaultModel")}
            extra={t("models.defaultModelExtra")}
          >
            <Input placeholder="deepseek-chat" />
          </Form.Item>

          <Form.Item label={t("models.modelList")} style={{ marginBottom: 0 }}>
            <Form.List name="models">
              {(fields, { add, remove }) => (
                <>
                  {fields.map(({ key, name, ...rest }) => (
                    <div key={key} className="model-row">
                      <Form.Item
                        {...rest}
                        name={[name, "model"]}
                        noStyle
                        rules={[{ required: true, whitespace: true, message: t("models.modelNameRequired") }]}
                      >
                        <Input placeholder="gpt-4o" />
                      </Form.Item>
                      <label className="cap-switch">
                        <Form.Item {...rest} name={[name, "vision"]} valuePropName="checked" noStyle>
                          <Switch size="small" />
                        </Form.Item>
                        <EyeOutlined />
                        {t("models.visionCap")}
                      </label>
                      <label className="cap-switch">
                        <Form.Item {...rest} name={[name, "audio"]} valuePropName="checked" noStyle>
                          <Switch size="small" />
                        </Form.Item>
                        <SoundOutlined />
                        {t("models.audioCap")}
                      </label>
                      <Button
                        type="text"
                        icon={<MinusCircleOutlined />}
                        onClick={() => remove(name)}
                        aria-label={t("common.delete")}
                      />
                    </div>
                  ))}
                  <Button
                    type="dashed"
                    block
                    icon={<PlusOutlined />}
                    onClick={() => add({ model: "", vision: false, audio: false })}
                  >
                    {t("models.addModel")}
                  </Button>
                  <div className="ant-form-item-extra" style={{ marginTop: 6 }}>
                    {t("models.modelListExtra")}
                  </div>
                </>
              )}
            </Form.List>
          </Form.Item>

          <div className="form-grid-2">
            <Form.Item name="name" label={t("models.displayName")}>
              <Input placeholder={t("models.displayNamePlaceholder")} />
            </Form.Item>
            <Form.Item
              name="id"
              label={t("models.profileId")}
              rules={[{ pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("models.idPattern") }]}
            >
              <Input placeholder={t("models.idAuto")} disabled={Boolean(editing)} />
            </Form.Item>
          </div>
          <Form.Item name="apiFormat" label={t("models.apiFormat")} extra={t("models.apiFormatExtra")}>
            <Select options={apiFormats} />
          </Form.Item>
          <Form.Item
            name="fallback"
            label={t("models.fallback")}
            extra={t("models.fallbackExtra")}
          >
            <Select allowClear placeholder={t("models.noFallback")} options={fallbackOptions} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
