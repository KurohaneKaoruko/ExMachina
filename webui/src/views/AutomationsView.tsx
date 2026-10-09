/** 自动化管理：定时任务（cron / 一次性）的创建、启停、手动执行与运行记录 */
import React, { useCallback, useEffect, useState } from "react";
import { Segmented, Collapse, Button, Drawer, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Switch, Table, Tag, message } from "antd";
import { CaretRightOutlined, PlusOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type Channel, type CronJob, type CronRun } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";
import { useT } from "../i18n/core";

/** 常用 cron 模板（4.4）：一键填入表达式 */
const CRON_TEMPLATES: { expr: string; labelKey: "cron.tpl.daily" | "cron.tpl.weekday" | "cron.tpl.hourly" | "cron.tpl.weekly" | "cron.tpl.monthly" }[] = [
  { expr: "0 9 * * *", labelKey: "cron.tpl.daily" },
  { expr: "0 9 * * 1-5", labelKey: "cron.tpl.weekday" },
  { expr: "0 * * * *", labelKey: "cron.tpl.hourly" },
  { expr: "0 9 * * 1", labelKey: "cron.tpl.weekly" },
  { expr: "0 9 1 * *", labelKey: "cron.tpl.monthly" },
];

export function AutomationsView(): React.ReactElement {
  const t = useT();
  const { groups, activeGroup } = useExm();
  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [runsFor, setRunsFor] = useState<CronJob | null>(null);
  const [runs, setRuns] = useState<CronRun[]>([]);
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [jobs, chs] = await Promise.all([api.listCron(), api.listChannels()]);
      setJobs(jobs);
      setChannels(chs);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const submit = async () => {
    const v = await form.validateFields();
    const oneOff = v.mode === "at";
    try {
      await api.createCron({
        name: v.name,
        prompt: v.prompt,
        cron: oneOff ? undefined : (v.cron as string | undefined),
        at: oneOff ? (v.at as string | undefined) : undefined,
        group: v.group || undefined,
        sessionTitle: v.sessionTitle || undefined,
        notifyChannels: ((v.notifyChannels ?? []) as string[]).map((s) => s.trim()).filter(Boolean),
      });
      message.success(t("cron.created"));
      setModal(false);
      form.resetFields();
      await load();
    } catch (e) {
      message.error(t("cron.createFailed", { err: String(e) }));
    }
  };

  const toggle = async (job: CronJob, enabled: boolean) => {
    await api.updateCron(job.id, { enabled });
    await load();
  };

  const runNow = async (job: CronJob) => {
    try {
      const run = await api.runCron(job.id);
      message.success(t("cron.ran", { status: run.status, id: run.sessionId.slice(0, 8) }));
    } catch (e) {
      message.error(t("cron.runFailed", { err: String(e) }));
    }
  };

  const openRuns = async (job: CronJob) => {
    setRunsFor(job);
    setRuns(await api.cronRuns(job.id, 30));
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="CRON"
        title={t("nav.cron")}
        desc={t("cron.desc")}
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setModal(true)}>
              {t("cron.new")}
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              {t("common.refresh")}
            </Button>
          </>
        }
      />
      <Spin spinning={loading}>
        <Table<CronJob>
          size="small"
          rowKey="id"
          pagination={false}
          dataSource={jobs}
          locale={{ emptyText: <Empty description={t("cron.empty")} className="pane-empty" /> }}
          columns={[
            {
              title: t("cron.colJob"),
              render: (_, j) => (
                <Space direction="vertical" size={0}>
                  <b>{j.name}</b>
                  <span className="mono dim">{j.id}</span>
                </Space>
              ),
            },
            {
              title: t("cron.colSchedule"),
              render: (_, j) => (
                <Space direction="vertical" size={2}>
                  {j.cron ? <Tag color="cyan">cron[{j.cron}]</Tag> : <Tag color="purple">{t("cron.oneOff", { at: j.at ?? "" })}</Tag>}
                  {j.enabled && j.nextRunAt && (
                    <span className="dim mono">{t("cron.nextRun", { at: new Date(j.nextRunAt).toLocaleString() })}</span>
                  )}
                </Space>
              ),
            },
            {
              title: t("cron.colGroup"),
              render: (_, j) => (
                <Space direction="vertical" size={0}>
                  <span className="mono">{j.group ?? t("cron.currentGroup")}</span>
                  {j.notifyChannels && j.notifyChannels.length > 0 && (
                    <span className="mono dim">
                      {t("cron.f.notifyChannels")}: {j.notifyChannels.join(", ")}
                    </span>
                  )}
                </Space>
              ),
            },
            {
              title: t("cron.colEnabled"),
              render: (_, j) => <Switch size="small" checked={j.enabled} onChange={(v) => void toggle(j, v)} />,
            },
            {
              title: t("cron.colLastRun"),
              render: (_, j) => (
                <span className="mono dim">
                  {j.lastRunAt ?? t("cron.notRun")}
                  {j.lastStatus ? ` · ${j.lastStatus}` : ""}
                </span>
              ),
            },
            {
              title: t("cron.colOps"),
              render: (_, j) => (
                <Space>
                  <Button size="small" icon={<CaretRightOutlined />} onClick={() => void runNow(j)}>
                    {t("cron.run")}
                  </Button>
                  <Button size="small" onClick={() => void openRuns(j)}>
                    {t("cron.runs")}
                  </Button>
                  <Popconfirm title={t("cron.deleteConfirm")} onConfirm={() => void api.deleteCron(j.id).then(load)}>
                    <Button size="small" danger>
                      {t("common.delete")}
                    </Button>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </Spin>

      <Drawer title={t("cron.runsTitle", { name: runsFor?.name ?? "" })} open={runsFor !== null} onClose={() => setRunsFor(null)} width={520}>
        {runs.length === 0 && <Empty description={t("cron.noRuns")} />}
        {runs.map((r) => (
          <div key={r.id} className="run-row">
            <div>
              <Tag color={r.status === "done" ? "success" : "error"}>{r.status}</Tag>
              <span className="mono dim">{r.startedAt}</span>
            </div>
            <div className="dim">
              {t("cron.sessionLabel")} <span className="mono">{r.sessionId.slice(0, 8)}</span> · {r.summary}
            </div>
          </div>
        ))}
      </Drawer>

      <Modal open={modal} title={t("cron.modalNew")} onCancel={() => setModal(false)} onOk={() => void submit()} okText={t("cron.create")}>
        <Form form={form} layout="vertical" initialValues={{ mode: "cron" }}>
          <Form.Item name="name" label={t("cron.f.name")} rules={[{ required: true }]}>
            <Input placeholder={t("cron.f.namePh")} />
          </Form.Item>
          <Form.Item name="prompt" label={t("cron.f.prompt")} rules={[{ required: true }]}>
            <Input.TextArea rows={3} placeholder={t("cron.f.promptPh")} />
          </Form.Item>
          <Form.Item name="mode" label={t("cron.f.mode")}>
            <Segmented
              options={[
                { value: "cron", label: t("cron.mode.cron") },
                { value: "at", label: t("cron.mode.at") },
              ]}
            />
          </Form.Item>
          <Form.Item noStyle shouldUpdate={(a, b) => a.mode !== b.mode}>
            {({ getFieldValue }) =>
              getFieldValue("mode") === "at" ? (
                <Form.Item name="at" label={t("cron.f.at")} rules={[{ required: true }]}>
                  <Input placeholder="2026-09-13T09:00:00Z" />
                </Form.Item>
              ) : (
                <>
                  <Form.Item label={t("cron.templates")} style={{ marginBottom: 6 }}>
                    <Space size={4} wrap>
                      {CRON_TEMPLATES.map((tpl) => (
                        <Button
                          key={tpl.expr}
                          size="small"
                          onClick={() => form.setFieldsValue({ cron: tpl.expr })}
                          title={tpl.expr}
                        >
                          {t(tpl.labelKey)}
                        </Button>
                      ))}
                    </Space>
                  </Form.Item>
                  <Form.Item
                    name="cron"
                    label={t("cron.f.cron")}
                    rules={[{ required: true }, { pattern: /^(\S+\s+){4}\S+$/, message: t("cron.cronPattern") }]}
                  >
                    <Input placeholder="0 9 * * *" />
                  </Form.Item>
                </>
              )
            }
          </Form.Item>
          <Form.Item name="group" label={t("cron.f.group")} initialValue={activeGroup}>
            <Select
              allowClear
              placeholder={t("cron.f.groupPh")}
              options={groups.map((g) => ({ value: g.id, label: `${g.name}（${g.id}）` }))}
            />
          </Form.Item>

          <Collapse
            ghost
            className="form-advanced"
            items={[
              {
                key: "adv",
                label: t("cron.adv"),
                children: (
                  <>
                    <Form.Item
                      name="sessionTitle"
                      label={t("cron.f.sessionTitle")}
                      extra={t("cron.f.sessionTitleExtra")}
                    >
                      <Input placeholder={t("cron.f.sessionTitlePh")} />
                    </Form.Item>
                    <Form.Item
                      name="notifyChannels"
                      label={t("cron.f.notifyChannels")}
                      extra={t("cron.f.notifyChannelsExtra")}
                    >
                      <Select
                        mode="multiple"
                        allowClear
                        placeholder={t("cron.f.notifyChannelsPh")}
                        options={channels.map((c) => ({
                          value: c.id,
                          label: c.account ? `${c.account}（${c.id}）` : c.id,
                        }))}
                      />
                    </Form.Item>
                  </>
                ),
              },
            ]}
          />
        </Form>
      </Modal>
    </div>
  );
}
