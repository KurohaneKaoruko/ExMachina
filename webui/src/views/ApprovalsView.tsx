/** 执行审批：被闸门拦截的命令，批准后代执行 / 拒绝 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Empty, Segmented, Space, Spin, Table, Tag, message } from "antd";
import { CheckOutlined, CloseOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, type ApprovalRequest } from "../api";
import { PageHeader } from "../components/PageHeader";

export function ApprovalsView(): React.ReactElement {
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
        message.success(`已批准并代执行（审批单 ${id}）`);
      } else if (r.status === "failed") {
        message.warning(`已批准但执行失败（审批单 ${id}）`);
      } else {
        message.info(`已拒绝（审批单 ${id}）`);
      }
      await load();
    } catch (e) {
      message.error(`操作失败：${String(e)}`);
    }
  };

  return (
    <div className="pane-wrap">
      <PageHeader
        no="11"
        en="APPROVALS"
        title="审批"
        desc="终端命令的拦截与放行记录：闸门档位在「设置」页 security.execApproval 控制（off / risky / always），批准后由系统代执行并留存输出。"
        actions={
          <>
            <Segmented
              value={status}
              onChange={(v) => setStatus(v as string)}
              options={[
                { value: "pending", label: "待审批" },
                { value: "all", label: "全部" },
              ]}
            />
            <Button icon={<ReloadOutlined />} onClick={() => void load()}>
              刷新
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
          locale={{ emptyText: <Empty description="无审批单" className="pane-empty" /> }}
          columns={[
            {
              title: "状态",
              width: 100,
              render: (_, r) =>
                r.status === "pending" ? (
                  <Tag color="warning">待审批</Tag>
                ) : r.status === "executed" ? (
                  <Tag color="success">已执行</Tag>
                ) : r.status === "approved" ? (
                  <Tag color="success">已批准</Tag>
                ) : r.status === "failed" ? (
                  <Tag color="error">执行失败</Tag>
                ) : (
                  <Tag>已拒绝</Tag>
                ),
            },
            {
              title: "命令",
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
              title: "结果",
              render: (_, r) =>
                r.result ? <span className="dim result-cell">{r.result.slice(0, 240)}</span> : <span className="dim">—</span>,
            },
            {
              title: "操作",
              width: 160,
              render: (_, r) =>
                r.status === "pending" ? (
                  <Space>
                    <Button size="small" type="primary" icon={<CheckOutlined />} onClick={() => void decide(r.id, true)}>
                      批准
                    </Button>
                    <Button size="small" danger icon={<CloseOutlined />} onClick={() => void decide(r.id, false)}>
                      拒绝
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
