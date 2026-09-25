/** 智能体页（单体）：档案管理 —— 创建/删除/卡片菜单（切换模型 / 编辑描述 / 编辑 SOUL.MD）；交互切换在「对话」页左栏 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Dropdown, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message } from "antd";
import { MoreOutlined, PlusOutlined, ReloadOutlined, UndoOutlined, UserOutlined } from "@ant-design/icons";
import { api, type LlmProfile, type PersonaInfo } from "../api";
import { PageHeader } from "../components/PageHeader";
import { buildModelOptions, modelLabel } from "../models";
import { useT } from "../i18n/core";

interface SingleInfo {
  identifier: string;
  name: string;
  description: string;
  domain: string;
  capabilities?: string[];
  modelHint?: string | null;
}

export function SinglesView(): React.ReactElement {
  const t = useT();
  const [singles, setSingles] = useState<SingleInfo[]>([]);
  const [activeSingle, setActiveSingle] = useState<string | undefined>(undefined);
  const [profiles, setProfiles] = useState<LlmProfile[]>([]);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [modelTarget, setModelTarget] = useState<SingleInfo | null>(null);
  const [modelDraft, setModelDraft] = useState<string>("");
  const [descTarget, setDescTarget] = useState<SingleInfo | null>(null);
  const [descDraft, setDescDraft] = useState<string>("");
  const [soulOf, setSoulOf] = useState<SingleInfo | null>(null);
  const [soulInfo, setSoulInfo] = useState<PersonaInfo | null>(null);
  const [soulDraft, setSoulDraft] = useState<string>("");
  const [soulSaving, setSoulSaving] = useState(false);
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const r = await api.listSingles();
      setSingles(r.singles);
      setActiveSingle(r.active ?? undefined);
      setProfiles((await api.llmProfiles()).profiles);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const submit = async () => {
    const v = await form.validateFields();
    try {
      await api.createSingle({
        name: v.name,
        identifier: v.identifier,
        description: v.description,
        prompt: v.prompt || undefined,
      });
      message.success(t("singles.created", { id: v.identifier }));
      setModal(false);
      form.resetFields();
      await load();
    } catch (e) {
      message.error(t("singles.createFailed", { err: String(e) }));
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteSingle(id);
      message.success(t("singles.removed", { id }));
      await load();
    } catch (e) {
      message.error(t("singles.deleteFailed", { err: String(e) }));
    }
  };

  const saveModel = async () => {
    if (!modelTarget) return;
    try {
      await api.setAgentModel(modelTarget.identifier, modelDraft);
      message.success(t("singles.modelUpdated", { name: modelTarget.name }));
      setModelTarget(null);
      await load();
    } catch (e) {
      message.error(t("singles.setFailed", { err: String(e) }));
    }
  };

  const saveDesc = async () => {
    if (!descTarget) return;
    if (!descDraft.trim()) {
      message.warning(t("singles.f.descPh"));
      return;
    }
    try {
      await api.updateSingle(descTarget.identifier, { description: descDraft.trim() });
      message.success(t("singles.descSaved", { name: descTarget.name }));
      setDescTarget(null);
      await load();
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  const openSoul = async (s: SingleInfo) => {
    setSoulOf(s);
    setSoulInfo(null);
    setSoulDraft("");
    try {
      const info = await api.singlePersona(s.identifier);
      setSoulInfo(info);
      setSoulDraft(info.persona);
    } catch (e) {
      message.error(t("agents.personaReadFailed", { err: String(e) }));
      setSoulOf(null);
    }
  };

  const saveSoul = async () => {
    if (!soulOf) return;
    setSoulSaving(true);
    try {
      await api.singlesSetPersona(soulOf.identifier, soulDraft);
      message.success(t("singles.soulSaved"));
      setSoulInfo(await api.singlePersona(soulOf.identifier));
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    } finally {
      setSoulSaving(false);
    }
  };

  const resetSoul = async () => {
    if (!soulOf) return;
    try {
      await api.singlesResetPersona(soulOf.identifier);
      setSoulInfo(await api.singlePersona(soulOf.identifier));
      setSoulDraft((await api.singlePersona(soulOf.identifier)).persona);
      message.success(t("agents.resetBtn"));
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  const cardMenu = (s: SingleInfo) => [
    {
      key: "model",
      label: t("singles.menuModel"),
      onClick: () => {
        setModelTarget(s);
        setModelDraft(s.modelHint ?? "");
      },
    },
    {
      key: "desc",
      label: t("singles.menuDesc"),
      onClick: () => {
        setDescTarget(s);
        setDescDraft(s.description);
      },
    },
    {
      key: "soul",
      label: t("singles.menuSoul"),
      onClick: () => void openSoul(s),
    },
  ];

  return (
    <div className="pane-wrap">
      <PageHeader
        en="AGENTS"
        title={t("nav.agent")}
        desc={t("singles.desc")}
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setModal(true)}>
              {t("singles.new")}
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              {t("common.refresh")}
            </Button>
          </>
        }
      />

      <Spin spinning={loading}>
        <div className="skill-grid">
          {singles.map((s) => (
            <Card
              key={s.identifier}
              size="small"
              className="hud model-card"
              title={
                <Space>
                  <UserOutlined />
                  <span>{s.name}</span>
                  <span className="mono dim">{s.identifier}</span>
                  {activeSingle === s.identifier && <Tag color="processing">{t("singles.activeTag")}</Tag>}
                </Space>
              }
              extra={
                <Space>
                  <Popconfirm title={t("singles.deleteConfirm")} onConfirm={() => void remove(s.identifier)}>
                    <Button size="small" danger>
                      {t("common.delete")}
                    </Button>
                  </Popconfirm>
                  <Dropdown menu={{ items: cardMenu(s) }} trigger={["click"]} placement="bottomRight">
                    <Button size="small" icon={<MoreOutlined />} aria-label={t("singles.cardMenu")} />
                  </Dropdown>
                </Space>
              }
            >
              <div className="dim">{s.description}</div>
              <div className="skill-line">{t("singles.domain", { domain: s.domain || t("singles.solo") })}</div>
              <div className="skill-line">
                {t("singles.model")}<Tag color="geekblue">{modelLabel(s.modelHint, profiles)}</Tag>
              </div>
            </Card>
          ))}
        </div>
        {!loading && singles.length === 0 && (
          <Empty description={t("singles.empty")} className="graph-empty" />
        )}
      </Spin>

      {/* 切换模型（跟随全局 / 指定提供商与模型）——卡片菜单唯一默认模型入口 */}
      <Modal
        open={modelTarget !== null}
        title={t("singles.modelTitle", { name: modelTarget?.name ?? "" })}
        onCancel={() => setModelTarget(null)}
        onOk={() => void saveModel()}
        okText={t("common.save")}
        width={480}
      >
        <div className="pane-hint" style={{ marginBottom: 12 }}>
          {t("singles.modelHint")}
        </div>
        <Select
          value={modelDraft}
          onChange={setModelDraft}
          style={{ width: "100%" }}
          options={buildModelOptions(profiles)}
        />
      </Modal>

      {/* 编辑描述（轻量：仅简介字段） */}
      <Modal
        open={descTarget !== null}
        title={descTarget ? t("singles.descTitle", { name: descTarget.name }) : ""}
        onCancel={() => setDescTarget(null)}
        onOk={() => void saveDesc()}
        okText={t("common.save")}
        width={480}
      >
        <Input.TextArea
          rows={3}
          value={descDraft}
          onChange={(e) => setDescDraft(e.target.value)}
          placeholder={t("singles.f.descPh")}
        />
      </Modal>

      {/* 编辑 SOUL.MD（人设：自定义状态 + 恢复默认 + 文本编辑） */}
      <Modal
        open={soulOf !== null}
        title={soulOf ? t("singles.soulTitle", { name: soulOf.name }) : ""}
        onCancel={() => setSoulOf(null)}
        width={640}
        footer={
          <Space>
            <Popconfirm title={t("agents.resetConfirm")} onConfirm={() => void resetSoul()} disabled={soulInfo ? !soulInfo.custom : false}>
              <Button icon={<UndoOutlined />} disabled={soulInfo ? !soulInfo.custom : false}>
                {t("agents.resetBtn")}
              </Button>
            </Popconfirm>
            <Button onClick={() => setSoulOf(null)}>{t("common.close")}</Button>
            <Button type="primary" loading={soulSaving} onClick={() => void saveSoul()}>
              {t("agents.saveHot")}
            </Button>
          </Space>
        }
      >
        <Space direction="vertical" style={{ width: "100%" }} size="small">
          {soulInfo && (
            <div className="persona-status">
              {soulInfo.custom ? (
                <Tag color="gold">{t("agents.customPersona")}</Tag>
              ) : (
                <Tag>{t("agents.defaultPersona")}</Tag>
              )}
              <span className="persona-hint">{t("singles.soulHint", { id: soulOf?.identifier ?? "" })}</span>
            </div>
          )}
          <Input.TextArea
            rows={10}
            value={soulDraft}
            onChange={(e) => setSoulDraft(e.target.value)}
            placeholder={t("singles.soulPh")}
          />
        </Space>
      </Modal>

      <Modal
        open={modal}
        title={t("singles.new")}
        onCancel={() => setModal(false)}
        onOk={() => void submit()}
        okText={t("singles.create")}
      >
        <Form form={form} layout="vertical">
          <Form.Item name="name" label={t("singles.f.name")} rules={[{ required: true }]}>
            <Input placeholder={t("singles.f.namePh")} />
          </Form.Item>
          <Form.Item
            name="identifier"
            label={t("singles.f.identifier")}
            rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("singles.idPattern") }]}
          >
            <Input placeholder={t("singles.f.identifierPh")} />
          </Form.Item>
          <Form.Item name="description" label={t("singles.f.desc")} rules={[{ required: true }]}>
            <Input.TextArea rows={2} placeholder={t("singles.f.descPh")} />
          </Form.Item>
          <Form.Item name="prompt" label={t("singles.f.prompt")}>
            <Input.TextArea rows={4} placeholder={t("singles.f.promptPh")} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
