/** 技能包管理：派发时按触发词自动携带的指令包（数据化技能，热装载） */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Empty, Form, Input, Modal, Popconfirm, Space, Spin, Tag, message } from "antd";
import { PlusOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type SkillDef } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useT } from "../i18n/core";

export function SkillsView(): React.ReactElement {
  const t = useT();
  const [skills, setSkills] = useState<SkillDef[]>([]);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setSkills(await api.skills());
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
      await api.createSkill({
        id: v.id,
        name: v.name,
        description: v.description ?? "",
        triggers: (v.triggers ?? "").split(/[,，]/).map((s: string) => s.trim()).filter(Boolean),
        instructions: v.instructions,
        agents: (v.agents ?? "").split(/[,，]/).map((s: string) => s.trim()).filter(Boolean),
      });
      message.success(t("skills.created", { id: v.id }));
      setModal(false);
      form.resetFields();
      await load();
    } catch (e) {
      message.error(t("skills.createFailed", { err: String(e) }));
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteSkill(id);
      message.success(t("skills.deleted", { id }));
      await load();
    } catch (e) {
      message.error(t("skills.deleteFailed", { err: String(e) }));
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="SKILLS"
        title={t("nav.skills")}
        desc={t("skills.desc")}
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setModal(true)}>
              {t("skills.new")}
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              {t("common.refresh")}
            </Button>
          </>
        }
      />
      <Spin spinning={loading}>
        <div className="skill-grid">
          {skills.map((s) => (
            <Card
              key={s.id}
              size="small"
              className="hud skill-card"
              title={
                <Space>
                  <span>{s.name}</span>
                  <span className="mono dim">{s.id}</span>
                </Space>
              }
              extra={
                <Popconfirm title={t("skills.deleteConfirm")} onConfirm={() => void remove(s.id)}>
                  <Button size="small" danger>
                    {t("common.delete")}
                  </Button>
                </Popconfirm>
              }
            >
              {s.description && <div className="dim">{s.description}</div>}
              <div className="skill-line">
                {t("skills.triggerLabel")}
                {s.triggers.length ? (
                  s.triggers.map((tl) => (
                    <Tag key={tl} color="cyan">
                      {tl}
                    </Tag>
                  ))
                ) : (
                  <span className="dim">—</span>
                )}
              </div>
              <div className="skill-line">
                {t("skills.applyLabel")}{s.agents.length ? s.agents.map((a) => <Tag key={a}>{a}</Tag>) : <Tag color="purple">{t("skills.allAgents")}</Tag>}
              </div>
              <div className="skill-instructions">{s.instructions}</div>
            </Card>
          ))}
        </div>
        {!loading && skills.length === 0 && (
          <Empty description={t("skills.empty")} className="graph-empty" />
        )}
      </Spin>

      <Modal open={modal} title={t("skills.new")} onCancel={() => setModal(false)} onOk={() => void submit()} okText={t("skills.create")}>
        <Form form={form} layout="vertical">
          <Form.Item name="id" label="ID" rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("skills.idPattern") }]}>
            <Input placeholder={t("skills.f.idPh")} />
          </Form.Item>
          <Form.Item name="name" label={t("skills.f.name")} rules={[{ required: true }]}>
            <Input placeholder={t("skills.f.namePh")} />
          </Form.Item>
          <Form.Item name="description" label={t("skills.f.desc")}>
            <Input />
          </Form.Item>
          <Form.Item name="triggers" label={t("skills.f.triggers")}>
            <Input placeholder="git push, git reset, git clean" />
          </Form.Item>
          <Form.Item name="agents" label={t("skills.f.agents")}>
            <Input placeholder="coding-agent, ops-agent" />
          </Form.Item>
          <Form.Item name="instructions" label={t("skills.f.instructions")} rules={[{ required: true }]}>
            <Input.TextArea rows={4} placeholder={t("skills.f.instructionsPh")} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
