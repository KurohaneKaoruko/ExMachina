/** 执行审批：被闸门拦截的命令，批准后代执行 / 拒绝 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Empty, Segmented, Space, Spin, Table, Tag, message } from "antd";
import { CheckOutlined, CloseOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type ApprovalRequest } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useT } from "../i18n/core";

export function ApprovalsView(): React.ReactElement {
  const t = useT();
  const [status, setStatus] = useState<string>("pending");
  const [items, setItems] = useState<ApprovalRequest[]>([]);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setItems(await api.listApprovals(status === "all" ? undefined : status, 100));
    } finally {
      setLoading(false);
    }
  }, [status]);

  useEffect(() => {
    void load();
  }, [load]);

  const decide = async (id: string, approve: boolean) => {
    try {
      const r = await api.decideApproval(id, approve);
      if (r.status === "executed") {
        message.success(t("approvals.approvedExecuted", { id }));
      } else if (r.status === "failed") {
        message.warning(t("approvals.approvedFailed", { id }));
      } else {
        message.info(t("approvals.rejected", { id }));
      }
      await load();
    } catch (e) {
      message.error(t("common.opFailed", { err: String(e) }));
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        en="APPROVALS"
        title={t("nav.approvals")}
        desc={t("approvals.desc")}
        actions={
          <>
            <Segmented
              value={status}
              onChange={(v) => setStatus(v as string)}
              options={[
                { value: "pending", label: t("approvals.tabPending") },
                { value: "all", label: t("approvals.tabAll") },
              ]}
            />
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              {t("common.refresh")}
            </Button>
          </>
        }
      />
      <Spin spinning={loading}>
        <Table<ApprovalRequest>
          size="small"
          rowKey="id"
          pagination={{ pageSize: 12 }}
          dataSource={items}
          locale={{ emptyText: <Empty description={t("approvals.empty")} className="pane-empty" /> }}
          columns={[
            {
              title: t("approvals.colStatus"),
              width: 100,
              render: (_, r) =>
                r.status === "pending" ? (
                  <Tag color="warning">{t("approvals.stPending")}</Tag>
                ) : r.status === "executed" ? (
                  <Tag color="success">{t("approvals.stExecuted")}</Tag>
                ) : r.status === "approved" ? (
                  <Tag color="success">{t("approvals.stApproved")}</Tag>
                ) : r.status === "failed" ? (
                  <Tag color="error">{t("approvals.stFailed")}</Tag>
                ) : (
                  <Tag>{t("approvals.stRejected")}</Tag>
                ),
            },
            {
              title: t("approvals.colCommand"),
              render: (_, r) => (
                <Space direction="vertical" size={0} className="full-width">
                  <code className="cmd-code">$ {r.command}</code>
                  <span className="mono dim">
                    {r.agentId} · {r.createdAt}
                  </span>
                </Space>
              ),
            },
            {
              title: t("approvals.colResult"),
              render: (_, r) =>
                r.result ? <span className="dim result-cell">{r.result.slice(0, 240)}</span> : <span className="dim">—</span>,
            },
            {
              title: t("approvals.colOps"),
              width: 160,
              render: (_, r) =>
                r.status === "pending" ? (
                  <Space>
                    <Button size="small" type="primary" icon={<CheckOutlined />} onClick={() => void decide(r.id, true)}>
                      {t("approvals.approve")}
                    </Button>
                    <Button size="small" danger icon={<CloseOutlined />} onClick={() => void decide(r.id, false)}>
                      {t("approvals.reject")}
                    </Button>
                  </Space>
                ) : (
                  <span className="mono dim">{r.decidedAt ?? "—"}</span>
                ),
            },
          ]}
        />
      </Spin>
    </div>
  );
}
