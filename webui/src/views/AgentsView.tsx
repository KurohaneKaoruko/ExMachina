/** 个体面板：激活组的个体清单，支持人设编辑与（自定义组内）创建/删除子个体 */
import React, { useState } from "react";
import {
  Button,
  Card,
  Form,
  Input,
  message,
  Modal,
  Popconfirm,
  Select,
  Space,
  Table,
  Tag,
  Tooltip,
} from "antd";
import { DeleteOutlined, EditOutlined, PlusOutlined, UndoOutlined } from "@ant-design/icons";
import { useExm } from "../store";
import { api, type PersonaInfo } from "../api";
import type { AgentDefinition } from "../types";

// 域标签由核心装载体规范化为中文后下发，前端直接展示，不做静态映射

export function AgentsView(): React.ReactElement {
  const { agents, graph } = useExm();
  const groups = useExm((s) => s.groups);
  const activeGroup = useExm((s) => s.activeGroup);
  const refreshAgents = useExm((s) => s.refreshAgents);
  const [personaOf, setPersonaOf] = useState<AgentDefinition | null>(null);
  const [personaInfo, setPersonaInfo] = useState<PersonaInfo | null>(null);
  const [personaDraft, setPersonaDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [form] = Form.useForm();

  const activeMeta = groups.find((g) => g.id === activeGroup);
  const isBuiltin = activeMeta?.builtin ?? true;

  const submitCreate = async () => {
    const v = await form.validateFields();
    try {
      await api.createAgent({
        name: v.name,
        identifier: v.identifier,
        description: v.description,
        domain: v.domain || "自定义",
        tier: v.tier || "unit",
        prompt: v.prompt || undefined,
      });
      message.success(`子个体已创建：${v.name}`);
      setCreateOpen(false);
      form.resetFields();
      await refreshAgents();
    } catch (e) {
      message.error(`创建失败：${String(e)}`);
    }
  };

  const removeOne = async (identifier: string) => {
    try {
      await api.removeAgent(identifier);
      message.success(`已删除：${identifier}`);
      await refreshAgents();
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  const busyAgents = new Set(
    (graph?.nodes ?? [])
      .filter((n) => ["running", "dispatched", "syncing"].includes(n.status))
      .map((n) => n.agentIdentifier),
  );

  const openPersona = async (a: AgentDefinition) => {
    setPersonaOf(a);
    setPersonaInfo(null);
    setPersonaDraft("");
    try {
      const info = await api.getPersona(a.identifier);
      setPersonaInfo(info);
      setPersonaDraft(info.persona);
    } catch (e) {
      message.error(`读取人设失败：${String(e)}`);
      setPersonaOf(null);
    }
  };

  const savePersona = async () => {
    if (!personaOf) return;
    setSaving(true);
    try {
      await api.putPersona(personaOf.identifier, personaDraft);
      message.success("人设已保存，下一次派发热生效");
      const info = await api.getPersona(personaOf.identifier);
      setPersonaInfo(info);
    } catch (e) {
      message.error(`保存失败：${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const resetPersona = async () => {
    if (!personaOf) return;
    try {
      await api.resetPersona(personaOf.identifier);
      const info = await api.getPersona(personaOf.identifier);
      setPersonaInfo(info);
      setPersonaDraft(info.persona);
      message.success("已恢复默认智械体风格");
    } catch (e) {
      message.error(`重置失败：${String(e)}`);
    }
  };

  const grouped = agents.reduce<Record<string, AgentDefinition[]>>((acc, a) => {
    (acc[a.domain] ??= []).push(a);
    return acc;
  }, {});

  return (
    <div className="agents-wrap">
      <Space className="agents-toolbar" align="center">
        <span>
          激活组：
          <b>{activeMeta?.name ?? activeGroup}</b>
          {activeMeta?.builtin ? <Tag>内置</Tag> : <Tag color="purple">自定义</Tag>}
        </span>
        {!isBuiltin && (
          <Button type="primary" size="small" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>
            新建子个体
          </Button>
        )}
        {isBuiltin && (
          <Tooltip title="内置默认组的编成受保护；如需自定义集群请新建组">
            <span className="persona-hint">内置组编成受保护</span>
          </Tooltip>
        )}
      </Space>
      {Object.entries(grouped).map(([domain, list]) => (
        <Card key={domain} size="small" title={domain} className="agents-group">
          <Table
            size="small"
            rowKey="identifier"
            pagination={false}
            dataSource={list}
            columns={[
              {
                title: "个体",
                key: "name",
                render: (_, a) => (
                  <span>
                    {a.tier === "orchestrator" ? <Tag color="blue">指挥体</Tag> : null}
                    <b>{a.name}</b> <code>{a.identifier}</code>
                  </span>
                ),
              },
              { title: "职责", dataIndex: "description", key: "desc", ellipsis: true },
              {
                title: "工具",
                dataIndex: "tools",
                key: "tools",
                width: 200,
                render: (tools: string[]) => tools.map((t) => <Tag key={t}>{t}</Tag>),
              },
              {
                title: "状态",
                key: "state",
                width: 90,
                render: (_, a) =>
                  busyAgents.has(a.identifier) ? <Tag color="processing">执行中</Tag> : <Tag>空闲</Tag>,
              },
              {
                title: "操作",
                key: "ops",
                width: 150,
                render: (_, a) => (
                  <Space>
                    <Button size="small" icon={<EditOutlined />} onClick={() => void openPersona(a)}>
                      人设
                    </Button>
                    {!isBuiltin && a.identifier !== activeMeta?.primary && (
                      <Popconfirm title={`确认删除 ${a.identifier}？`} onConfirm={() => void removeOne(a.identifier)}>
                        <Button size="small" danger icon={<DeleteOutlined />} />
                      </Popconfirm>
                    )}
                  </Space>
                ),
              },
            ]}
          />
        </Card>
      ))}

      <Modal
        open={createOpen}
        title="新建子个体"
        onCancel={() => setCreateOpen(false)}
        onOk={() => void submitCreate()}
        okText="创建"
      >
        <Form form={form} layout="vertical">
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input placeholder="如：市场分析师" />
          </Form.Item>
          <Form.Item
            name="identifier"
            label="identifier"
            rules={[
              { required: true },
              { pattern: /^[A-Za-z0-9_-]{2,48}$/, message: "仅字母/数字/-/_,2–48字符" },
            ]}
          >
            <Input placeholder="如 market-analyst" />
          </Form.Item>
          <Form.Item name="description" label="职责描述" rules={[{ required: true }]}>
            <Input.TextArea rows={2} placeholder="该个体负责什么（会写入其提示词模板）" />
          </Form.Item>
          <Form.Item name="domain" label="领域标签" initialValue="自定义">
            <Input placeholder="如：市场域 / 产品域" />
          </Form.Item>
          <Form.Item name="prompt" label="职责提示词（可选；缺省按职责生成模板）">
            <Input.TextArea rows={4} placeholder={"# 名称\n\n你是 …（职责、规则、输出格式）"} />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        open={personaOf !== null}
        title={personaOf ? `人设编辑 — ${personaOf.name} (${personaOf.identifier})` : ""}
        onCancel={() => setPersonaOf(null)}
        width={640}
        footer={
          <Space>
            <Popconfirm
              title="恢复默认智械体风格？"
              onConfirm={() => void resetPersona()}
              disabled={personaInfo ? !personaInfo.custom : false}
            >
              <Button icon={<UndoOutlined />} disabled={personaInfo ? !personaInfo.custom : false}>
                恢复默认
              </Button>
            </Popconfirm>
            <Button onClick={() => setPersonaOf(null)}>关闭</Button>
            <Button type="primary" loading={saving} onClick={() => void savePersona()}>
              保存并热生效
            </Button>
          </Space>
        }
      >
        <Space direction="vertical" style={{ width: "100%" }} size="small">
          {personaInfo && (
            <div className="persona-status">
              {personaInfo.custom ? (
                <Tag color="gold">自定义人设</Tag>
              ) : (
                <Tag>默认智械体风格</Tag>
              )}
              <span className="persona-hint">
                人设影响该智能体的说话风格；保存在 agents/personas/{personaOf?.identifier}.md，下一次派发热生效。
              </span>
            </div>
          )}
          <Input.TextArea
            rows={10}
            value={personaDraft}
            onChange={(e) => setPersonaDraft(e.target.value)}
            placeholder="描述该个体的说话风格…"
          />
        </Space>
      </Modal>
    </div>
  );
}
