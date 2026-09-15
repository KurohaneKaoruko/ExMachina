/** 智能体组页：组列表 + 编成管理 + 组设置弹窗（切组在「对话」页左栏进行） */
import React, { useCallback, useEffect, useState } from "react";
import {
  Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message,
} from "antd";
import { PlusOutlined, SettingOutlined, UsergroupDeleteOutlined } from "@ant-design/icons";
import { api, type GroupMeta, type GroupOverview, type LlmProfile } from "../api";
import { PageHeader } from "../components/PageHeader";
import type { AgentDefinition } from "../types";
import { buildModelOptions } from "../models";
import { useExm } from "../store";
import { useT } from "../i18n/core";

export function GroupsView(): React.ReactElement {
  const t = useT();
  const { groups, activeGroup, refreshAgents } = useExm();
  const [selected, setSelected] = useState<string>("");
  const [members, setMembers] = useState<AgentDefinition[]>([]);
  const [loading, setLoading] = useState(false);
  const [overview, setOverview] = useState<GroupOverview | null>(null);
  const [profiles, setProfiles] = useState<LlmProfile[]>([]);
  const [modelDraft, setModelDraft] = useState<string>("");
  const [groupModal, setGroupModal] = useState(false);
  const [agentModal, setAgentModal] = useState(false);
  const [settingsModal, setSettingsModal] = useState(false);
  const [wsDraft, setWsDraft] = useState("");
  const [descDraft, setDescDraft] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
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
    void (async () => {
      try { setProfiles((await api.llmProfiles()).profiles); } catch { /* noop */ }
    })();
  }, []);

  const submitGroup = async () => {
    const v = await groupForm.validateFields();
    try {
      await api.createGroup({ name: v.name, id: v.id || undefined, description: v.description ?? "" });
      message.success(t("groups.created", { name: v.name }));
      setGroupModal(false);
      groupForm.resetFields();
      setSelected(v.id);
    } catch (e) {
      message.error(t("groups.createFailed", { err: String(e) }));
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
      message.success(t("groups.agentCreated", { id: v.identifier }));
      setAgentModal(false);
      agentForm.resetFields();
      await loadMembers(meta!.id);
      await refreshAgents();
    } catch (e) {
      message.error(t("groups.createFailed", { err: String(e) }));
    }
  };

  const doDeleteGroup = async (gid: string) => {
    try {
      await api.deleteGroup(gid);
      message.success(t("groups.groupDeleted", { id: gid }));
      if (selected === gid) setSelected("");
    } catch (e) {
      message.error(t("groups.deleteFailed", { err: String(e) }));
    }
  };

  const doRemoveAgent = async (identifier: string) => {
    try {
      await fetch(`/api/agents/${identifier}`, { method: "DELETE" });
      await loadMembers(meta!.id);
      await refreshAgents();
      message.success(t("groups.agentDeleted", { id: identifier }));
    } catch (e) {
      message.error(t("groups.deleteFailed", { err: String(e) }));
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
      message.success(t("groups.primarySet", { id: identifier }));
    } catch (e) {
      message.error(t("groups.setFailed", { err: String(e) }));
    }
  };

  const isBuiltin = meta?.builtin ?? true;

  // 打开设置弹窗时从 meta 同步草稿
  const openSettings = () => {
    setWsDraft(meta?.workspace ?? "");
    setDescDraft(meta?.description ?? "");
    setModelDraft(meta?.model ?? "");
    setSettingsOpen(true);
  };

  const saveSettings = async () => {
    if (!meta) return;
    try {
      await api.setGroupWorkspace(meta.id, wsDraft.trim());
      await api.setGroupModel(meta.id, modelDraft);
      message.success(t("groups.settingsSaved"));
      setSettingsOpen(false);
      await loadMembers(meta.id);
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="GROUPS"
        title={t("nav.groups")}
        desc={t("groups.desc")}
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setGroupModal(true)}>
              {t("groups.new")}
            </Button>
            <Button
              danger
              icon={<UsergroupDeleteOutlined />}
              disabled={!meta || meta.builtin || meta.id === activeGroup}
              onClick={() => meta && void doDeleteGroup(meta.id)}
            >
              {t("groups.del")}
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
                {g.builtin ? <Tag>{t("groups.builtin")}</Tag> : <Tag color="purple">{t("groups.custom")}</Tag>}
              </div>
              <div className="list-row-sub mono">{g.id}</div>
              {g.description && <div className="list-row-sub">{g.description}</div>}
            </div>
          ))}
        </div>

        {/* 组详情与成员 */}
        <div className="pane-detail">
          {!meta ? (
            <Empty description={t("groups.emptyPick")} className="graph-empty" />
          ) : (
            <Card
              size="small"
              className="hud"
              title={
                <Space>
                  <span>{meta.name}</span>
                  <span className="mono dim">{meta.id}</span>
                  {meta.builtin ? <Tag>{t("groups.builtinProtected")}</Tag> : <Tag color="purple">{t("groups.customGroup")}</Tag>}
                </Space>
              }
              extra={
                <Space>
                  <Button size="small" icon={<SettingOutlined />} onClick={() => { openSettings(); }}>
                    {t("groups.settings")}
                  </Button>
                  {!isBuiltin && (
                    <Button size="small" type="primary" icon={<PlusOutlined />} onClick={() => setAgentModal(true)}>
                      {t("groups.newAgent")}
                    </Button>
                  )}
                </Space>
              }
            >
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
              <Spin spinning={loading}>
                <table className="plain-table">
                  <thead>
                    <tr><th>{t("groups.unit")}</th><th>{t("groups.duty")}</th><th>{t("groups.role")}</th><th>{t("groups.actions")}</th></tr>
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
                          {meta.primary === a.identifier ? <Tag color="cyan">{t("groups.primary")}</Tag> : <Tag>{t("groups.unit")}</Tag>}
                        </td>
                        <td>
                          {!isBuiltin && meta.primary !== a.identifier && (
                            <Space>
                              <Button size="small" onClick={() => void doSetPrimary(a.identifier)}>
                                {t("groups.setPrimary")}
                              </Button>
                              <Popconfirm title={t("groups.agentDeleteConfirm")} onConfirm={() => void doRemoveAgent(a.identifier)}>
                                <Button size="small" danger>{t("common.delete")}</Button>
                              </Popconfirm>
                            </Space>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {members.length === 0 && (
                  <Empty description={t("groups.emptyMembers")} className="pane-empty" />
                )}
              </Spin>
            </Card>
          )}
        </div>
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
          <div className="persona-hint">
            {t("groups.newHint")}
          </div>
        </Form>
      </Modal>

      <Modal open={agentModal} title={t("groups.modalNewAgent", { name: meta?.name ?? "" })} onCancel={() => setAgentModal(false)} onOk={() => void submitAgent()} okText={t("groups.create")}>
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
          <Form.Item name="tier" label={t("groups.f.tier")} initialValue="unit">
            <Select options={[{ value: "unit", label: t("groups.tier.unit") }, { value: "orchestrator", label: t("groups.tier.orch") }]} />
          </Form.Item>
        </Form>
      </Modal>

      {/* 组设置弹窗 */}
      <Modal
        open={settingsOpen}
        title={t("groups.modalSettings", { name: meta?.name ?? "" })}
        onCancel={() => setSettingsOpen(false)}
        onOk={() => void saveSettings()}
        okText={t("common.save")}
        width={520}
      >
        <Form layout="vertical">
          <Form.Item label={t("groups.f.workspace")} help={t("groups.f.workspaceHelp")}>
            <Input
              value={wsDraft}
              onChange={(e) => setWsDraft(e.target.value)}
              placeholder={t("groups.f.wsPh")}
            />
          </Form.Item>
          <Form.Item label={t("groups.f.model")} help={t("groups.f.modelHelp")}>
            <Select
              value={modelDraft}
              onChange={setModelDraft}
              options={buildModelOptions(profiles)}
            />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
