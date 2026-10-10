/** 智能连结网络（实验性）：外部连结体管理——远程 EXMACHINA 网关 / 本机 CLI 智能体的接入与维护 */
import React, { useCallback, useEffect, useState } from "react";
import { Alert, Button, Card, Form, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message } from "antd";
import { BrandEmpty } from "../components/BrandEmpty";
import { PlusOutlined, SettingOutlined, UsergroupDeleteOutlined } from "@ant-design/icons";
import { api, type NexusLink } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useT } from "../i18n/core";

const KIND_COLORS: Record<string, string> = {
  exmachina: "cyan",
  opencode: "purple",
  codex: "geekblue",
  claude: "gold",
  custom: "default",
};

export function NexusView(): React.ReactElement {
  const t = useT();
  const [links, setLinks] = useState<NexusLink[]>([]);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState<{ link: NexusLink | null } | null>(null);
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setLinks((await api.nexusLinks()).links);
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
      const r = await api.saveNexusLink({
        id: v.id.trim(),
        name: v.name.trim(),
        kind: v.kind,
        endpoint: (v.endpoint ?? "").trim(),
        apiKey: (v.apiKey ?? "").trim(),
        command: (v.command ?? "").trim(),
        enabled: true,
      });
      message.success(t("nexus.saved", { name: r.link.name }));
      setModal(null);
      form.resetFields();
      await load();
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e).slice(0, 140) }));
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteNexusLink(id);
      message.success(t("nexus.deleted"));
      await load();
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e).slice(0, 140) }));
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="NEXUS"
        title={t("nav.nexus")}
        desc={t("nexus.desc")}
        actions={
          <Button type="primary" icon={<PlusOutlined />} onClick={() => { form.resetFields(); form.setFieldsValue({ kind: "exmachina" }); setModal({ link: null }); }}>
            {t("nexus.add")}
          </Button>
        }
      />
      <Alert
        type="warning"
        showIcon
        className="nexus-exp-alert app-banner"
        message={t("nexus.expTitle")}
        description={t("nexus.expDesc")}
        style={{ marginBottom: 12 }}
      />
      <div className="pane-body">
        <Spin spinning={loading}>
          {links.length === 0 ? (
            <BrandEmpty description={t("nexus.empty")} className="pane-empty" />
          ) : (
            <div className="nexus-links">
              {links.map((l) => (
                <Card key={l.id} size="small" className="hud nexus-link-card">
                  <div className="nexus-link-row">
                    <Tag color={l.enabled ? KIND_COLORS[l.kind] ?? "default" : "default"}>{t("nexus.kind." + l.kind)}</Tag>
                    <b>{l.name}</b>
                    <span className="mono dim">{l.id}</span>
                    {(l.endpoint || l.command) && (
                      <span className="dim mono nexus-endpoint">{l.endpoint || l.command}</span>
                    )}
                    <Space size={4} style={{ marginLeft: "auto" }}>
                      {!l.enabled && <Tag>disabled</Tag>}
                      <Button
                        size="small"
                        icon={<SettingOutlined />}
                        onClick={() => {
                          form.setFieldsValue({ ...l, apiKey: undefined });
                          setModal({ link: l });
                        }}
                      />
                      <Popconfirm title={t("nexus.delConfirm", { name: l.name })} onConfirm={() => void remove(l.id)}>
                        <Button size="small" danger icon={<UsergroupDeleteOutlined />} />
                      </Popconfirm>
                    </Space>
                  </div>
                </Card>
              ))}
            </div>
          )}
        </Spin>
      </div>

      <Modal
        open={modal !== null}
        title={modal?.link ? t("nexus.editTitle", { name: modal.link.name }) : t("nexus.add")}
        onCancel={() => setModal(null)}
        onOk={() => void submit()}
        okText={t("common.save")}
      >
        <Form form={form} layout="vertical" initialValues={{ kind: "exmachina" }}>
          <Form.Item name="id" label={t("nexus.f.id")} rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("groups.idPattern") }]}>
            <Input placeholder={t("nexus.f.idPh")} disabled={modal?.link != null} />
          </Form.Item>
          <Form.Item name="name" label={t("nexus.f.name")} rules={[{ required: true }]}>
            <Input placeholder={t("nexus.f.namePh")} />
          </Form.Item>
          <Form.Item name="kind" label={t("nexus.f.kind")} rules={[{ required: true }]}>
            <Select
              options={[
                { value: "exmachina", label: t("nexus.kind.exmachina") },
                { value: "opencode", label: t("nexus.kind.opencode") },
                { value: "codex", label: t("nexus.kind.codex") },
                { value: "claude", label: t("nexus.kind.claude") },
                { value: "custom", label: t("nexus.kind.custom") },
              ]}
            />
          </Form.Item>
          <Form.Item noStyle shouldUpdate={(a, b) => a.kind !== b.kind}>
            {({ getFieldValue }) =>
              getFieldValue("kind") === "exmachina" ? (
                <>
                  <Form.Item name="endpoint" label={t("nexus.f.endpoint")} rules={[{ required: true, whitespace: true, message: t("nexus.f.endpointReq") }]}>
                    <Input placeholder="http://192.168.2.10:4173" />
                  </Form.Item>
                  <Form.Item name="apiKey" label={t("nexus.f.apiKey")} extra={t("nexus.f.apiKeyExtra")}>
                    <Input.Password placeholder="******" />
                  </Form.Item>
                </>
              ) : getFieldValue("kind") === "custom" ? (
                <Form.Item name="command" label={t("nexus.f.command")} extra={t("nexus.f.commandExtra")} rules={[{ required: true, whitespace: true, message: t("nexus.f.commandReq") }]}>
                  <Input placeholder="my-agent --task" />
                </Form.Item>
              ) : null
            }
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
