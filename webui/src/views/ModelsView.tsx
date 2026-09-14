/**
 * 提供商页：API 端点与密钥的唯一下发处。
 *
 * 设计原则：**这一页只回答两件事——端点在哪、密钥是什么。**
 * 模型名、嵌入模型、失败回退等属于运维细节，收进「高级设置」折叠区，
 * 不打扰「接一个厂商进来」这个主流程（厂商预设走下拉，不再是按钮墙）。
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import {
  Badge, Button, Card, Collapse, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin,
  Tag, Tooltip, message,
} from "antd";
import {
  ApiOutlined, CheckCircleOutlined, DeleteOutlined, EditOutlined, PlusOutlined, ReloadOutlined,
} from "@ant-design/icons";
import { api, type LlmProfile, type LlmProfilesInfo } from "../api";
import { PageHeader } from "../components/PageHeader";

/** 厂商预设：选中即填端点、协议与常用模型名（模型名可在高级设置里改） */
interface Preset {
  key: string;
  name: string;
  baseUrl: string;
  orch: string;
  unit: string;
  apiFormat: string;
  /** 端点形态提示 */
  hint: string;
}

const PRESETS: Preset[] = [
  { key: "openai", name: "OpenAI", baseUrl: "https://api.openai.com/v1", orch: "gpt-4o", unit: "gpt-4o-mini", apiFormat: "openai", hint: "官方端点" },
  { key: "anthropic", name: "Anthropic", baseUrl: "https://api.anthropic.com", orch: "claude-sonnet-4-5", unit: "claude-haiku-4-5", apiFormat: "anthropic", hint: "Claude 原生协议" },
  { key: "gemini", name: "Google Gemini", baseUrl: "https://generativelanguage.googleapis.com/v1beta", orch: "gemini-2.5-flash", unit: "gemini-2.5-flash-lite", apiFormat: "gemini", hint: "Gemini 原生协议" },
  { key: "azure", name: "Azure OpenAI", baseUrl: "https://<资源名>.openai.azure.com", orch: "<部署名>", unit: "<部署名>", apiFormat: "azure", hint: "需替换资源名与部署名" },
  { key: "deepseek", name: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", orch: "deepseek-chat", unit: "deepseek-chat", apiFormat: "openai", hint: "OpenAI 兼容" },
  { key: "moonshot", name: "Moonshot / Kimi", baseUrl: "https://api.moonshot.cn/v1", orch: "moonshot-v1-32k", unit: "moonshot-v1-8k", apiFormat: "openai", hint: "OpenAI 兼容" },
  { key: "qwen", name: "通义千问", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1", orch: "qwen-plus", unit: "qwen-turbo", apiFormat: "openai", hint: "兼容模式端点" },
  { key: "zhipu", name: "智谱 GLM", baseUrl: "https://open.bigmodel.cn/api/paas/v4", orch: "glm-4-plus", unit: "glm-4-flash", apiFormat: "openai", hint: "OpenAI 兼容" },
  { key: "ollama", name: "Ollama（本机）", baseUrl: "http://127.0.0.1:11434/v1", orch: "llama3.1", unit: "qwen2.5", apiFormat: "openai", hint: "本机推理，无需密钥" },
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

function splitKeys(text: string | undefined): string[] {
  return (text ?? "")
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
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
    form.setFieldsValue({ preset: "openai", apiFormat: "openai", name: "", id: "" });
    setModal(true);
  };

  const openEdit = (p: LlmProfile) => {
    setEditing(p);
    const preset = PRESETS.find((x) => x.baseUrl === p.baseUrl)?.key ?? CUSTOM;
    form.setFieldsValue({
      preset,
      id: p.id,
      name: p.name,
      baseUrl: p.baseUrl,
      apiFormat: p.apiFormat || "openai",
      apiKey: p.apiKey,
      apiKeys: (p.apiKeys ?? []).join("\n"),
      model: p.orchModel === p.unitModel ? p.orchModel : "",
      orchModel: p.orchModel,
      unitModel: p.unitModel,
      fallback: p.fallback ?? undefined,
      embedModel: p.embedModel ?? "",
    });
    setModal(true);
  };

  /** 选中预设：只填「端点 / 协议 / 默认模型名」，密钥永远要自己贴 */
  const applyPreset = (key: string) => {
    const preset = PRESETS.find((p) => p.key === key);
    if (!preset) {
      form.setFieldsValue({ apiFormat: "openai" });
      return;
    }
    form.setFieldsValue({
      baseUrl: preset.baseUrl,
      apiFormat: preset.apiFormat,
      model: preset.orch,
      orchModel: undefined,
      unitModel: undefined,
      name: form.getFieldValue("name") || preset.name,
    });
  };

  const submit = async () => {
    const v = await form.validateFields();
    const model = (v.model ?? "").trim();
    const existing = editing;
    const orch = (v.orchModel ?? "").trim() || model || existing?.orchModel || "";
    const unit = (v.unitModel ?? "").trim() || model || existing?.unitModel || "";
    try {
      await api.saveLlmProfile({
        id: existing?.id ?? ((v.id ?? "").trim() || undefined),
        name: (v.name ?? "").trim() || "未命名提供商",
        baseUrl: (v.baseUrl ?? "").trim(),
        apiFormat: v.apiFormat ?? "openai",
        apiKey: v.apiKey ?? "",
        apiKeys: splitKeys(v.apiKeys),
        orchModel: orch,
        unitModel: unit,
        fallback: v.fallback ?? "",
        embedModel: (v.embedModel ?? "").trim(),
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
    ...PRESETS.map((p) => ({ value: p.key, label: `${p.name}　·　${p.hint}` })),
    { value: CUSTOM, label: "自定义 / 自建端点（OpenAI 兼容）" },
  ];

  return (
    <div className="pane-wrap">
      <PageHeader
        title="提供商"
        desc="API 端点与密钥的唯一下发处：先在这里接入模型，智能体与智能体组才能工作。智能体 / 组的默认模型在各自页面选择，未选择的跟随这里的「全局默认」。"
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
            const keyCount = p.apiKeys?.length ?? 0;
            const hasKey = Boolean(p.apiKey) || keyCount > 0;
            const modelSame = p.orchModel === p.unitModel;
            return (
              <Card
                key={p.id}
                size="small"
                className="hud model-card"
                title={
                  <div className="model-head">
                    {active ? <Badge status="processing" /> : null}
                    <span>{p.name}</span>
                    <span className="mono dim">{p.id}</span>
                    {active ? <Tag color="success">全局默认</Tag> : null}
                  </div>
                }
                extra={
                  <Space size={4}>
                    {!active && (
                      <Tooltip title="未单独指定模型的智能体 / 组都跟随它">
                        <Button size="small" type="primary" icon={<CheckCircleOutlined />} onClick={() => void activate(p.id)}>
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
                  {modelSame ? (
                    <Tag color="cyan">{p.orchModel || "未指定"}</Tag>
                  ) : (
                    <>
                      <Tag color="cyan">{p.orchModel || "—"}</Tag>
                      <span className="dim">指挥体</span>
                      <Tag>{p.unitModel || "—"}</Tag>
                      <span className="dim">子个体</span>
                    </>
                  )}
                </div>
                <div className="model-kv">
                  <span className="k">密钥</span>
                  {hasKey ? (
                    <Tag color="warning">已配置{keyCount > 1 ? `（Key 池 ${keyCount} 把）` : ""}</Tag>
                  ) : (
                    <Tag color="error">未配置（对话前请补填）</Tag>
                  )}
                  {p.fallback ? <Tag>失败回退 → {p.fallback}</Tag> : null}
                  {p.embedModel ? <Tag color="purple">嵌入 {p.embedModel}</Tag> : null}
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
          <Form.Item
            name="apiKey"
            label="API Key"
            extra={
              editing
                ? "留空 = 沿用已配置的密钥；如需更换请直接粘贴新 Key"
                : "留空可先建档案，之后在此补填（未配置密钥时无法对话）"
            }
          >
            <Input.Password placeholder={editing ? KEY_MASK : "sk-…"} />
          </Form.Item>
          <Form.Item
            name="apiKeys"
            label="Key 池（可选，每行一把）"
            extra="同一智能体粘性使用其中一把，仅在限额类失败时切换；不填则只用上面的 API Key"
          >
            <Input.TextArea rows={2} placeholder={"sk-key-one\nsk-key-two"} />
          </Form.Item>

          <Collapse
            ghost
            className="form-advanced"
            defaultActiveKey={editing && (editing.fallback || editing.embedModel) ? ["adv"] : []}
            items={[
              {
                key: "adv",
                label: "高级设置（名称与 ID / 协议 / 模型 / 嵌入 / 回退）",
                children: (
                  <>
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
                    <Form.Item name="apiFormat" label="API 协议">
                      <Select options={API_FORMATS} />
                    </Form.Item>
                    <Form.Item
                      name="model"
                      label="默认模型"
                      extra="指挥体与子个体共用；留空则沿用该档案已有模型名"
                    >
                      <Input placeholder="deepseek-chat" />
                    </Form.Item>
                    <div className="form-grid-2">
                      <Form.Item name="orchModel" label="指挥体模型（覆盖默认）">
                        <Input placeholder="可留空" />
                      </Form.Item>
                      <Form.Item name="unitModel" label="子个体模型（覆盖默认）">
                        <Input placeholder="可留空" />
                      </Form.Item>
                    </div>
                    <Form.Item
                      name="embedModel"
                      label="嵌入模型（可选）"
                      extra="仅混合记忆检索需要（如 text-embedding-3-small）；留空 = 纯词项召回"
                    >
                      <Input placeholder="留空表示不需要语义检索" />
                    </Form.Item>
                    <Form.Item
                      name="fallback"
                      label="失败回退（可选）"
                      extra="该档案请求失败且尚未产生内容时，自动改用所选档案重试"
                    >
                      <Select allowClear placeholder="不回退" options={fallbackOptions} />
                    </Form.Item>
                  </>
                ),
              },
            ]}
          />
        </Form>
      </Modal>
    </div>
  );
}
