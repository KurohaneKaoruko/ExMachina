/** 模型提供商页：API 端点与密钥的唯一下发处；全局默认 = 未单独指定模型的智能体/组的兜底 */
import React, { useCallback, useEffect, useState } from "react";
import {
  Badge, Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message,
} from "antd";
import { ApiOutlined, CheckCircleOutlined, PlusOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type LlmProfile, type LlmProfilesInfo } from "../api";

/** 厂商预设：一键填充端点与常用模型名（均可改） */
const PRESETS: { name: string; baseUrl: string; orch: string; unit: string; apiFormat?: string }[] = [
  { name: "OpenAI", baseUrl: "https://api.openai.com/v1", orch: "gpt-4o", unit: "gpt-4o-mini" },
  { name: "Anthropic", baseUrl: "https://api.anthropic.com", orch: "claude-sonnet-4-5", unit: "claude-haiku-4-5", apiFormat: "anthropic" },
  { name: "Google Gemini", baseUrl: "https://generativelanguage.googleapis.com/v1beta", orch: "gemini-2.5-flash", unit: "gemini-2.5-flash-lite", apiFormat: "gemini" },
  { name: "Azure OpenAI", baseUrl: "https://<资源名>.openai.azure.com", orch: "<部署名>", unit: "<部署名>", apiFormat: "azure" },
  { name: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", orch: "deepseek-chat", unit: "deepseek-chat" },
  { name: "Moonshot", baseUrl: "https://api.moonshot.cn/v1", orch: "moonshot-v1-32k", unit: "moonshot-v1-8k" },
  { name: "通义千问", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1", orch: "qwen-plus", unit: "qwen-turbo" },
  { name: "智谱", baseUrl: "https://open.bigmodel.cn/api/paas/v4", orch: "glm-4-plus", unit: "glm-4-flash" },
  { name: "Ollama(本机)", baseUrl: "http://127.0.0.1:11434/v1", orch: "llama3.1", unit: "qwen2.5" },
];

export function ModelsView(): React.ReactElement {
  const [info, setInfo] = useState<LlmProfilesInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [editing, setEditing] = useState<LlmProfile | null>(null);
  const [testing, setTesting] = useState<string>("");
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

  const openCreate = () => {
    setEditing(null);
    form.resetFields();
    setModal(true);
  };

  const openEdit = (p: LlmProfile) => {
    setEditing(p);
    form.setFieldsValue({
      id: p.id, name: p.name, baseUrl: p.baseUrl, apiKey: p.apiKey,
      apiKeys: (p.apiKeys ?? []).join("\\n"),
      orchModel: p.orchModel, unitModel: p.unitModel,
    });
    setModal(true);
  };

  const applyPreset = (preset: (typeof PRESETS)[number]) => {
    form.setFieldsValue({
      name: preset.name,
      baseUrl: preset.baseUrl,
      orchModel: preset.orch,
      unitModel: preset.unit,
      apiFormat: preset.apiFormat ?? "openai",
      apiKey: undefined,
    });
  };

  const submit = async () => {
    const v = await form.validateFields();
    try {
      await api.saveLlmProfile({
        id: editing?.id ?? v.id,
        name: v.name,
        baseUrl: v.baseUrl,
        apiFormat: v.apiFormat ?? "openai",
        apiKey: v.apiKey,
        apiKeys: (v.apiKeys ?? "")
          .split("\\n")
          .map((s: string) => s.trim())
          .filter(Boolean),
        orchModel: v.orchModel,
        unitModel: v.unitModel,
      });
      message.success(`提供商已保存：${v.name}`);
      setModal(false);
      await load();
    } catch (e) {
      message.error(`保存失败：${String(e)}`);
    }
  };

  const activate = async (id: string) => {
    try {
      const r = await api.activateLlmProfile(id);
      message.success(`全局默认已设为 ${id}${r.mock ? "（Mock 通道）" : "（真实推理）"}`);
      await load();
    } catch (e) {
      message.error(`切换失败：${String(e)}`);
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteLlmProfile(id);
      message.success(`提供商已删除：${id}`);
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
      } else if (r.mock) {
        message.info(r.message ?? "未配置 apiKey（模拟通道）");
      } else {
        message.error(`连通失败：${r.error ?? `HTTP ${r.status ?? "?"} ${r.snippet ?? ""}`.slice(0, 160)}`);
      }
    } catch (e) {
      message.error(`测试失败：${String(e)}`);
    } finally {
      setTesting("");
    }
  };

  return (
    <div className="pane-wrap">
      <div className="pane-toolbar">
        <Space>
          <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
            新增提供商
          </Button>
          <Button icon={<ReloadOutlined />} onClick={() => void load()}>
            刷新
          </Button>
          <Button icon={<ApiOutlined />} onClick={() => void test(undefined)} loading={testing === "__resolved__"}>
            测试全局默认
          </Button>
          <span className="pane-hint">
            专门填写 API 端点与密钥；支持 OpenAI 兼容 / Anthropic / Gemini 原生 / Azure OpenAI 四种协议。此处不切换交互对象——智能体/智能体组的默认模型请到各自设置中选择；未选择的跟随「全局默认」。
          </span>
        </Space>
      </div>

      <Spin spinning={loading}>
        <div className="skill-grid">
          {(info?.profiles ?? []).map((p) => {
            const active = info?.active === p.id;
            return (
              <Card
                key={p.id}
                size="small"
                className="hud model-card"
                title={
                  <Space>
                    {active ? <Badge status="processing" /> : null}
                    <span>{p.name}</span>
                    <span className="mono dim">{p.id}</span>
                    {active ? <Tag color="success">全局默认</Tag> : null}
                  </Space>
                }
                extra={
                  <Space>
                    {!active && (
                      <Button size="small" type="primary" icon={<CheckCircleOutlined />} onClick={() => void activate(p.id)}>
                        设为全局默认
                      </Button>
                    )}
                    <Button size="small" onClick={() => openEdit(p)}>
                      编辑
                    </Button>
                    <Button size="small" onClick={() => void test(p.id)} loading={testing === p.id}>
                      测试
                    </Button>
                    {(info?.profiles.length ?? 0) > 1 && (
                      <Popconfirm title="确认删除该档案？" onConfirm={() => void remove(p.id)}>
                        <Button size="small" danger>
                          删除
                        </Button>
                      </Popconfirm>
                    )}
                  </Space>
                }
              >
                <div className="model-line mono">{p.baseUrl}</div>
                <div className="skill-line">
                  模型： <Tag color="cyan">{p.orchModel || "—"}</Tag>
                  <span className="dim">指挥体</span>　<Tag>{p.unitModel || "—"}</Tag>
                  <span className="dim">子个体</span>
                </div>
                <div className="skill-line">
                  密钥：
                  {p.apiKey || (p.apiKeys?.length ?? 0) > 0 ? (
                    <Tag color="warning">
                      已配置{p.apiKeys?.length ? `（Key 池 ${p.apiKeys.length} 把）` : ""}
                    </Tag>
                  ) : (
                    <Tag>空（Mock）</Tag>
                  )}
                </div>
              </Card>
            );
          })}
        </div>
        {!loading && (info?.profiles.length ?? 0) === 0 && <Empty description="尚无提供商" className="graph-empty" />}
      </Spin>

      <Modal
        open={modal}
        title={editing ? `编辑提供商 ${editing.id}` : "新增模型提供商"}
        onCancel={() => setModal(false)}
        onOk={() => void submit()}
        okText="保存"
        width={560}
      >
        <div className="preset-row">
          <span className="rail-label">厂商预设</span>
          {PRESETS.map((p) => (
            <Button key={p.name} size="small" onClick={() => applyPreset(p)}>
              {p.name}
            </Button>
          ))}
        </div>
        <Form form={form} layout="vertical">
          {!editing && (
            <Form.Item
              name="id"
              label="档案 ID"
              rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: "仅字母/数字/-/_" }]}
            >
              <Input placeholder="如 deepseek-main" />
            </Form.Item>
          )}
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input placeholder="如 DeepSeek 主力" />
          </Form.Item>
          <Form.Item name="apiFormat" label="API 协议" initialValue="openai">
            <Select
              options={[
                { value: "openai", label: "OpenAI 兼容（/chat/completions）" },
                { value: "anthropic", label: "Anthropic 原生（/v1/messages）" },
                { value: "gemini", label: "Google Gemini 原生（generateContent）" },
                { value: "azure", label: "Azure OpenAI（deployments + api-key）" },
              ]}
            />
          </Form.Item>
          <Form.Item name="baseUrl" label="Base URL" rules={[{ required: true }]}>
            <Input placeholder="https://api.deepseek.com/v1" />
          </Form.Item>
          <Form.Item
            name="apiKey"
            label="API Key（主键）"
            extra={editing ? "留空或保持掩码 = 沿用已配置的密钥" : "留空 = Mock 模拟通道"}
          >
            <Input.Password placeholder={editing ? "***已配置***" : "sk-…"} />
          </Form.Item>
          <Form.Item
            name="apiKeys"
            label="Key 池（每行一把，粘性负载均衡：仅限额时切换并粘住）"
            extra="掩码行 = 沿用旧池同位 Key；不同子个体按标识散列从不同 Key 起步"
          >
            <Input.TextArea rows={3} placeholder={"sk-key-one\nsk-key-two"} />
          </Form.Item>
          <Space style={{ display: "flex" }} align="start">
            <Form.Item name="orchModel" label="指挥体模型" rules={[{ required: true }]} style={{ minWidth: 220 }}>
              <Input placeholder="deepseek-chat" />
            </Form.Item>
            <Form.Item name="unitModel" label="子个体模型" rules={[{ required: true }]} style={{ minWidth: 220 }}>
              <Input placeholder="deepseek-chat" />
            </Form.Item>
          </Space>
        </Form>
      </Modal>
    </div>
  );
}
