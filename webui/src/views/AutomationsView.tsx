/** 自动化管理：定时任务（cron / 一次性）的创建、启停、手动执行与运行记录 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Drawer, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Switch, Table, Tag, message } from "antd";
import { CaretRightOutlined, PlusOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type CronJob, type CronRun } from "../api";
import { useExm } from "../store";

export function AutomationsView(): React.ReactElement {
  const { groups, activeGroup } = useExm();
  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [runsFor, setRunsFor] = useState<CronJob | null>(null);
  const [runs, setRuns] = useState<CronRun[]>([]);
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setJobs(await api.listCron());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const submit = async () => {
    const v = await form.validateFields();
    if (!v.cron && !v.at) {
      message.error("需要 cron 表达式或一次性时间");
      return;
    }
    try {
      await api.createCron({
        name: v.name,
        prompt: v.prompt,
        cron: v.cron || undefined,
        at: v.at || undefined,
        group: v.group || undefined,
        sessionTitle: v.sessionTitle || undefined,
      });
      message.success("定时任务已创建（exm serve 常驻时自动调度）");
      setModal(false);
      form.resetFields();
      await load();
    } catch (e) {
      message.error(`创建失败：${String(e)}`);
    }
  };

  const toggle = async (job: CronJob, enabled: boolean) => {
    await api.updateCron(job.id, { enabled });
    await load();
  };

  const runNow = async (job: CronJob) => {
    try {
      const run = await api.runCron(job.id);
      message.success(`已执行：${run.status}（会话 ${run.sessionId.slice(0, 8)}…）`);
    } catch (e) {
      message.error(`执行失败：${String(e)}`);
    }
  };

  const openRuns = async (job: CronJob) => {
    setRunsFor(job);
    setRuns(await api.cronRuns(job.id, 30));
  };

  return (
    <div className="pane-wrap">
      <div className="pane-toolbar">
        <Space>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setModal(true)}>
            新建任务
          </Button>
          <Button icon={<ReloadOutlined />} onClick={() => void load()}>
            刷新
          </Button>
          <span className="pane-hint">网关（exm serve）常驻时每 20 秒扫描到期任务；一次性任务触发后自动停用。</span>
        </Space>
      </div>
      <Spin spinning={loading}>
        <Table<CronJob>
          size="small"
          rowKey="id"
          pagination={false}
          dataSource={jobs}
          locale={{ emptyText: <Empty description="无定时任务" className="pane-empty" /> }}
          columns={[
            {
              title: "任务",
              render: (_, j) => (
                <Space direction="vertical" size={0}>
                  <b>{j.name}</b>
                  <span className="mono dim">{j.id}</span>
                </Space>
              ),
            },
            {
              title: "调度",
              render: (_, j) =>
                j.cron ? <Tag color="cyan">cron[{j.cron}]</Tag> : <Tag color="purple">一次性 {j.at}</Tag>,
            },
            { title: "组", render: (_, j) => <span className="mono">{j.group ?? "(激活组)"}</span> },
            {
              title: "启用",
              render: (_, j) => <Switch size="small" checked={j.enabled} onChange={(v) => void toggle(j, v)} />,
            },
            {
              title: "上次运行",
              render: (_, j) => (
                <span className="mono dim">
                  {j.lastRunAt ?? "未运行"}
                  {j.lastStatus ? ` · ${j.lastStatus}` : ""}
                </span>
              ),
            },
            {
              title: "操作",
              render: (_, j) => (
                <Space>
                  <Button size="small" icon={<CaretRightOutlined />} onClick={() => void runNow(j)}>
                    执行
                  </Button>
                  <Button size="small" onClick={() => void openRuns(j)}>
                    记录
                  </Button>
                  <Popconfirm title="确认删除该任务？" onConfirm={() => void api.deleteCron(j.id).then(load)}>
                    <Button size="small" danger>
                      删除
                    </Button>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </Spin>

      <Drawer title={`运行记录 · ${runsFor?.name ?? ""}`} open={runsFor !== null} onClose={() => setRunsFor(null)} width={520}>
        {runs.length === 0 && <Empty description="无运行记录" />}
        {runs.map((r) => (
          <div key={r.id} className="run-row">
            <div>
              <Tag color={r.status === "done" ? "success" : "error"}>{r.status}</Tag>
              <span className="mono dim">{r.startedAt}</span>
            </div>
            <div className="dim">
              会话 <span className="mono">{r.sessionId.slice(0, 8)}</span> · {r.summary}
            </div>
          </div>
        ))}
      </Drawer>

      <Modal open={modal} title="新建定时任务" onCancel={() => setModal(false)} onOk={() => void submit()} okText="创建">
        <Form form={form} layout="vertical">
          <Form.Item name="name" label="任务名" rules={[{ required: true }]}>
            <Input placeholder="如：每日巡检" />
          </Form.Item>
          <Form.Item name="prompt" label="提示词（到期注入指挥体）" rules={[{ required: true }]}>
            <Input.TextArea rows={3} placeholder="巡检任务账与风险账并给出摘要" />
          </Form.Item>
          <Space style={{ display: "flex" }} align="start">
            <Form.Item
              name="cron"
              label="Cron（五段：分 时 日 月 周）"
              rules={[{ pattern: /^(\S+\s+){4}\S+$/, message: "须为五段表达式" }]}
            >
              <Input placeholder="0 9 * * *" />
            </Form.Item>
            <Form.Item name="at" label="或一次性时间（ISO8601）">
              <Input placeholder="2026-09-13T09:00:00Z" />
            </Form.Item>
          </Space>
          <Form.Item name="group" label="执行组（缺省 = 运行时激活组）" initialValue={activeGroup}>
            <Select
              allowClear
              options={groups.map((g) => ({ value: g.id, label: `${g.name}（${g.id}）` }))}
            />
          </Form.Item>
          <Form.Item name="sessionTitle" label="会话标题（复用同名会话，可选）">
            <Input placeholder="留空 = job-<id>" />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
