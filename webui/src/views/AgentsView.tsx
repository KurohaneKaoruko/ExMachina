/** 子个体页：任选智能体组查看/管理其子个体（人设、经验优化、新建/删除）；交互切换在对话页 */
import React, { useCallback, useEffect, useState } from "react";
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
} from "antd";
import { DeleteOutlined, EditOutlined, PlusOutlined, UndoOutlined } from "@ant-design/icons";
import { api, type PersonaInfo } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";
import type { AgentDefinition } from "../types";

interface GroupLite {
  id: string;
  name: string;
  builtin: boolean;
  primary?: string;
}

/** 主智能体置顶，其余按 identifier 排序 */
function sortMembers(list: AgentDefinition[], primary?: string): AgentDefinition[] {
  return [...list].sort((a, b) => {
    const pa = a.identifier === primary ? 0 : 1;
    const pb = b.identifier === primary ? 0 : 1;
    return pa - pb || a.identifier.localeCompare(b.identifier);
  });
}

export function AgentsView(): React.ReactElement {
  const groups = useExm((s) => s.groups);
  const activeGroup = useExm((s) => s.activeGroup);
  const [gid, setGid] = useState<string>("");
  const [members, setMembers] = useState<AgentDefinition[]>([]);
  const [loading, setLoading] = useState(false);
  const [personaOf, setPersonaOf] = useState<AgentDefinition | null>(null);
  const [personaInfo, setPersonaInfo] = useState<PersonaInfo | null>(null);
  const [personaDraft, setPersonaDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [form] = Form.useForm();

  const meta = groups.find((g) => g.id === gid);
  const isBuiltin = meta?.builtin ?? true;
  const primary = meta?.primary;

  const loadMembers = useCallback(async (id: string) => {
    if (!id) return;
    setMembers(sortMembers(await api.groupAgents(id)));
  }, []);

  // 初次进入：默认选中当前交互组（会话上下文）
  useEffect(() => {
    if (!gid && groups.length > 0) {
      setGid(groups.some((g) => g.id === activeGroup) ? activeGroup : groups[0].id);
    }
  }, [groups, activeGroup, gid]);

  useEffect(() => {
    void loadMembers(gid);
  }, [gid, loadMembers]);

  const reload = useCallback(async () => {
    await loadMembers(gid);
  }, [gid, loadMembers]);

  const submitCreate = async () => {
    const v = await form.validateFields();
    try {
      await api.createAgent({
        name: v.name,
        identifier: v.identifier,
        description: v.description,
        tier: v.tier || "unit",
        prompt: v.prompt || undefined,
        group: gid || undefined,
      });
      message.success(`子个体已创建：${v.name}`);
      setCreateOpen(false);
      form.resetFields();
      await reload();
    } catch (e) {
      message.error(`创建失败：${String(e)}`);
    }
  };

  const removeOne = async (identifier: string) => {
    try {
      await api.removeGroupAgent(gid, identifier);
      message.success(`已删除：${identifier}`);
      await reload();
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  const busyAgents = new Set(
    (useExm((s) => s.graph)?.nodes ?? [])
      .filter((n) => ["running", "dispatched", "syncing"].includes(n.status))
      .map((n) => n.agentIdentifier),
  );

  const openPersona = async (a: AgentDefinition) => {
    setPersonaOf(a);
    setPersonaInfo(null);
    setPersonaDraft("");
    try {
      setPersonaInfo(await api.getPersona(a.identifier));
      const info = await api.getPersona(a.identifier);
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
      setPersonaInfo(await api.getPersona(personaOf.identifier));
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
      setPersonaInfo(await api.getPersona(personaOf.identifier));
      setPersonaDraft(``);
      message.success("已恢复默认智械体风格");
    } catch (e) {
      message.error(`重置失败：${String(e)}`);
    }
  };

  return (
    <div className="agents-wrap">
      <PageHeader
        title="子个体"
        desc={
          isBuiltin
            ? "当前查看内置组的编成（受保护，不可增删）；人设与经验优化可在此调整。"
            : "当前查看自定义组的编成：可新建 / 删除个体，并调整人设与经验优化。"
        }
        actions={
          <>
            <Select
              style={{ minWidth: 200 }}
              value={gid || undefined}
              onChange={(v) => setGid(v)}
              options={groups.map((g) => ({ value: g.id, label: `${g.name}（${g.id}）` }))}
            />
            {!isBuiltin && (
              <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>
                新建子个体
              </Button>
            )}
          </>
        }
      />

      <Card size="small" className="agents-group">
        <Table
          size="small"
          rowKey="identifier"
          pagination={false}
          dataSource={sortMembers(members, primary)}
          columns={[
            {
              title: "子个体",
              key: "name",
              render: (_, a) => (
                <span>
                  {a.tier === "orchestrator" || a.identifier === primary ? <Tag color="blue">主智能体</Tag> : null}
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
                  {!isBuiltin && a.identifier !== primary && (
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
            <Input.TextArea rows={2} placeholder="该子个体负责什么（会写入其提示词模板）" />
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
