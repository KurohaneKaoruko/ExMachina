/** 通道网关：多平台多账号接入（webhook / telegram），每个账号可绑定不同智能体组 */
import React, { useCallback, useEffect, useState } from "react";
import { Collapse,
  Button, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Switch, Table, Tag, message,
} from "antd";
import { PlusOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type Channel } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";

export function ChannelsView(): React.ReactElement {
  const { groups } = useExm();
  const [channels, setChannels] = useState<Channel[]>([]);
  const [loading, setLoading] = useState(false);
  const [modal, setModal] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [form] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setChannels(await api.listChannels());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const openCreate = () => {
    setEditing(null);
    form.resetFields();
    form.setFieldsValue({ platform: "webhook" });
    setModal(true);
  };

  const openEdit = (c: Channel) => {
    setEditing(c);
    form.setFieldsValue({
      platform: c.type,
      account: c.account,
      group: c.group,
      secret: c.secret,
      replyWebhook: c.replyWebhook,
      token: c.token,
      allowedChats: (c.allowedChats ?? []).join(", "),
    });
    setModal(true);
  };

  const submit = async () => {
    const v = await form.validateFields();
    const body = {
      platform: v.platform,
      account: v.account || undefined,
      group: v.group || undefined,
      secret: v.secret || undefined,
      replyWebhook: v.replyWebhook || undefined,
      token: v.token || undefined,
      allowedChats: (v.allowedChats ?? "")
        .split(/[,,\s]+/)
        .map((s: string) => s.trim())
        .filter(Boolean),
    };
    try {
      if (editing) {
        await api.updateChannel(editing.id, body);
        message.success(`通道已更新：${editing.id}`);
      } else {
        await api.createChannel({ id: v.id, enabled: true, ...body });
        message.success(`通道已创建：${v.id}（5 秒内自动接管调度）`);
      }
      setModal(false);
      await load();
    } catch (e) {
      message.error(`保存失败：${String(e)}`);
    }
  };

  const toggle = async (c: Channel, enabled: boolean) => {
    await api.updateChannel(c.id, { enabled });
    await load();
  };

  const remove = async (id: string) => {
    try {
      await api.deleteChannel(id);
      message.success(`通道已删除：${id}`);
      await load();
    } catch (e) {
      message.error(`删除失败：${String(e)}`);
    }
  };

  const origin = typeof window !== "undefined" ? window.location.origin : "";
  const platformTag = (t: string) =>
    t === "telegram" ? (
      <Tag color="processing">Telegram</Tag>
    ) : t === "qq" ? (
      <Tag color="geekblue">QQ</Tag>
    ) : t === "wechat" ? (
      <Tag color="green">微信</Tag>
    ) : (
      <Tag color="cyan">Webhook</Tag>
    );

  return (
    <div className="pane-wrap">
      <PageHeader
        en="CHANNELS"
        title="通道"
        desc="把外部的消息平台接到某个智能体组：多平台可并存、同平台可多账号，每个账号绑定一个组，来消息即以该组上下文执行。"
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
              接入账号
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              刷新
            </Button>
          </>
        }
      />

      <Spin spinning={loading}>
        <Table<Channel>
          size="small"
          rowKey="id"
          pagination={false}
          dataSource={channels}
          locale={{ emptyText: <Empty description="无接入账号：接入 Telegram 机器人或 Webhook 消息源" className="pane-empty" /> }}
          columns={[
            {
              title: "账号",
              render: (_, c) => (
                <Space direction="vertical" size={0}>
                  <b>
                    {c.account ?? c.id} {c.account ? <span className="mono dim">{c.id}</span> : null}
                  </b>
                  <span className="mono dim">{c.createdAt}</span>
                </Space>
              ),
            },
            { title: "平台", render: (_, c) => platformTag(c.type) },
            {
              title: "绑定组",
              render: (_, c) =>
                c.group ? <Tag color="purple">{c.group}</Tag> : <Tag>当前组（跟随对话页切换）</Tag>,
            },
            {
              title: "凭据",
              render: (_, c) =>
                c.type === "telegram" ? (
                  c.token ? <Tag color="warning">token 已设置</Tag> : <Tag color="error">缺 token</Tag>
                ) : c.secret ? (
                  <Tag color="warning">密钥已设置</Tag>
                ) : (
                  <Tag>无密钥</Tag>
                ),
            },
            {
              title: "回调",
              render: (_, c) => <span className="mono dim">{c.replyWebhook ?? "—"}</span>,
            },
            {
              title: "启用",
              render: (_, c) => <Switch size="small" checked={c.enabled} onChange={(v) => void toggle(c, v)} />,
            },
            {
              title: "操作",
              width: 140,
              render: (_, c) => (
                <Space>
                  <Button size="small" onClick={() => openEdit(c)}>
                    编辑
                  </Button>
                  <Popconfirm title="确认删除该账号通道？" onConfirm={() => void remove(c.id)}>
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

      {channels.some((c) => c.type === "webhook") && (
        <div className="inbound-hint hud">
          <div className="rail-label">Webhook 入站（其他平台经此桥接）</div>
          <pre className="live-pre">{`POST ${origin}/api/channels/<通道ID>/inbound
Content-Type: application/json

{ "secret": "<密钥>", "text": "消息内容", "sessionKey": "用户标识" }`}</pre>
          <div className="dim">
            Telegram 为原生长轮询（填 BotFather token 即用）；Slack / 飞书 / 钉钉 / Discord 可用其
            Outgoing Webhook / 事件订阅功能把消息转发到上面的 inbound 地址，回复经 replyWebhook 或 WS 返回。
          </div>
        </div>
      )}

      <Modal
        open={modal}
        title={editing ? `编辑账号 ${editing.id}` : "接入账号"}
        onCancel={() => setModal(false)}
        onOk={() => void submit()}
        okText="保存"
      >
        <Form form={form} layout="vertical">
          <Form.Item name="platform" label="平台" rules={[{ required: true }]}>
            <Select
              disabled={editing !== null}
              options={[
                { value: "telegram", label: "Telegram（原生机器人，长轮询）" },
                { value: "qq", label: "QQ（经桥接器：消息转发到 inbound）" },
                { value: "wechat", label: "微信（经桥接器：消息转发到 inbound）" },
                { value: "webhook", label: "Webhook（通用 HTTP 入站）" },
              ]}
              onChange={(v) => form.setFieldsValue({ token: undefined, secret: undefined })}
            />
          </Form.Item>
          {!editing && (
            <Form.Item name="id" label="通道 ID" rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: "仅字母/数字/-/_" }]}>
              <Input placeholder="如 tg-main" />
            </Form.Item>
          )}
          <Form.Item noStyle shouldUpdate={(a, b) => a.platform !== b.platform}>
            {({ getFieldValue }) =>
              getFieldValue("platform") === "telegram" ? (
                <Form.Item
                  name="token"
                  label="Bot Token（@BotFather 签发）"
                  rules={editing ? [] : [{ required: true }]}
                  extra={editing ? "留空 = 沿用已设置 token" : undefined}
                >
                  <Input.Password placeholder="123456:ABC-DEF…" />
                </Form.Item>
              ) : (
                <Form.Item name="secret" label="入站密钥（可选）">
                  <Input.Password placeholder="留空则不校验" />
                </Form.Item>
              )
            }
          </Form.Item>
          <Form.Item name="group" label="绑定智能体组（该账号的消息在此组执行）">
            <Select
              allowClear
              placeholder="缺省 = 当前组（跟随对话页切换）"
              options={groups.map((g) => ({ value: g.id, label: `${g.name}（${g.id}）` }))}
            />
          </Form.Item>

          <Collapse
            ghost
            className="form-advanced"
            items={[
              {
                key: "adv",
                label: "高级设置（备注 / 会话白名单 / 出站回调）",
                children: (
                  <>
                    <Form.Item name="account" label="账号备注（同平台多账号时区分用途）">
                      <Input placeholder="如 主号 / 客服号" />
                    </Form.Item>
                    <Form.Item
                      name="allowedChats"
                      label="会话白名单（Telegram）"
                      extra="允许交互的 chat id，逗号分隔；留空 = 不限，可防陌生人滥用机器人"
                    >
                      <Input placeholder="如 123456789, 987654321" />
                    </Form.Item>
                    <Form.Item name="replyWebhook" label="出站回调 URL（webhook 平台）">
                      <Input placeholder="运行结束后 POST 结果" />
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
