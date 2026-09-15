/** 智能体页（单体）：档案管理 —— 创建/删除/默认模型设置；交互切换在「对话」页左栏进行 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message } from "antd";
import { PlusOutlined, ReloadOutlined, SettingOutlined, UserOutlined } from "@ant-design/icons";
import { api, type LlmProfile } from "../api";
import { PageHeader } from "../components/PageHeader";
import { buildModelOptions, modelLabel } from "../models";
import { useT } from "../i18n/core";

interface SingleInfo {
  identifier: string;
  name: string;
  description: string;
  domain: string;
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
                  <Button
                    size="small"
                    icon={<SettingOutlined />}
                    onClick={() => {
                      setModelTarget(s);
                      setModelDraft(s.modelHint ?? "");
                    }}
                  >
                    {t("singles.settings")}
                  </Button>
                  <Popconfirm title={t("singles.deleteConfirm")} onConfirm={() => void remove(s.identifier)}>
                    <Button size="small" danger>
                      {t("common.delete")}
                    </Button>
                  </Popconfirm>
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

      {/* 默认模型设置（跟随全局 / 指定提供商与模型） */}
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
          <Form.Item name="description" label={t("singles.f.duty")} rules={[{ required: true }]}>
            <Input.TextArea rows={2} placeholder={t("singles.f.dutyPh")} />
          </Form.Item>
          <Form.Item name="prompt" label={t("singles.f.prompt")}>
            <Input.TextArea rows={4} placeholder={t("singles.f.promptPh")} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
