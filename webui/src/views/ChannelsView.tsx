/** 通道网关：多平台多账号接入（QQ 官方机器人 / NapCat / Telegram / Discord / Slack / Matrix / webhook 桥接），每个账号可绑定不同智能体组 */
import React, { useCallback, useEffect, useState } from "react";
import {
  Button, Collapse, Empty, Form, Input, Modal, Popconfirm, Select, Space, Spin, Switch, Table, Tag, Tooltip, message,
} from "antd";
import { PlusOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type Channel, type ChannelStatus } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";
import { useT, type TKey } from "../i18n/core";

/** 平台目录：表单字段、凭证判定、回复形态全部由目录驱动 —— 接新平台加一条即可 */
interface ChanField {
  key: string;
  labelKey: TKey;
  secret?: boolean;
  required?: boolean;
  ph?: string;
  helpKey?: TKey;
  /** 存顶层字段（telegram.token / 桥接.secret）；缺省存 config.<key> */
  top?: "token" | "secret";
  /** 布尔开关（存 "true"/空） */
  toggle?: boolean;
}
interface ChanPlat {
  key: string;
  color: string;
  /** 表格里的平台标牌（品牌名，各语言一致） */
  tag?: string;
  tagKey?: TKey;
  labelKey: TKey;
  helpKey: TKey;
  fields: ChanField[];
  /** 回复形态：平台原生回信（适配器直接发回原会话）；缺省 = webhook 回调 */
  replyBuiltin?: boolean;
}

const PLATFORMS: ChanPlat[] = [
  {
    key: "qqbot",
    color: "geekblue",
    tag: "QQ Bot",
    labelKey: "channels.plat.qqbot",
    helpKey: "channels.platHelp.qqbot",
    replyBuiltin: true,
    fields: [
      { key: "appId", labelKey: "channels.f.appId", required: true, ph: "AppID", helpKey: "channels.f.appIdHelp" },
      { key: "appSecret", labelKey: "channels.f.appSecret", secret: true, required: true, ph: "AppSecret", helpKey: "channels.f.appSecretHelp" },
      { key: "sandbox", labelKey: "channels.f.sandbox", toggle: true, helpKey: "channels.f.sandboxHelp" },
    ],
  },
  {
    key: "napcat",
    color: "blue",
    tag: "NapCat",
    labelKey: "channels.plat.napcat",
    helpKey: "channels.platHelp.napcat",
    replyBuiltin: true,
    fields: [
      { key: "url", labelKey: "channels.f.napUrl", required: true, ph: "ws://127.0.0.1:3001", helpKey: "channels.f.napUrlHelp" },
      { key: "token", labelKey: "channels.f.napToken", secret: true, helpKey: "channels.f.napTokenHelp" },
    ],
  },
  {
    key: "telegram",
    color: "processing",
    tag: "Telegram",
    labelKey: "channels.plat.telegram",
    helpKey: "channels.platHelp.telegram",
    replyBuiltin: true,
    fields: [{ top: "token", key: "token", labelKey: "channels.f.token", secret: true, required: true, ph: "123456:ABC-DEF…" }],
  },
  {
    key: "discord",
    color: "purple",
    tag: "Discord",
    labelKey: "channels.plat.discord",
    helpKey: "channels.platHelp.discord",
    replyBuiltin: true,
    fields: [
      { top: "token", key: "token", labelKey: "channels.f.discordToken", secret: true, required: true, ph: "MTIz…（Bot Token）" },
    ],
  },
  {
    key: "slack",
    color: "magenta",
    tag: "Slack",
    labelKey: "channels.plat.slack",
    helpKey: "channels.platHelp.slack",
    replyBuiltin: true,
    fields: [
      { top: "token", key: "token", labelKey: "channels.f.botToken", secret: true, required: true, ph: "xoxb-…" },
      { key: "appToken", labelKey: "channels.f.appToken", secret: true, required: true, ph: "xapp-…", helpKey: "channels.f.appTokenHelp" },
    ],
  },
  {
    key: "matrix",
    color: "gold",
    tag: "Matrix",
    labelKey: "channels.plat.matrix",
    helpKey: "channels.platHelp.matrix",
    replyBuiltin: true,
    fields: [
      { key: "homeserver", labelKey: "channels.f.homeserver", required: true, ph: "https://matrix.org", helpKey: "channels.f.homeserverHelp" },
      { top: "token", key: "token", labelKey: "channels.f.accessToken", secret: true, required: true },
    ],
  },
  {
    key: "webhook",
    color: "cyan",
    tag: "Webhook",
    labelKey: "channels.plat.webhook",
    helpKey: "channels.platHelp.webhook",
    fields: [{ top: "secret", key: "secret", labelKey: "channels.f.secret", secret: true, ph: "channels.f.secretPh" }],
  },
  {
    key: "qq",
    color: "geekblue",
    tagKey: "channels.tag.qq",
    labelKey: "channels.plat.qq",
    helpKey: "channels.platHelp.qq",
    fields: [{ top: "secret", key: "secret", labelKey: "channels.f.secret", secret: true, ph: "channels.f.secretPh" }],
  },
  {
    key: "wechat",
    color: "green",
    tagKey: "channels.tag.wechat",
    labelKey: "channels.plat.wechat",
    helpKey: "channels.platHelp.wechat",
    fields: [{ top: "secret", key: "secret", labelKey: "channels.f.secret", secret: true, ph: "channels.f.secretPh" }],
  },
];

/** webhook 桥接语义的平台：入站靠外部桥 POST，出站靠回调 */
const BRIDGE_TYPES = ["webhook", "qq", "wechat"];

export function ChannelsView(): React.ReactElement {
  const t = useT();
  const { groups } = useExm();
  const [channels, setChannels] = useState<Channel[]>([]);
  const [status, setStatus] = useState<Record<string, ChannelStatus>>({});
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

  // 运行状态轮询：适配器对账周期 5s，这里同步 5s 拉一次总览
  useEffect(() => {
    let alive = true;
    const pull = () =>
      api
        .channelStatus()
        .then((s) => {
          if (alive) setStatus(s);
        })
        .catch(() => {});
    pull();
    const timer = setInterval(pull, 5000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  const openCreate = () => {
    setEditing(null);
    form.resetFields();
    form.setFieldsValue({ platform: "qqbot" });
    setModal(true);
  };

  const openEdit = (c: Channel) => {
    setEditing(c);
    const plat = PLATFORMS.find((p) => p.key === c.type);
    const cfg: Record<string, unknown> = { ...(c.config ?? {}) };
    plat?.fields
      .filter((f) => f.toggle)
      .forEach((f) => {
        cfg[f.key] = c.config?.[f.key] === "true";
      });
    form.setFieldsValue({
      platform: c.type,
      account: c.account,
      group: c.group,
      secret: c.secret,
      replyWebhook: c.replyWebhook,
      token: c.token,
      allowedChats: (c.allowedChats ?? []).join(", "),
      config: cfg,
    });
    setModal(true);
  };

  const submit = async () => {
    const v = await form.validateFields();
    const plat = PLATFORMS.find((p) => p.key === v.platform) ?? PLATFORMS[0];
    const config: Record<string, string> = {};
    for (const f of plat.fields) {
      if (f.top) continue;
      const raw = v.config?.[f.key];
      config[f.key] = f.toggle ? (raw ? "true" : "") : String(raw ?? "").trim();
    }
    const body = {
      platform: v.platform,
      account: v.account || undefined,
      group: v.group || undefined,
      secret: v.secret || undefined,
      replyWebhook: v.replyWebhook || undefined,
      token: v.token || undefined,
      config,
      allowedChats: (v.allowedChats ?? "")
        .split(/[,,\s]+/)
        .map((s: string) => s.trim())
        .filter(Boolean),
    };
    try {
      if (editing) {
        await api.updateChannel(editing.id, body);
        message.success(t("channels.updated", { id: editing.id }));
      } else {
        await api.createChannel({ id: v.id, enabled: true, ...body });
        message.success(t("channels.created", { id: v.id }));
      }
      setModal(false);
      await load();
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  const toggle = async (c: Channel, enabled: boolean) => {
    await api.updateChannel(c.id, { enabled });
    await load();
  };

  const remove = async (id: string) => {
    try {
      await api.deleteChannel(id);
      message.success(t("channels.del", { id }));
      await load();
    } catch (e) {
      message.error(t("channels.deleteFailed", { err: String(e) }));
    }
  };

  const origin = typeof window !== "undefined" ? window.location.origin : "";

  const platformTag = (p: string) => {
    const plat = PLATFORMS.find((x) => x.key === p);
    if (!plat) return <Tag>{p}</Tag>;
    return <Tag color={plat.color}>{plat.tag ?? (plat.tagKey ? t(plat.tagKey) : p)}</Tag>;
  };

  const fieldSet = (c: Channel, f: ChanField) =>
    f.top === "token" ? !!c.token : f.top === "secret" ? !!c.secret : !!c.config?.[f.key];

  const credTag = (c: Channel) => {
    const plat = PLATFORMS.find((x) => x.key === c.type);
    if (!plat) return <Tag>{c.type}</Tag>;
    const missing = plat.fields.filter((f) => f.required && !fieldSet(c, f));
    if (missing.length > 0) {
      return (
        <Tag color="error">
          {t("channels.credMissing", { fields: missing.map((f) => t(f.labelKey)).join(" / ") })}
        </Tag>
      );
    }
    const any = plat.fields.some((f) => (f.required || f.secret) && fieldSet(c, f));
    return any ? <Tag color="success">{t("channels.credOk")}</Tag> : <Tag>{t("channels.noCred")}</Tag>;
  };

  /** 运行状态：桥接类型无运行时；内置适配器以最近一次上报为准 */
  const statusCell = (c: Channel) => {
    if (BRIDGE_TYPES.includes(c.type) || !c.enabled) return <span className="dim">—</span>;
    const st = status[c.id];
    if (!st) return <Tag>{t("channels.stWait")}</Tag>;
    return (
      <Space direction="vertical" size={0}>
        <Tooltip title={`${st.detail} · ${st.at}`}>
          <Tag color={st.state === "ok" ? "success" : "error"} style={{ cursor: "default" }}>
            {st.state === "ok" ? t("channels.stOk") : t("channels.stErr")}
          </Tag>
        </Tooltip>
        <span
          className="mono dim"
          style={{ maxWidth: 210, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
        >
          {st.detail}
        </span>
      </Space>
    );
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="CHANNELS"
        title={t("nav.channels")}
        desc={t("channels.desc")}
        actions={
          <>
            <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
              {t("channels.add")}
            </Button>
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              {t("common.refresh")}
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
          locale={{ emptyText: <Empty description={t("channels.empty")} className="pane-empty" /> }}
          columns={[
            {
              title: t("channels.colAccount"),
              render: (_, c) => (
                <Space direction="vertical" size={0}>
                  <b>
                    {c.account ?? c.id} {c.account ? <span className="mono dim">{c.id}</span> : null}
                  </b>
                  <span className="mono dim">{c.createdAt}</span>
                </Space>
              ),
            },
            { title: t("channels.colPlatform"), render: (_, c) => platformTag(c.type) },
            {
              title: t("channels.colGroup"),
              render: (_, c) =>
                c.group ? <Tag color="purple">{c.group}</Tag> : <Tag>{t("channels.followCurrent")}</Tag>,
            },
            { title: t("channels.colSecret"), render: (_, c) => credTag(c) },
            { title: t("channels.colStatus"), render: (_, c) => statusCell(c) },
            {
              title: t("channels.colReply"),
              render: (_, c) => {
                const plat = PLATFORMS.find((x) => x.key === c.type);
                return plat?.replyBuiltin ? (
                  <Tag>{t("channels.replyBuiltin")}</Tag>
                ) : (
                  <span className="mono dim">{c.replyWebhook ?? "—"}</span>
                );
              },
            },
            {
              title: t("channels.colEnabled"),
              render: (_, c) => <Switch size="small" checked={c.enabled} onChange={(v) => void toggle(c, v)} />,
            },
            {
              title: t("channels.colOps"),
              width: 140,
              render: (_, c) => (
                <Space>
                  <Button size="small" onClick={() => openEdit(c)}>
                    {t("common.edit")}
                  </Button>
                  <Popconfirm title={t("channels.deleteConfirm")} onConfirm={() => void remove(c.id)}>
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

      {channels.some((c) => BRIDGE_TYPES.includes(c.type)) && (
        <div className="inbound-hint hud">
          <div className="rail-label">{t("channels.webhookTitle")}</div>
          <pre className="live-pre">{t("channels.inboundSample", { origin })}</pre>
          <div className="dim">{t("channels.bridgeHint")}</div>
        </div>
      )}

      <Modal
        open={modal}
        title={editing ? t("channels.editTitle", { id: editing.id }) : t("channels.add")}
        onCancel={() => setModal(false)}
        onOk={() => void submit()}
        okText={t("common.save")}
        cancelText={t("common.cancel")}
      >
        <Form form={form} layout="vertical">
          <Form.Item noStyle shouldUpdate={(a, b) => a.platform !== b.platform}>
            {({ getFieldValue }) => {
              const plat = PLATFORMS.find((p) => p.key === getFieldValue("platform")) ?? PLATFORMS[0];
              return (
                <>
                  <Form.Item
                    name="platform"
                    label={t("channels.f.platform")}
                    rules={[{ required: true }]}
                    extra={t(plat.helpKey)}
                  >
                    <Select
                      disabled={editing !== null}
                      options={PLATFORMS.map((p) => ({ value: p.key, label: t(p.labelKey) }))}
                      onChange={() => form.setFieldsValue({ token: undefined, secret: undefined, config: {} })}
                    />
                  </Form.Item>
                  {!editing && (
                    <Form.Item
                      name="id"
                      label={t("channels.f.id")}
                      rules={[{ required: true }, { pattern: /^[A-Za-z0-9_-]{1,48}$/, message: t("channels.idPattern") }]}
                    >
                      <Input placeholder={t("channels.f.idPh")} />
                    </Form.Item>
                  )}
                  {plat.fields.map((f) =>
                    f.toggle ? (
                      <Form.Item
                        key={f.key}
                        name={f.top ? f.key : ["config", f.key]}
                        valuePropName="checked"
                        label={t(f.labelKey)}
                        extra={f.helpKey ? t(f.helpKey) : undefined}
                      >
                        <Switch size="small" />
                      </Form.Item>
                    ) : (
                      <Form.Item
                        key={f.key}
                        name={f.top ? f.key : ["config", f.key]}
                        label={t(f.labelKey)}
                        rules={
                          f.required && !editing
                            ? [{ required: true, message: t("channels.f.required") }]
                            : undefined
                        }
                        extra={f.helpKey ? t(f.helpKey) : undefined}
                      >
                        {f.secret ? (
                          <Input.Password autoComplete="new-password" placeholder={f.ph?.startsWith("channels.") ? t(f.ph as TKey) : f.ph} />
                        ) : (
                          <Input placeholder={f.ph?.startsWith("channels.") ? t(f.ph as TKey) : f.ph} />
                        )}
                      </Form.Item>
                    ),
                  )}
                  <Form.Item name="group" label={t("channels.f.group")}>
                    <Select
                      allowClear
                      placeholder={t("channels.f.groupPh")}
                      options={groups.map((g) => ({ value: g.id, label: `${g.name}（${g.id}）` }))}
                    />
                  </Form.Item>

                  <Collapse
                    ghost
                    className="form-advanced"
                    items={[
                      {
                        key: "adv",
                        label: plat.replyBuiltin ? t("channels.advNative") : t("channels.adv"),
                        children: (
                          <>
                            <Form.Item name="account" label={t("channels.f.account")}>
                              <Input placeholder={t("channels.f.accountPh")} />
                            </Form.Item>
                            <Form.Item
                              name="allowedChats"
                              label={t("channels.f.allowedChats")}
                              extra={t("channels.f.allowedChatsExtra")}
                            >
                              <Input placeholder={t("channels.f.allowedChatsPh")} />
                            </Form.Item>
                            {!plat.replyBuiltin && (
                              <Form.Item name="replyWebhook" label={t("channels.f.replyWebhook")}>
                                <Input placeholder={t("channels.f.replyWebhookPh")} />
                              </Form.Item>
                            )}
                          </>
                        ),
                      },
                    ]}
                  />
                </>
              );
            }}
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
