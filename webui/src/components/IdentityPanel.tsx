/** 通道身份面板：配对码签发 / 绑定清单管理（capability-completion 组 3.4）
 *  数据源：/api/identity/list · /api/identity/code · /api/identity/role · /api/identity/:id */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Input, Popconfirm, Space, Table, Tag, message } from "antd";
import { CodeOutlined, ReloadOutlined } from "@ant-design/icons";
import { identityIssue, identityList, identitySetRole, identityUnbind } from "../api";
import { useT } from "../i18n/core";

interface IdentityItem {
  id: string;
  channel: string;
  externalId: string;
  displayName: string;
  role: string;
  pairedAt: string;
  note: string;
}
interface CodeItem {
  code: string;
  note: string;
  platform?: string | null;
  createdAt: string;
  expiresAtMs: number;
  expired: boolean;
}
interface IdentityList {
  identityRequired: boolean;
  identities: IdentityItem[];
  codes: CodeItem[];
}

export function IdentityPanel(): React.ReactElement {
  const t = useT();
  const [data, setData] = useState<IdentityList | null>(null);
  const [note, setNote] = useState("");
  const [issued, setIssued] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setData(await identityList());
    } catch {
      /* 静默 */
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const issue = async () => {
    setBusy(true);
    try {
      const r = await identityIssue(note.trim());
      setIssued(r.code);
      setNote("");
      message.success(t("identity.issued"));
      await load();
    } catch (e) {
      message.error(String(e).slice(0, 160));
    } finally {
      setBusy(false);
    }
  };

  const setRole = async (id: string, role: string) => {
    try {
      await identitySetRole(id, role);
      await load();
    } catch (e) {
      message.error(String(e).slice(0, 160));
    }
  };

  const unbind = async (id: string) => {
    try {
      await identityUnbind(id);
      await load();
    } catch (e) {
      message.error(String(e).slice(0, 160));
    }
  };

  return (
    <Card size="small" className="identity-panel" title={<><CodeOutlined /> {t("identity.title")}</>}
      extra={<Button size="small" type="text" icon={<ReloadOutlined />} onClick={() => void load()} />}>
      <Space.Compact style={{ width: "100%", marginBottom: 12 }}>
        <Input
          value={note}
          placeholder={t("identity.notePlaceholder")}
          onChange={(e) => setNote(e.target.value)}
          onPressEnter={() => void issue()}
        />
        <Button type="primary" loading={busy} onClick={() => void issue()}>{t("identity.issue")}</Button>
      </Space.Compact>
      {issued && (
        <div className="identity-issued">
          <Tag color="green">/pair {issued}</Tag>
          <span className="dim">{t("identity.hint")}</span>
        </div>
      )}
      <Table
        size="small"
        rowKey="code"
        dataSource={data?.codes ?? []}
        pagination={false}
        columns={[
          { title: t("identity.code"), dataIndex: "code", render: (c: string, r: CodeItem) =>
            r.expired ? <span className="dim">{c}</span> : <Tag color="blue">/pair {c}</Tag> },
          { title: t("identity.note"), dataIndex: "note" },
          { title: t("identity.platform"), dataIndex: "platform", render: (p?: string) => p ?? t("identity.any") },
          { title: t("identity.status"), dataIndex: "expired", render: (x: boolean) =>
            x ? <Tag color="default">{t("identity.expired")}</Tag> : <Tag color="processing">{t("identity.active")}</Tag> },
        ]}
      />
      <Table
        size="small"
        style={{ marginTop: 12 }}
        rowKey="id"
        dataSource={data?.identities ?? []}
        pagination={false}
        columns={[
          { title: t("identity.user"), dataIndex: "displayName" },
          { title: t("identity.channel"), dataIndex: "channel" },
          { title: t("identity.externalId"), dataIndex: "externalId" },
          { title: t("identity.role"), dataIndex: "role", render: (role: string, r: IdentityItem) => (
            <Space>
              {role === "admin" ? <Tag color="gold">admin</Tag> : <Tag>member</Tag>}
              {role === "member"
                ? <Button size="small" type="link" onClick={() => void setRole(r.id, "admin")}>{t("identity.toAdmin")}</Button>
                : <Button size="small" type="link" onClick={() => void setRole(r.id, "member")}>{t("identity.toMember")}</Button>}
            </Space>
          ) },
          { title: t("identity.pairedAt"), dataIndex: "pairedAt", render: (s: string) => <span className="mono dim">{s.slice(0, 16)}</span> },
          { title: "", render: (_: unknown, r: IdentityItem) => (
            <Popconfirm title={t("identity.unbindConfirm")} onConfirm={() => void unbind(r.id)}>
              <Button size="small" danger type="text">{t("identity.unbind")}</Button>
            </Popconfirm>
          ) },
        ]}
      />
    </Card>
  );
}
