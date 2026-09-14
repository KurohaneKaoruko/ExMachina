/** 智能体组管理（独立页面）：组列表 + 成员管理 + 建组/组默认模型/组内个体增删（切组在「对话」页进行） */
import React, { useCallback, useEffect, useState } from "react";
import {
  Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message,
} from "antd";
import { PlusOutlined, UsergroupDeleteOutlined } from "@ant-design/icons";
import { api, type GroupMeta, type GroupOverview, type LlmProfile } from "../api";
import { PageHeader } from "../components/PageHeader";
import type { AgentDefinition } from "../types";
import { buildModelOptions } from "../models";
import { useExm } from "../store";

export function GroupsView(): React.ReactElement {
  const { groups, activeGroup, refreshAgents } = useExm();
  const [selected, setSelected] = useState<string>("");
  const [members, setMembers] = useState<AgentDefinition[]>([]);
  const [loading, setLoading] = useState(false);
  const [overview, setOverview] = useState<GroupOverview | null>(null);
  const [workspaceDraft, setWorkspaceDraft] = useState("");
  const [profiles, setProfiles] = useState<LlmProfile[]>([]);
  const [modelDraft, setModelDraft] = useState<string>("");
  const [groupModal, setGroupModal] = useState(false);
  const [agentModal, setAgentModal] = useState(false);
  const [groupForm] = Form.useForm();
  const [agentForm] = Form.useForm();

  const meta: GroupMeta | undefined = groups.find((g) => g.id === selected) ?? groups.find((g) => g.id === activeGroup);

  const loadMembers = useCallback(async (gid: string) => {
    if (!gid) return;
    setLoading(true);
    try {
      setMembers(await api.groupAgents(gid));
      setOverview(await api.groupOverview(gid));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const gid = selected || activeGroup;
    if (gid) void loadMembers(gid);
  }, [selected, activeGroup, loadMembers]);

  useEffect(() => {
    setWorkspaceDraft(meta?.workspace ?? "");
  }, [meta?.id, meta?.workspace]);

  useEffect(() => {
    void (async () => {
      try {
        setProfiles((await api.llmProfiles()).profiles);
      } catch {
        // 提供商接口不可用时选项退化为「跟随全局」
      }
    })();
  }, []);

  useEffect(() => {
    setModelDraft(meta?.model ?? "");
  }, [meta?.id, meta?.model]);

  const submitGroup = async () => {
    const v = await groupForm.validateFields();
    try {
      await api.createGroup({ name: v.name, id: v.id || undefined, description: v.description ?? "" });
      message.success(`组已创建：${v.name}`);
      setGroupModal(false);
      groupForm.resetFields();
      setSelected(v.id);
    } catch (e) {
      message.error(`创建失败：${String(e)}`);
    }
  };

  const submitAgent = async () => {
    const v = await agentForm.validateFields();
    try {
      await api.createAgent({
        name: v.name,
        identifier: v.identifier,
        description: v.description,
        tier: v.tier || undefined,
        group: meta?.id,
      });
      message.success(`个体已创建：${v.identifier}`);
      setAgentModal(false);
      agentForm.resetFields();
      await loadMembers(meta!.id);
      await refreshAgents();
    } catch (e) {
      message.error(`创建失败：${String(e)}`);
    }
  };

  const doDeleteGroup = async (gid: string) => {
    try {
      await api.deleteGroup(gid);
      message.success(`组已删除：${gid}`);
      if (selected === gid) setSelected("");
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  const doRemoveAgent = async (identifier: string) => {
    try {
      await fetch(`/api/agents/${identifier}`, { method: "DELETE" });
      await loadMembers(meta!.id);
      await refreshAgents();
      message.success(`个体已删除：${identifier}`);
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  const doSetPrimary = async (identifier: string) => {
    try {
      const resp = await fetch(`/api/groups/${meta!.id}/primary`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ identifier }),
      });
      if (!resp.ok) throw new Error(await resp.text());
      await loadMembers(meta!.id);
      message.success(`主智能体已设置：${identifier}`);
    } catch (e) {
      message.error(`设置失败：${String(e)}`);
    }
  };

  const isBuiltin = meta?.builtin ?? true;

  return (
    <div className="pane-wrap">
      <PageHeader
        en="GROUPS"
        title="智能体组"
        desc="组是编成与隔离的基本单位：默认组「智械集群」内置受保护，其他组可自由编成，主智能体可在任务中扩编组内个体。"
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setGroupModal(true)}>
              新建组
            </Button>
            <Button
              danger
              icon={<UsergroupDeleteOutlined />}
              disabled={!meta || meta.builtin || meta.id === activeGroup}
              onClick={() => meta && void doDeleteGroup(meta.id)}
            >
              删除组
            </Button>
          </>
        }
      />

      <div className="pane-body groups-layout">
        {/* 组列表 */}
        <div className="list-rail">
          {groups.map((g) => (
            <div
              key={g.id}
              className={`list-row ${(selected || activeGroup) === g.id ? "active" : ""}`}
              onClick={() => setSelected(g.id)}
            >
              <div className="list-row-head">
                <b>{g.name}</b>
                {g.builtin ? <Tag>内置</Tag> : <Tag color="purple">自定义</Tag>}
              </div>
              <div className="list-row-sub mono">{g.id}</div>
              {g.description && <div className="list-row-sub">{g.description}</div>}
            </div>
          ))}
        </div>

        {/* 组详情与成员 */}
        <div className="pane-detail">
          {!meta ? (
            <Empty description="选择或创建一个智能体组" className="graph-empty" />
          ) : (
            <Card
              size="small"
              className="hud"
              title={
                <Space>
                  <span>{meta.name}</span>
                  <span className="mono dim">{meta.id}</span>
                  {meta.builtin ? <Tag>内置 · 编成受保护</Tag> : <Tag color="purple">自定义组</Tag>}
                </Space>
              }
              extra={
                !isBuiltin && (
                  <Button size="small" type="primary" icon={<PlusOutlined />} onClick={() => setAgentModal(true)}>
                    新建子个体
                  </Button>
                )
              }
            >
              <div className="group-meta-line">
                <span className="meta-field">
                  主智能体：<span className="mono">{meta.primary ?? "未设置"}</span>
                </span>
                <span className="meta-field">描述：{meta.description || "—"}</span>
              </div>
              <div className="group-meta-line workspace-line">
                工作区：
                <Input
                  size="small"
                  className="mono workspace-input"
                  value={workspaceDraft}
                  placeholder="全局工作区（留空）或相对子目录，如 projects/demo"
                  onChange={(e) => setWorkspaceDraft(e.target.value)}
                />
                <Button
                  size="small"
                  onClick={() => {
                    void (async () => {
                      try {
                        await api.setGroupWorkspace(meta.id, workspaceDraft.trim());
                        message.success("工作区已更新（该组个体的文件与命令将以此为根）");
                      } catch (e) {
                        message.error(`设置失败：${String(e)}`);
                      }
                    })();
                  }}
                >
                  保存
                </Button>
              </div>
              <div className="group-meta-line workspace-line">
                默认模型：
                <Select
                  size="small"
                  style={{ minWidth: 240 }}
                  value={modelDraft}
                  onChange={setModelDraft}
                  options={buildModelOptions(profiles)}
                />
                <Button
                  size="small"
                  onClick={() => {
                    void (async () => {
                      try {
                        await api.setGroupModel(meta.id, modelDraft);
                        message.success("组默认模型已保存（组内未单独设置的个体与指挥体规划将随组生效）");
                      } catch (e) {
                        message.error(`设置失败：${String(e)}`);
                      }
                    })();
                  }}
                >
                  保存
                </Button>
                <span className="dim">未单独选择的个体/组将跟随「提供商」页的全局默认</span>
              </div>
              {overview && (
                <div className="overview-chips">
                  <div className="chip">
                    <span className="chip-num">{overview.agents}</span>
                    <span className="chip-label">个体 [UNITS]</span>
                  </div>
                  <div className="chip">
                    <span className="chip-num">{overview.playbooks}</span>
                    <span className="chip-label">链路 [CHAINS]</span>
                  </div>
                  <div className="chip">
                    <span className="chip-num">{overview.sessions}</span>
                    <span className="chip-label">会话 [SESS]</span>
                  </div>
                  <div className="chip">
                    <span className="chip-num">{overview.memory.group}</span>
                    <span className="chip-label">组记忆 [MEM]</span>
                  </div>
                  <div className="chip">
                    <span className="chip-num">{overview.memory.shared}</span>
                    <span className="chip-label">全局 [GLOBAL]</span>
                  </div>
                </div>
              )}
              {overview !== null && overview.agentStats.length > 0 && (
                <div className="member-stats">
                  <div className="rail-label">成员可靠性 [RELIABILITY]</div>
                  {overview.agentStats.slice(0, 6).map((s) => (
                    <div key={s.agentId} className="member-stat-row">
                      <span className="mono">{s.agentId}</span>
                      <span className="dim">
                        执行 {s.runs} · 完成 {s.done} · 受阻 {s.blocked} · 置信度 {s.avgConfidence.toFixed(2)}
                      </span>
                    </div>
                  ))}
                </div>
              )}
              <Spin spinning={loading}>
                <table className="plain-table">
                  <thead>
                    <tr>
                      <th>个体</th>
                      <th>职责</th>
                      <th>角色</th>
                      <th>操作</th>
                    </tr>
                  </thead>
                  <tbody>
                    {[...members]
                      .sort((a, b) => {
                        const pa = meta.primary === a.identifier ? 0 : 1;
                        const pb = meta.primary === b.identifier ? 0 : 1;
                        return pa - pb || a.identifier.localeCompare(b.identifier);
                      })
                      .map((a) => (
                      <tr key={a.identifier}>
                        <td>
                          <b>{a.name}</b> <span className="mono dim">{a.identifier}</span>
                        </td>
                        <td className="dim">{a.description}</td>
                        <td>
                          {meta.primary === a.identifier ? (
                            <Tag color="cyan">主智能体</Tag>
                          ) : (
                            <Tag>子个体</Tag>
                          )}
                        </td>
                        <td>
                          {!isBuiltin && meta.primary !== a.identifier && (
                            <Space>
                              <Button size="small" onClick={() => void doSetPrimary(a.identifier)}>
                                设为主智能体
                              </Button>
                              <Popconfirm
                                title="确认删除该个体？"
                                onConfirm={() => void doRemoveAgent(a.identifier)}
                              >
                                <Button size="small" danger>
                                  删除
                                </Button>
                              </Popconfirm>
                            </Space>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {members.length === 0 && (
                  <Empty description="组内暂无个体：创建首个个体将自动成为主智能体" className="pane-empty" />
                )}
              </Spin>
            </Card>
          )}
        </div>
      </div>

      <Modal
        open={groupModal}
        title="新建智能体组"
        onCancel={() => setGroupModal(false)}
        onOk={() => void submitGroup()}
        okText="创建"
      >
        <Form form={groupForm} layout="vertical">
          <Form.Item name="name" label="组名" rules={[{ required: true }]}>
            <Input placeholder="如：研发组" />
          </Form.Item>
          <Form.Item
            name="id"
            label="组 ID（可选）"
            rules={[{ pattern: /^[A-Za-z0-9_-]{1,48}$/, message: "仅字母/数字/-/_" }]}
          >
            <Input placeholder="留空自动生成" />
          </Form.Item>
          <Form.Item name="description" label="描述">
            <Input.TextArea rows={2} placeholder="该组的用途与架构（如：经理 → 员工）" />
          </Form.Item>
          <div className="persona-hint">
            新组为空组：创建第一个个体时它将自动成为主智能体，之后可由它或你继续扩编。
          </div>
        </Form>
      </Modal>

      <Modal
        open={agentModal}
        title={`在组「${meta?.name ?? ""}」中新建个体`}
        onCancel={() => setAgentModal(false)}
        onOk={() => void submitAgent()}
        okText="创建"
      >
        <Form form={agentForm} layout="vertical">
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input placeholder="如：主笔体" />
          </Form.Item>
          <Form.Item
            name="identifier"
            label="标识（identifier）"
            rules={[
              { required: true },
              { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: "仅字母/数字/-/_" },
            ]}
          >
            <Input placeholder="如：chief-writer" />
          </Form.Item>
          <Form.Item name="description" label="职责" rules={[{ required: true }]}>
            <Input.TextArea rows={2} placeholder="该个体的职责一句话描述" />
          </Form.Item>
          <Form.Item name="tier" label="层级" initialValue="unit">
            <Select
              options={[
                { value: "unit", label: "unit（子个体）" },
                { value: "orchestrator", label: "orchestrator（主智能体）" },
              ]}
            />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
