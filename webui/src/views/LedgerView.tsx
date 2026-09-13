/** 三账面板：任务账 / 证据账 / 风险账 */
import React from "react";
import { Card, Col, Empty, List, Row, Tag } from "antd";
import { useExm } from "../store";

const LEVEL_COLORS: Record<string, string> = { A: "red", B: "orange", C: "blue", D: "default" };

export function LedgerView(): React.ReactElement {
  const { ledger } = useExm();
  if (!ledger) return <Empty description="暂无三账数据" />;

  return (
    <Row gutter={16} className="ledger-wrap">
      <Col span={8}>
        <Card title="任务账" size="small">
          <p><b>目标</b></p>
          <p>{ledger.task.goal || "（未设定）"}</p>
          <p><b>验收标准</b></p>
          <List size="small" dataSource={ledger.task.acceptance} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: "（无）" }} />
          <p><b>约束（范围内）</b></p>
          <List size="small" dataSource={ledger.task.constraints} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: "（无）" }} />
          <p><b>禁区</b></p>
          <List size="small" dataSource={ledger.task.forbidden} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: "（无）" }} />
        </Card>
      </Col>
      <Col span={8}>
        <Card title="证据账" size="small">
          <List
            size="small"
            dataSource={ledger.evidence.confirmed}
            locale={{ emptyText: "（尚无确认证据）" }}
            renderItem={(e) => (
              <List.Item>
                <Tag color={LEVEL_COLORS[e.level]}>{e.level}</Tag>
                <span className="ledger-evidence">{e.text}</span>
                <Tag>{e.source}</Tag>
              </List.Item>
            )}
          />
        </Card>
      </Col>
      <Col span={8}>
        <Card title="风险账" size="small">
          <p><b>影响面</b></p>
          <List size="small" dataSource={ledger.risk.impact} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: "（无）" }} />
          <p><b>阻断项</b></p>
          <List size="small" dataSource={ledger.risk.blockers} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: "（无）" }} />
          <p><b>未闭环断言</b></p>
          <List size="small" dataSource={ledger.risk.openAssertions} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: "（无）" }} />
        </Card>
      </Col>
    </Row>
  );
}
