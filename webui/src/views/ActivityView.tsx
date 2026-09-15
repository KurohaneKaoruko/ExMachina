/** 事件活动页：会话事件溯源 + 实时调度时间线 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Empty, Select, Space, Spin, Tag } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import { api, type StoredEvent } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";
import { useT } from "../i18n/core";

function describe(e: StoredEvent, t: ReturnType<typeof useT>): { tag: string; color: string; text: string } {
  const kind = e.type ?? e.kind ?? "event";
  const p = (e.payload ?? {}) as Record<string, unknown>;
  let text = "";
  switch (kind) {
    case "dispatch":
      text = t("activity.dispatch", {
        agent: String(p.agentIdentifier ?? ""),
        node: String(p.taskNodeId ?? p.nodeId ?? ""),
      });
      break;
    case "event": {
      const inner = (p as { kind?: string }).kind ?? "";
      text = t("activity.innerEvent", { kind: inner });
      break;
    }
    case "session.created":
      text = t("activity.sessionCreated");
      break;
    default:
      text = JSON.stringify(p).slice(0, 120);
  }
  const color = kind === "dispatch" ? "blue" : kind === "event" ? "default" : "cyan";
  return { tag: kind, color, text };
}

export function ActivityView(): React.ReactElement {
  const t = useT();
  const { sessions, timeline } = useExm();
  const [sid, setSid] = useState<string>("");
  const [events, setEvents] = useState<StoredEvent[]>([]);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async (id: string) => {
    if (!id) return;
    setLoading(true);
    try {
      setEvents(await api.sessionEvents(id, 300));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!sid && sessions.length > 0) {
      setSid(sessions[0].id);
    }
  }, [sessions, sid]);

  useEffect(() => {
    if (sid) void load(sid);
  }, [sid, load]);

  return (
    <div className="pane-wrap">
      <PageHeader
        en="EVENTS"
        title={t("activity.title")}
        desc={t("activity.desc")}
        actions={
          <>
            <Select
              showSearch
              value={sid || undefined}
              placeholder={t("activity.pickSession")}
              style={{ minWidth: 280 }}
              onChange={(v) => setSid(v)}
              options={sessions.map((s) => ({ value: s.id, label: `${s.title}（${s.id.slice(0, 8)}…）` }))}
            />
            <Button icon={<ReloadOutlined />} onClick={() => void load(sid)}>
              {t("common.refresh")}
            </Button>
          </>
        }
      />

      <Card size="small" className="hud activity-live" title={t("activity.liveTitle")}>
        {timeline.length === 0 ? (
          <span className="dim">{t("activity.liveEmpty")}</span>
        ) : (
          timeline
            .slice(-12)
            .reverse()
            .map((tl, i) => (
              <div key={i} className={`timeline-item tl-${tl.kind}`}>
                <Tag color={tl.kind === "dispatch" ? "blue" : tl.kind === "sync" ? "green" : tl.kind === "arbitration" ? "volcano" : "red"}>
                  {tl.kind === "dispatch" ? t("activity.tl.dispatch") : tl.kind === "sync" ? t("activity.tl.sync") : tl.kind === "arbitration" ? t("activity.tl.arbitration") : t("activity.tl.error")}
                </Tag>
                <span className="tl-text">{tl.text}</span>
              </div>
            ))
        )}
      </Card>

      <Spin spinning={loading}>
        <div className="event-stream">
          {events.length === 0 && !loading && <Empty description={t("activity.empty")} className="pane-empty" />}
          {events
            .slice()
            .reverse()
            .map((e, i) => {
              const d = describe(e, t);
              return (
                <div key={i} className="event-row">
                  <Tag color={d.color} className="mono">
                    {d.tag}
                  </Tag>
                  <span className="event-text">{d.text}</span>
                  <span className="mono dim event-time">{e.createdAt ?? ""}</span>
                </div>
              );
            })}
        </div>
      </Spin>
    </div>
  );
}
