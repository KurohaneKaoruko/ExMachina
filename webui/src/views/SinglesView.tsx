/** 智能体页（单体）：档案管理 —— 创建/删除/默认模型设置；交互切换在「对话」页左栏进行 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Tag, message } from "antd";
import { PlusOutlined, ReloadOutlined, SettingOutlined, UserOutlined } from "@ant-design/icons";
import { api, type LlmProfile } from "../api";
import { PageHeader } from "../components/PageHeader";
import { buildModelOptions, modelLabel } from "../models";

interface SingleInfo {
  identifier: string;
  name: string;
  description: string;
  domain: string;
  modelHint?: string | null;
}

export function SinglesView(): React.ReactElement {
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
      message.success(`智能体已创建：${v.identifier}`);
      setModal(false);
      form.resetFields();
      await load();
    } catch (e) {
      message.error(`创建失败：${String(e)}`);
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteSingle(id);
      message.success(`已删除：${id}`);
      await load();
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  const saveModel = async () => {
    if (!modelTarget) return;
    try {
      await api.setAgentModel(modelTarget.identifier, modelDraft);
      message.success(`默认模型已更新：${modelTarget.name}`);
      setModelTarget(null);
      await load();
    } catch (e) {
      message.error(`设置失败：${String(e)}`);
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        title="独立智能体"
        desc="独立于任何组、直接对接你的单体智能体（预置 Machina 以「本机」自称）。与谁对话在「对话」页左栏切换，这里只管档案与默认模型。"
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setModal(true)}>
              新建智能体
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              刷新
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
                  {activeSingle === s.identifier && <Tag color="processing">对话目标</Tag>}
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
                    设置
                  </Button>
                  <Popconfirm title="确认删除该智能体？" onConfirm={() => void remove(s.identifier)}>
                    <Button size="small" danger>
                      删除
                    </Button>
                  </Popconfirm>
                </Space>
              }
            >
              <div className="dim">{s.description}</div>
              <div className="skill-line">域：{s.domain || "单体"}</div>
              <div className="skill-line">
                默认模型：<Tag color="geekblue">{modelLabel(s.modelHint, profiles)}</Tag>
              </div>
            </Card>
          ))}
        </div>
        {!loading && singles.length === 0 && (
          <Empty description="尚无智能体：创建一个直接对话的独立智能体（默认 Machina 已内置）" className="graph-empty" />
        )}
      </Spin>

      {/* 默认模型设置（跟随全局 / 指定提供商与模型） */}
      <Modal
        open={modelTarget !== null}
        title={`默认模型 · ${modelTarget?.name ?? ""}`}
        onCancel={() => setModelTarget(null)}
        onOk={() => void saveModel()}
        okText="保存"
        width={480}
      >
        <div className="pane-hint" style={{ marginBottom: 12 }}>
          该智能体的对话与任务将使用所选模型；选「跟随全局默认」时使用「提供商」页中设为全局默认的提供商。
          提供商本身请到「提供商」页维护。
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
        title="新建智能体"
        onCancel={() => setModal(false)}
        onOk={() => void submit()}
        okText="创建"
      >
        <Form form={form} layout="vertical">
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input placeholder="如：写作助理" />
          </Form.Item>
          <Form.Item
            name="identifier"
            label="标识（identifier）"
            rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: "仅字母/数字/-/_" }]}
          >
            <Input placeholder="如 writing-buddy" />
          </Form.Item>
          <Form.Item name="description" label="职责" rules={[{ required: true }]}>
            <Input.TextArea rows={2} placeholder="该智能体的职责一句话描述" />
          </Form.Item>
          <Form.Item name="prompt" label="提示词（可选，留空生成模板）">
            <Input.TextArea rows={4} placeholder="定义它的身份、说话方式与工作方式" />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
