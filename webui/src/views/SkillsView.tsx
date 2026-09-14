/** 技能包管理：派发时按触发词自动携带的指令包（数据化技能，热装载） */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Empty, Form, Input, Modal, Popconfirm, Space, Spin, Tag, message } from "antd";
import { PlusOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type SkillDef } from "../api";
import { PageHeader } from "../components/PageHeader";

export function SkillsView(): React.ReactElement {
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
      message.success(`技能已创建：${v.id}（热装载，下一次派发即生效）`);
      setModal(false);
      form.resetFields();
      await load();
    } catch (e) {
      message.error(`创建失败：${String(e)}`);
    }
  };

  const remove = async (id: string) => {
    try {
      await api.deleteSkill(id);
      message.success(`技能已删除：${id}`);
      await load();
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="SKILLS"
        title="技能"
        desc="技能由全体智能体共用，新增即生效：任务目标命中触发词时，对应指令自动注入被派发个体的上下文。"
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setModal(true)}>
              新建技能
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              刷新
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
                <Popconfirm title="确认删除该技能？" onConfirm={() => void remove(s.id)}>
                  <Button size="small" danger>
                    删除
                  </Button>
                </Popconfirm>
              }
            >
              {s.description && <div className="dim">{s.description}</div>}
              <div className="skill-line">
                触发：
                {s.triggers.length ? (
                  s.triggers.map((t) => (
                    <Tag key={t} color="cyan">
                      {t}
                    </Tag>
                  ))
                ) : (
                  <span className="dim">—</span>
                )}
              </div>
              <div className="skill-line">
                适用：{s.agents.length ? s.agents.map((a) => <Tag key={a}>{a}</Tag>) : <Tag color="purple">全体</Tag>}
              </div>
              <div className="skill-instructions">{s.instructions}</div>
            </Card>
          ))}
        </div>
        {!loading && skills.length === 0 && (
          <Empty description="当前组无技能包" className="graph-empty" />
        )}
      </Spin>

      <Modal open={modal} title="新建技能" onCancel={() => setModal(false)} onOk={() => void submit()} okText="创建">
        <Form form={form} layout="vertical">
          <Form.Item name="id" label="ID" rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: "仅字母/数字/-/_" }]}>
            <Input placeholder="如：git-safety" />
          </Form.Item>
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input placeholder="如：Git 安全作业" />
          </Form.Item>
          <Form.Item name="description" label="描述（可选）">
            <Input />
          </Form.Item>
          <Form.Item name="triggers" label="触发词（逗号分隔）">
            <Input placeholder="git push, git reset, git clean" />
          </Form.Item>
          <Form.Item name="agents" label="适用个体（逗号分隔，留空 = 全体）">
            <Input placeholder="coding-agent, ops-agent" />
          </Form.Item>
          <Form.Item name="instructions" label="指令内容（派发时注入）" rules={[{ required: true }]}>
            <Input.TextArea rows={4} placeholder="行为约束与作业知识，逐条列出" />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
