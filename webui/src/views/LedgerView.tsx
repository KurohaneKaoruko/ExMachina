/** 三账面板：任务账 / 证据账 / 风险账 */
import React from "react";
import { Card, Col, Empty, List, Row, Tag } from "antd";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";
import { useT } from "../i18n/core";

const LEVEL_COLORS: Record<string, string> = { A: "red", B: "orange", C: "blue", D: "default" };

export function LedgerView(): React.ReactElement {
  const t = useT();
  const { ledger } = useExm();

  return (
    <div className="pane-wrap">
      <PageHeader
        en="LEDGER"
        title={t("nav.ledger")}
        desc={t("ledger.desc")}
      />
      {!ledger ? (
        <Empty description={t("ledger.empty")} className="pane-empty" />
      ) : (
        <Row gutter={16} className="ledger-wrap">
          <Col span={8}>
            <Card title={t("ledger.taskCard")} size="small" className="hud">
              <p><b>{t("ledger.goal")}</b></p>
              <p>{ledger.task.goal || t("ledger.unset")}</p>
              <p><b>{t("ledger.acceptance")}</b></p>
              <List size="small" dataSource={ledger.task.acceptance} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: t("ledger.none") }} />
              <p><b>{t("ledger.constraints")}</b></p>
              <List size="small" dataSource={ledger.task.constraints} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: t("ledger.none") }} />
              <p><b>{t("ledger.forbidden")}</b></p>
              <List size="small" dataSource={ledger.task.forbidden} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: t("ledger.none") }} />
            </Card>
          </Col>
          <Col span={8}>
            <Card title={t("ledger.evidenceCard")} size="small" className="hud">
              <List
                size="small"
                dataSource={ledger.evidence.confirmed}
                locale={{ emptyText: t("ledger.noEvidence") }}
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
            <Card title={t("ledger.riskCard")} size="small" className="hud">
              <p><b>{t("ledger.impact")}</b></p>
              <List size="small" dataSource={ledger.risk.impact} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: t("ledger.none") }} />
              <p><b>{t("ledger.blockers")}</b></p>
              <List size="small" dataSource={ledger.risk.blockers} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: t("ledger.none") }} />
              <p><b>{t("ledger.openAssertions")}</b></p>
              <List size="small" dataSource={ledger.risk.openAssertions} renderItem={(x) => <List.Item>{x}</List.Item>} locale={{ emptyText: t("ledger.none") }} />
            </Card>
          </Col>
        </Row>
      )}
    </div>
  );
}
