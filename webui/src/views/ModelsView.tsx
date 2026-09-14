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
  Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin,
  Tag, Tooltip, message,
} from "antd";
import {
  ApiOutlined, CheckCircleOutlined, DeleteOutlined, EditOutlined, MinusCircleOutlined,
  PlusOutlined, ReloadOutlined,
} from "@ant-design/icons";
import { api, type LlmProfile, type LlmProfilesInfo } from "../api";
import { PageHeader } from "../components/PageHeader";

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
  { key: "azure", name: "Azure OpenAI", baseUrl: "https://<资源名>.openai.azure.com", model: "<部署名>", apiFormat: "azure" },
  { key: "deepseek", name: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-chat", apiFormat: "openai" },
  { key: "moonshot", name: "Moonshot / Kimi", baseUrl: "https://api.moonshot.cn/v1", model: "moonshot-v1-32k", apiFormat: "openai" },
  { key: "qwen", name: "通义千问", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1", model: "qwen-plus", apiFormat: "openai" },
  { key: "zhipu", name: "智谱 GLM", baseUrl: "https://open.bigmodel.cn/api/paas/v4", model: "glm-4-plus", apiFormat: "openai" },
  { key: "ollama", name: "Ollama（本机）", baseUrl: "http://127.0.0.1:11434/v1", model: "llama3.1", apiFormat: "openai" },
];

const CUSTOM = "__custom__";

const API_FORMATS = [
  { value: "openai", label: "OpenAI 兼容（/chat/completions）" },
  { value: "anthropic", label: "Anthropic 原生（/v1/messages）" },
  { value: "gemini", label: "Google Gemini 原生（generateContent）" },
  { value: "azure", label: "Azure OpenAI（deployments + api-key）" },
];

const KEY_MASK = "***已配置***";

/** 协议短标签（卡片上的 Tag 用） */
const FORMAT_LABEL: Record<string, string> = {
  openai: "OpenAI 兼容",
  anthropic: "Anthropic",
  gemini: "Gemini",
  azure: "Azure",
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
  const [info, setInfo] = useState<LlmProfilesInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [editing, setEditing] = useState<LlmProfile | null>(null);
  const [testing, setTesting] = useState("");
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setInfo(await api.llmProfiles());
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
    form.setFieldsValue({ preset: "openai", apiFormat: "openai", keys: [{}] });
    setModal(true);
  };

  const openEdit = (p: LlmProfile) => {
    setEditing(p);
    const preset = PRESETS.find((x) => x.baseUrl === p.baseUrl)?.key ?? CUSTOM;
    // 密钥合并为一行：主 Key + Key 池（保留原顺序；掩码行的原值在提交时由服务端沿用）
    const mask = p.apiKey ? [KEY_MASK] : [];
    const pool = (p.apiKeys ?? []).map((k) => (k ? KEY_MASK : ""));
    form.setFieldsValue({
      preset,
      id: p.id,
      name: p.name,
      baseUrl: p.baseUrl,
      apiFormat: p.apiFormat || "openai",
      model: p.model,
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
        name: (v.name ?? "").trim() || "未命名提供商",
        baseUrl: (v.baseUrl ?? "").trim(),
        apiFormat: v.apiFormat ?? "openai",
        apiKey,
        apiKeys,
        model: (v.model ?? "").trim(),
        fallback: v.fallback ?? "",
      });
      message.success(existing ? `已保存：${v.name || existing.name}` : `提供商已接入：${v.name}`);
      setModal(false);
      await load();
    } catch (e) {
      message.error(`保存失败：${String(e)}`);
    }
  };

  const activate = async (id: string) => {
    try {
      await api.activateLlmProfile(id);
      message.success(`已设为全局默认：${id}`);
      await load();
    } catch (e) {
      message.error(`切换失败：${String(e)}`);
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteLlmProfile(id);
      message.success(`已删除：${id}`);
      await load();
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  const test = async (id?: string) => {
    const key = id ?? "__resolved__";
    setTesting(key);
    try {
      const r = await api.testLlmProfile(id);
      if (r.ok) {
        message.success(`连通正常（HTTP ${r.status ?? 200}）`);
      } else if (r.configured === false || r.mock) {
        message.warning(r.message ?? "该档案尚未配置 API Key");
      } else {
        message.error(
          `连通失败：${r.error ?? `HTTP ${r.status ?? "?"} ${r.snippet ?? ""}`.slice(0, 160)}`,
        );
      }
    } catch (e) {
      message.error(`测试失败：${String(e)}`);
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

  const presetOptions = [
    ...PRESETS.map((p) => ({ value: p.key, label: p.name })),
    { value: CUSTOM, label: "自定义 / 自建端点（OpenAI 兼容）" },
  ];

  return (
    <div className="pane-wrap">
      <PageHeader
        en="PROVIDERS"
        title="提供商"
        desc="接入模型端点与密钥。未单独指定模型的智能体 / 组跟随「全局默认」。"
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
              新增提供商
            </Button>
            <Button icon={<ApiOutlined />} onClick={() => void test(undefined)} loading={testing === "__resolved__"}>
              测试全局默认
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              刷新
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
                      <Tooltip title="未单独指定模型的智能体 / 组都跟随它">
                        <Button size="small" icon={<CheckCircleOutlined />} onClick={() => void activate(p.id)}>
                          设为默认
                        </Button>
                      </Tooltip>
                    )}
                    <Button size="small" icon={<EditOutlined />} onClick={() => openEdit(p)} />
                    <Button size="small" loading={testing === p.id} onClick={() => void test(p.id)}>
                      测试
                    </Button>
                    {profiles.length > 1 && (
                      <Popconfirm title={`确认删除「${p.name}」？`} onConfirm={() => void remove(p.id)}>
                        <Button size="small" danger icon={<DeleteOutlined />} />
                      </Popconfirm>
                    )}
                  </Space>
                }
              >
                <div className="model-kv">
                  <span className="k">端点</span>
                  <span className="mono">{p.baseUrl}</span>
                  <Tag>{FORMAT_LABEL[p.apiFormat || "openai"] ?? p.apiFormat}</Tag>
                </div>
                <div className="model-kv">
                  <span className="k">模型</span>
                  <Tag color="cyan">{p.model || "未指定"}</Tag>
                </div>
                <div className="model-kv">
                  <span className="k">密钥</span>
                  {hasKey ? (
                    // 已配置是正常态，用中性标签；只有缺密钥才需要警示
                    <Tag className="tag-ok">已配置{keyCount > 1 ? `（${keyCount} 把）` : ""}</Tag>
                  ) : (
                    <Tag color="warning">未配置（对话前请补填）</Tag>
                  )}
                  {p.fallback ? <Tag>失败回退 → {p.fallback}</Tag> : null}
                </div>
                <div className="model-kv">
                  <span className="k">状态</span>
                  {active ? (
                    <Tag color="success">全局默认</Tag>
                  ) : (
                    <span className="dim">跟随全局默认</span>
                  )}
                </div>
              </Card>
            );
          })}
        </div>
        {!loading && profiles.length === 0 && <Empty description="尚无提供商" className="pane-empty" />}
      </Spin>

      <Modal
        open={modal}
        title={editing ? `编辑提供商　${editing.name}` : "新增提供商"}
        onCancel={() => setModal(false)}
        onOk={() => void submit()}
        okText="保存"
        width={560}
      >
        <Form form={form} layout="vertical">
          <Form.Item name="preset" label="提供商类型" rules={[{ required: true }]}>
            <Select options={presetOptions} onChange={(k) => applyPreset(k)} />
          </Form.Item>
          <Form.Item
            name="baseUrl"
            label="Base URL"
            rules={[{ required: true, whitespace: true, message: "请填写 API 端点" }]}
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
                        <Input.Password placeholder={editing ? KEY_MASK : "sk-…"} />
                      </Form.Item>
                      {fields.length > 1 && (
                        <Button
                          type="text"
                          icon={<MinusCircleOutlined />}
                          onClick={() => remove(name)}
                          aria-label="移除这把密钥"
                        />
                      )}
                    </div>
                  ))}
                  <Button type="dashed" block icon={<PlusOutlined />} onClick={() => add("")}>
                    添加密钥
                  </Button>
                  <div className="ant-form-item-extra" style={{ marginTop: 6 }}>
                    可填多把：同一智能体粘性使用其中一把，仅在限额类失败时切换。
                    {editing ? "留空或保持掩码 = 沿用已配置的密钥。" : "留空可先建档案，之后再补填。"}
                  </div>
                </>
              )}
            </Form.List>
          </Form.Item>

          <Form.Item
            name="model"
            label="默认模型"
            extra="连接该端点后默认使用的模型名；个体与组可在各自页面单独指定"
          >
            <Input placeholder="deepseek-chat" />
          </Form.Item>

          <div className="form-grid-2">
            <Form.Item name="name" label="显示名称">
              <Input placeholder="如 DeepSeek 主力" />
            </Form.Item>
            <Form.Item
              name="id"
              label="档案 ID"
              rules={[{ pattern: /^[A-Za-z0-9_-]{1,48}$/, message: "仅字母 / 数字 / - / _" }]}
            >
              <Input placeholder="留空自动生成" disabled={Boolean(editing)} />
            </Form.Item>
          </div>
          <Form.Item name="apiFormat" label="API 协议" extra="预设已按厂商填好，通常无需改动">
            <Select options={API_FORMATS} />
          </Form.Item>
          <Form.Item
            name="fallback"
            label="失败回退（可选）"
            extra="该档案请求失败且尚未产生内容时，自动改用所选档案重试"
          >
            <Select allowClear placeholder="不回退" options={fallbackOptions} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
