/** 智能体组页：组卡片总览 → 点入组详情（编成管理 / 组设置 / 子个体成员一体）。
 *  子个体是组的成员：成员的新建 / 编辑 / 人设 / 经验优化 / 删除都在组详情内完成（不再单设页面）。 */
import React, { useCallback, useEffect, useState } from "react";
import {
  Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Table, Tag, message,
} from "antd";
import {
  ArrowLeftOutlined, DeleteOutlined, EditOutlined, FormOutlined, PlusOutlined,
  SettingOutlined, UndoOutlined, UsergroupDeleteOutlined,
} from "@ant-design/icons";
import { api, type GroupCapabilities, type GroupMeta, type GroupOverview, type LlmProfile } from "../api";
import { PageHeader } from "../components/PageHeader";
import type { AgentDefinition } from "../types";
import { buildCapabilityOptions, buildModelOptions, groupIdEn } from "../models";
import { useExm } from "../store";
import { useT } from "../i18n/core";

export function GroupsView(): React.ReactElement {
  const t = useT();
  const { groups, activeGroup, refreshAgents } = useExm();
  /** null = 卡片总览；非空 = 已进入该组详情 */
  const [entered, setEntered] = useState<string | null>(null);
  const [members, setMembers] = useState<AgentDefinition[]>([]);
  const [loading, setLoading] = useState(false);
  const [overview, setOverview] = useState<GroupOverview | null>(null);
  const [profiles, setProfiles] = useState<LlmProfile[]>([]);
  const [modelDraft, setModelDraft] = useState<string>("");
  const [groupModal, setGroupModal] = useState(false);
  const [agentModal, setAgentModal] = useState(false);
  const [wsDraft, setWsDraft] = useState("");
  const [descDraft, setDescDraft] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [capsDraft, setCapsDraft] = useState<GroupCapabilities>({});
  const [groupForm] = Form.useForm();
  const [agentForm] = Form.useForm();
  // 成员编辑 / 人设（原「子个体」页能力并入组详情）
  const [editing, setEditing] = useState<AgentDefinition | null>(null);
  const [editForm] = Form.useForm();
  const [editFormProfiles, setEditFormProfiles] = useState<LlmProfile[]>([]);
  const [promptLoading, setPromptLoading] = useState(false);

  const meta: GroupMeta | undefined = groups.find((g) => g.id === entered) ?? undefined;

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
    if (entered) void loadMembers(entered);
  }, [entered, loadMembers]);

  useEffect(() => {
    void (async () => {
      try {
        const p = (await api.llmProfiles()).profiles;
        setProfiles(p);
        setEditFormProfiles(p);
      } catch { /* noop */ }
    })();
  }, []);

  const submitGroup = async () => {
    const v = await groupForm.validateFields();
    try {
      await api.createGroup({ name: v.name, id: v.id || undefined, description: v.description ?? "" });
      message.success(t("groups.created", { name: v.name }));
      setGroupModal(false);
      groupForm.resetFields();
      setEntered(v.id || "");
    } catch (e) {
      message.error(t("groups.createFailed", { err: String(e) }));
    }
  };

  const submitAgent = async () => {
    if (!meta) return;
    const v = await agentForm.validateFields();
    try {
      await api.createAgent({
        name: v.name,
        identifier: v.identifier,
        description: v.description,
        tier: v.tier || undefined,
        prompt: v.prompt || undefined,
        group: meta.id,
      });
      message.success(t("groups.agentCreated", { id: v.identifier }));
      setAgentModal(false);
      agentForm.resetFields();
      await loadMembers(meta.id);
      await refreshAgents();
    } catch (e) {
      message.error(t("groups.createFailed", { err: String(e) }));
    }
  };

  const doDeleteGroup = async (gid: string) => {
    try {
      await api.deleteGroup(gid);
      message.success(t("groups.groupDeleted", { id: gid }));
      setEntered(null);
    } catch (e) {
      message.error(t("groups.deleteFailed", { err: String(e) }));
    }
  };

  const doRemoveAgent = async (identifier: string) => {
    if (!meta) return;
    try {
      await api.removeGroupAgent(meta.id, identifier);
      await loadMembers(meta.id);
      await refreshAgents();
      message.success(t("groups.agentDeleted", { id: identifier }));
    } catch (e) {
      message.error(t("groups.deleteFailed", { err: String(e) }));
    }
  };

  const doSetPrimary = async (identifier: string) => {
    if (!meta) return;
    try {
      await api.setGroupPrimary(meta.id, identifier);
      await loadMembers(meta.id);
      message.success(t("groups.primarySet", { id: identifier }));
    } catch (e) {
      message.error(t("groups.setFailed", { err: String(e) }));
    }
  };

  const isBuiltin = meta?.builtin ?? true;

  // ---- 组设置（工作区 / 默认模型 / 能力覆盖） ----
  const openSettings = () => {
    setWsDraft(meta?.workspace ?? "");
    setDescDraft(meta?.description ?? "");
    setModelDraft(meta?.model ?? "");
    setCapsDraft(meta?.capabilities ?? {});
    setSettingsOpen(true);
  };

  const saveSettings = async () => {
    if (!meta) return;
    try {
      await api.setGroupWorkspace(meta.id, wsDraft.trim());
      await api.setGroupModel(meta.id, modelDraft);
      await api.setGroupCapabilities(meta.id, capsDraft);
      message.success(t("groups.settingsSaved"));
      setSettingsOpen(false);
      await loadMembers(meta.id);
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  const capField = (key: keyof GroupCapabilities, label: string) => (
    <div className="cap-row" key={key}>
      <div className="cap-label">{label}</div>
      <Select
        allowClear
        showSearch
        value={capsDraft[key] || undefined}
        options={buildCapabilityOptions(profiles)}
        placeholder={t("groups.capsFollow")}
        onChange={(v) => setCapsDraft((c) => ({ ...c, [key]: v ?? "" }))}
      />
    </div>
  );

  // ---- 成员编辑 / 人设（并入原「子个体」页能力） ----
  const openEdit = async (a: AgentDefinition) => {
    setEditing(a);
    editForm.setFieldsValue({
      name: a.name,
      description: a.description,
      modelHint: a.modelHint ?? undefined,
      prompt: "",
    });
    setPromptLoading(true);
    try {
      const info = await api.getAgentPrompt(meta!.id, a.identifier);
      editForm.setFieldsValue({ prompt: info.prompt });
    } catch {
      editForm.setFieldsValue({ prompt: "" });
      message.info(t("agents.promptMissing"));
    } finally {
      setPromptLoading(false);
    }
  };

  const submitEdit = async () => {
    if (!editing || !meta) return;
    const v = await editForm.validateFields();
    try {
      await api.updateAgent(editing.identifier, {
        name: v.name,
        description: v.description,
        modelHint: v.modelHint ?? "",
      });
      if ((v.prompt ?? "").trim()) {
        await api.putAgentPrompt(meta.id, editing.identifier, v.prompt);
      }
      message.success(t("agents.editSaved", { name: v.name }));
      setEditing(null);
      await loadMembers(meta.id);
      await refreshAgents();
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  // 在途个体（任务图运行中）：成员表状态列
  const busyAgents = new Set(
    (useExm((s) => s.graph)?.nodes ?? [])
      .filter((n) => ["running", "dispatched", "syncing"].includes(n.status))
      .map((n) => n.agentIdentifier),
  );

  // ================= 组详情视图 =================
  if (entered && meta) {
    return (
      <div className="pane-wrap">
        <PageHeader
          en="GROUP"
          title={meta.name}
          desc={
            <Space size={6} wrap>
              <Button size="small" icon={<ArrowLeftOutlined />} onClick={() => setEntered(null)}>
                {t("groups.back")}
              </Button>
              <span className="mono dim">{groupIdEn(meta.id)}</span>
              {meta.builtin ? <Tag>{t("groups.builtinProtected")}</Tag> : <Tag color="purple">{t("groups.customGroup")}</Tag>}
              {meta.id === activeGroup && <Tag color="cyan">{t("groups.activeTag")}</Tag>}
            </Space>
          }
          actions={
            <Space>
              <Button icon={<SettingOutlined />} onClick={() => { openSettings(); }}>
                {t("groups.settings")}
              </Button>
              <Popconfirm
                title={t("groups.groupDeleteConfirm", { name: meta.name })}
                disabled={isBuiltin || meta.id === activeGroup}
                onConfirm={() => void doDeleteGroup(meta.id)}
              >
                <Button danger icon={<UsergroupDeleteOutlined />} disabled={isBuiltin || meta.id === activeGroup}>
                  {t("groups.del")}
                </Button>
              </Popconfirm>
              {!isBuiltin && (
                <Button type="primary" icon={<PlusOutlined />} onClick={() => setAgentModal(true)}>
                  {t("groups.newAgent")}
                </Button>
              )}
            </Space>
          }
        />

        <div className="pane-body">
          {overview && (
            <div className="overview-chips">
              <div className="chip"><span className="chip-num">{overview.agents}</span><span className="chip-label">{t("groups.chipAgents")}</span></div>
              <div className="chip"><span className="chip-num">{overview.playbooks}</span><span className="chip-label">{t("groups.chipPlaybooks")}</span></div>
              <div className="chip"><span className="chip-num">{overview.sessions}</span><span className="chip-label">{t("groups.chipSessions")}</span></div>
              <div className="chip"><span className="chip-num">{overview.memory.group}</span><span className="chip-label">{t("groups.chipGroupMem")}</span></div>
              <div className="chip"><span className="chip-num">{overview.memory.shared}</span><span className="chip-label">{t("groups.chipShared")}</span></div>
            </div>
          )}
          {overview && overview.agentStats.length > 0 && (
            <div className="member-stats">
              <div className="rail-label">{t("groups.reliability")}</div>
              {overview.agentStats.slice(0, 6).map((s) => (
                <div key={s.agentId} className="member-stat-row">
                  <span className="mono">{s.agentId}</span>
                  <span className="dim">
                    {t("groups.stat", { runs: s.runs, done: s.done, blocked: s.blocked, conf: s.avgConfidence.toFixed(2) })}
                  </span>
                </div>
              ))}
            </div>
          )}

          <Card size="small" className="hud" title={t("groups.membersTitle")}>
            <Spin spinning={loading}>
              <Table
                size="small"
                rowKey="identifier"
                pagination={false}
                dataSource={[...members].sort((a, b) => {
                  const pa = meta.primary === a.identifier ? 0 : 1;
                  const pb = meta.primary === b.identifier ? 0 : 1;
                  return pa - pb || a.identifier.localeCompare(b.identifier);
                })}
                columns={[
                  {
                    title: t("agents.colUnit"),
                    key: "name",
                    render: (_, a: AgentDefinition) => (
                      <span>
                        <b>{a.name}</b>
                        {meta.primary === a.identifier && <Tag color="blue">{t("agents.primaryTag")}</Tag>}
                        <code>{a.identifier}</code>
                      </span>
                    ),
                  },
                  { title: t("agents.colDuty"), dataIndex: "description", key: "desc", ellipsis: true },
                  {
                    title: t("agents.colTools"),
                    dataIndex: "tools",
                    key: "tools",
                    width: 180,
                    render: (tools: string[]) => (tools ?? []).map((tl) => <Tag key={tl}>{tl}</Tag>),
                  },
                  {
                    title: t("agents.colState"),
                    key: "state",
                    width: 84,
                    render: (_, a: AgentDefinition) =>
                      busyAgents.has(a.identifier) ? <Tag color="processing">{t("agents.busy")}</Tag> : <Tag>{t("agents.idle")}</Tag>,
                  },
                  {
                    title: t("agents.colOps"),
                    key: "ops",
                    width: 220,
                    render: (_, a: AgentDefinition) => (
                      <Space size={4}>
                        <Button size="small" icon={<FormOutlined />} onClick={() => void openEdit(a)}>
                          {t("agents.edit")}
                        </Button>
                        {!isBuiltin && meta.primary !== a.identifier && (
                          <>
                            <Button size="small" onClick={() => void doSetPrimary(a.identifier)}>
                              {t("groups.setPrimary")}
                            </Button>
                            <Popconfirm title={t("groups.agentDeleteConfirm")} onConfirm={() => void doRemoveAgent(a.identifier)}>
                              <Button size="small" danger icon={<DeleteOutlined />} />
                            </Popconfirm>
                          </>
                        )}
                      </Space>
                    ),
                  },
                ]}
              />
              {members.length === 0 && <Empty description={t("groups.emptyMembers")} className="pane-empty" />}
            </Spin>
          </Card>
        </div>

        <Modal open={groupModal} title={t("groups.modalNew")} onCancel={() => setGroupModal(false)} onOk={() => void submitGroup()} okText={t("groups.create")}>
          <Form form={groupForm} layout="vertical">
            <Form.Item name="name" label={t("groups.f.name")} rules={[{ required: true }]}>
              <Input placeholder={t("groups.f.namePh")} />
            </Form.Item>
            <Form.Item name="id" label={t("groups.f.id")} rules={[{ pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("groups.idPattern") }]}>
              <Input placeholder={t("groups.idAuto")} />
            </Form.Item>
            <Form.Item name="description" label={t("groups.f.desc")}>
              <Input.TextArea rows={2} placeholder={t("groups.f.descPh")} />
            </Form.Item>
            <div className="persona-hint">{t("groups.newHint")}</div>
          </Form>
        </Modal>

        <Modal open={agentModal} title={t("groups.modalNewAgent", { name: meta.name })} onCancel={() => setAgentModal(false)} onOk={() => void submitAgent()} okText={t("groups.create")}>
          <Form form={agentForm} layout="vertical">
            <Form.Item name="name" label={t("groups.f.agentName")} rules={[{ required: true }]}>
              <Input placeholder={t("groups.f.agentNamePh")} />
            </Form.Item>
            <Form.Item name="identifier" label={t("groups.f.identifier")} rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("groups.idPattern") }]}>
              <Input placeholder={t("groups.f.identifierPh")} />
            </Form.Item>
            <Form.Item name="description" label={t("groups.duty")} rules={[{ required: true }]}>
              <Input.TextArea rows={2} placeholder={t("groups.f.dutyPh")} />
            </Form.Item>
            <Form.Item name="prompt" label={t("agents.f.prompt")} extra={t("agents.f.promptExtra")}>
              <Input.TextArea rows={4} placeholder={t("agents.f.promptPh")} />
            </Form.Item>
            <Form.Item name="tier" label={t("groups.f.tier")} initialValue="unit">
              <Select options={[{ value: "unit", label: t("groups.tier.unit") }, { value: "orchestrator", label: t("groups.tier.orch") }]} />
            </Form.Item>
          </Form>
        </Modal>

        {/* 成员编辑弹窗 */}
        <Modal
          open={editing !== null}
          title={editing ? t("agents.editTitle", { name: editing.name }) : ""}
          onCancel={() => setEditing(null)}
          onOk={() => void submitEdit()}
          okText={t("common.save")}
        >
          <Form form={editForm} layout="vertical">
            <Form.Item name="name" label={t("agents.f.name")} rules={[{ required: true }]}>
              <Input />
            </Form.Item>
            <Form.Item name="description" label={t("agents.f.duty")} rules={[{ required: true }]}>
              <Input.TextArea rows={2} />
            </Form.Item>
            <Spin spinning={promptLoading}>
              <Form.Item
                name="prompt"
                label={t("agents.f.promptTitle")}
                extra={t("agents.f.promptEditExtra")}
                rules={[{ required: true, whitespace: true, message: t("models.modelNameRequired") }]}
              >
                <Input.TextArea rows={12} className="prompt-editor" spellCheck={false} />
              </Form.Item>
            </Spin>
            <Form.Item name="modelHint" label={t("agents.f.model")} extra={t("agents.f.modelExtra")}>
              <Select
                allowClear
                showSearch
                style={{ width: "100%" }}
                options={buildModelOptions(editFormProfiles)}
                placeholder={t("agents.f.modelPh")}
              />
            </Form.Item>
          </Form>
        </Modal>


        {/* 组设置弹窗 */}
        <Modal
          open={settingsOpen}
          title={t("groups.modalSettings", { name: meta.name })}
          onCancel={() => setSettingsOpen(false)}
          onOk={() => void saveSettings()}
          okText={t("common.save")}
          width={520}
        >
          <Form layout="vertical">
            <Form.Item label={t("groups.f.workspace")} help={t("groups.f.workspaceHelp")}>
              <Input value={wsDraft} onChange={(e) => setWsDraft(e.target.value)} placeholder={t("groups.f.wsPh")} />
            </Form.Item>
            <Form.Item label={t("groups.f.model")} help={t("groups.f.modelHelp")}>
              <Select value={modelDraft || undefined} onChange={setModelDraft} options={buildModelOptions(profiles)} allowClear />
            </Form.Item>
            <Form.Item label={t("groups.capsTitle")} help={t("groups.capsHint")} style={{ marginBottom: 8 }}>
              <div className="cap-grid" style={{ gap: "8px 14px" }}>
                {capField("speech", t("models.capSpeech"))}
                {capField("transcribe", t("models.capStt"))}
                {capField("visionRelay", t("models.capVisionRelay"))}
                {capField("embedding", t("models.capEmbed"))}
              </div>
            </Form.Item>
          </Form>
        </Modal>
      </div>
    );
  }

  // ================= 卡片总览视图 =================
  return (
    <div className="pane-wrap">
      <PageHeader
        en="GROUPS"
        title={t("nav.groups")}
        desc={t("groups.desc")}
        actions={
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setGroupModal(true)}>
            {t("groups.new")}
          </Button>
        }
      />

      <div className="pane-body">
        <div className="group-grid">
          {groups.map((g) => (
            <Card
              key={g.id}
              size="small"
              className="group-card hud"
              onClick={() => setEntered(g.id)}
              title={
                <div className="group-card-head">
                  <span className="group-card-name">{g.name}</span>
                  <span className="group-card-id">{groupIdEn(g.id)}</span>
                </div>
              }
            >
              <div className="group-card-meta">
                {g.builtin ? <Tag>{t("groups.builtin")}</Tag> : <Tag color="purple">{t("groups.custom")}</Tag>}
                {g.id === activeGroup && <Tag color="cyan">{t("groups.activeTag")}</Tag>}
                {g.primary && <Tag color="blue">{t("groups.primaryShort")}: {g.primary}</Tag>}
              </div>
              {g.description && <div className="group-card-desc">{g.description}</div>}
            </Card>
          ))}
        </div>
        {groups.length === 0 && <Empty description={t("groups.none")} className="pane-empty" />}
      </div>

      <Modal open={groupModal} title={t("groups.modalNew")} onCancel={() => setGroupModal(false)} onOk={() => void submitGroup()} okText={t("groups.create")}>
        <Form form={groupForm} layout="vertical">
          <Form.Item name="name" label={t("groups.f.name")} rules={[{ required: true }]}>
            <Input placeholder={t("groups.f.namePh")} />
          </Form.Item>
          <Form.Item name="id" label={t("groups.f.id")} rules={[{ pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("groups.idPattern") }]}>
            <Input placeholder={t("groups.idAuto")} />
          </Form.Item>
          <Form.Item name="description" label={t("groups.f.desc")}>
            <Input.TextArea rows={2} placeholder={t("groups.f.descPh")} />
          </Form.Item>
          <div className="persona-hint">{t("groups.newHint")}</div>
        </Form>
      </Modal>
    </div>
  );
}
