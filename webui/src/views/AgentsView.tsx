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
import { useT } from "../i18n/core";
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
  const t = useT();
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
      message.success(t("agents.created", { name: v.name }));
      setCreateOpen(false);
      form.resetFields();
      await reload();
    } catch (e) {
      message.error(t("agents.createFailed", { err: String(e) }));
    }
  };

  const removeOne = async (identifier: string) => {
    try {
      await api.removeGroupAgent(gid, identifier);
      message.success(t("agents.removed", { id: identifier }));
      await reload();
    } catch (e) {
      message.error(t("agents.deleteFailed", { err: String(e) }));
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
      message.error(t("agents.personaReadFailed", { err: String(e) }));
      setPersonaOf(null);
    }
  };

  const savePersona = async () => {
    if (!personaOf) return;
    setSaving(true);
    try {
      await api.putPersona(personaOf.identifier, personaDraft);
      message.success(t("agents.personaSaved"));
      setPersonaInfo(await api.getPersona(personaOf.identifier));
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
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
      message.success(t("agents.personaReset"));
    } catch (e) {
      message.error(t("agents.resetFailed", { err: String(e) }));
    }
  };

  return (
    <div className="agents-wrap">
      <PageHeader
        en="UNITS"
        title={t("nav.units")}
        desc={
          isBuiltin
            ? t("agents.descBuiltin")
            : t("agents.descCustom")
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
                {t("agents.new")}
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
              title: t("agents.colUnit"),
              key: "name",
              render: (_, a) => (
                <span>
                  {a.tier === "orchestrator" || a.identifier === primary ? <Tag color="blue">{t("agents.primaryTag")}</Tag> : null}
                  <b>{a.name}</b> <code>{a.identifier}</code>
                </span>
              ),
            },
            { title: t("agents.colDuty"), dataIndex: "description", key: "desc", ellipsis: true },
            {
              title: t("agents.colTools"),
              dataIndex: "tools",
              key: "tools",
              width: 200,
              render: (tools: string[]) => tools.map((tl) => <Tag key={tl}>{tl}</Tag>),
            },
            {
              title: t("agents.colState"),
              key: "state",
              width: 90,
              render: (_, a) =>
                busyAgents.has(a.identifier) ? <Tag color="processing">{t("agents.busy")}</Tag> : <Tag>{t("agents.idle")}</Tag>,
            },
            {
              title: t("agents.colOps"),
              key: "ops",
              width: 150,
              render: (_, a) => (
                <Space>
                  <Button size="small" icon={<EditOutlined />} onClick={() => void openPersona(a)}>
                    {t("agents.personaBtn")}
                  </Button>
                  {!isBuiltin && a.identifier !== primary && (
                    <Popconfirm title={t("agents.deleteConfirm", { id: a.identifier })} onConfirm={() => void removeOne(a.identifier)}>
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
        title={t("agents.new")}
        onCancel={() => setCreateOpen(false)}
        onOk={() => void submitCreate()}
        okText={t("agents.create")}
      >
        <Form form={form} layout="vertical">
          <Form.Item name="name" label={t("agents.f.name")} rules={[{ required: true }]}>
            <Input placeholder={t("agents.f.namePh")} />
          </Form.Item>
          <Form.Item
            name="identifier"
            label="identifier"
            rules={[
              { required: true },
              { pattern: /^[A-Za-z0-9_-]{2,48}$/, message: t("agents.idPattern") },
            ]}
          >
            <Input placeholder={t("agents.f.identifierPh")} />
          </Form.Item>
          <Form.Item name="description" label={t("agents.f.duty")} rules={[{ required: true }]}>
            <Input.TextArea rows={2} placeholder={t("agents.f.dutyPh")} />
          </Form.Item>
          <Form.Item name="prompt" label={t("agents.f.prompt")}>
            <Input.TextArea rows={4} placeholder={t("agents.f.promptPh")} />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        open={personaOf !== null}
        title={personaOf ? t("agents.personaTitle", { name: personaOf.name, id: personaOf.identifier }) : ""}
        onCancel={() => setPersonaOf(null)}
        width={640}
        footer={
          <Space>
            <Popconfirm
              title={t("agents.resetConfirm")}
              onConfirm={() => void resetPersona()}
              disabled={personaInfo ? !personaInfo.custom : false}
            >
              <Button icon={<UndoOutlined />} disabled={personaInfo ? !personaInfo.custom : false}>
                {t("agents.resetBtn")}
              </Button>
            </Popconfirm>
            <Button onClick={() => setPersonaOf(null)}>{t("common.close")}</Button>
            <Button type="primary" loading={saving} onClick={() => void savePersona()}>
              {t("agents.saveHot")}
            </Button>
          </Space>
        }
      >
        <Space direction="vertical" style={{ width: "100%" }} size="small">
          {personaInfo && (
            <div className="persona-status">
              {personaInfo.custom ? (
                <Tag color="gold">{t("agents.customPersona")}</Tag>
              ) : (
                <Tag>{t("agents.defaultPersona")}</Tag>
              )}
              <span className="persona-hint">
                {t("agents.personaHint", { id: personaOf?.identifier ?? "" })}
              </span>
            </div>
          )}
          <Input.TextArea
            rows={10}
            value={personaDraft}
            onChange={(e) => setPersonaDraft(e.target.value)}
            placeholder={t("agents.personaPh")}
          />
        </Space>
      </Modal>
    </div>
  );
}
