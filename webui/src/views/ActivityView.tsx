/** 事件活动页：会话事件溯源 + 实时调度时间线 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Empty, Select, Space, Spin, Tag } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import { api, type StoredEvent } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";

function describe(e: StoredEvent): { tag: string; color: string; text: string } {
  const kind = e.type ?? e.kind ?? "event";
  const p = (e.payload ?? {}) as Record<string, unknown>;
  let text = "";
  switch (kind) {
    case "dispatch":
      text = `派发 → ${String(p.agentIdentifier ?? "")}（${String(p.taskNodeId ?? p.nodeId ?? "")}）`;
      break;
    case "event": {
      const inner = (p as { kind?: string }).kind ?? "";
      text = `内部事件 ${inner}`;
      break;
    }
    case "session.created":
      text = `会话创建`;
      break;
    default:
      text = JSON.stringify(p).slice(0, 120);
  }
  const color = kind === "dispatch" ? "blue" : kind === "event" ? "default" : "cyan";
  return { tag: kind, color, text };
}

export function ActivityView(): React.ReactElement {
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
        title="活动"
        desc="实时调度流与历史事件回放：对话执行时，这里按时间顺序滚动显示派发、回流、裁决与错误。"
        actions={
          <>
            <Select
              showSearch
              value={sid || undefined}
              placeholder="选择会话"
              style={{ minWidth: 280 }}
              onChange={(v) => setSid(v)}
              options={sessions.map((s) => ({ value: s.id, label: `${s.title}（${s.id.slice(0, 8)}…）` }))}
            />
            <Button icon={<ReloadOutlined />} onClick={() => void load(sid)}>
              刷新
            </Button>
          </>
        }
      />

      <Card size="small" className="hud activity-live" title="实时调度 [LIVE · WS]">
        {timeline.length === 0 ? (
          <span className="dim">暂无实时事件；对话运行时此处滚动显示派发 / 回流 / 裁决。</span>
        ) : (
          timeline
            .slice(-12)
            .reverse()
            .map((t, i) => (
              <div key={i} className={`timeline-item tl-${t.kind}`}>
                <Tag color={t.kind === "dispatch" ? "blue" : t.kind === "sync" ? "green" : t.kind === "arbitration" ? "volcano" : "red"}>
                  {t.kind === "dispatch" ? "派发" : t.kind === "sync" ? "回流" : t.kind === "arbitration" ? "裁决" : "错误"}
                </Tag>
                <span className="tl-text">{t.text}</span>
              </div>
            ))
        )}
      </Card>

      <Spin spinning={loading}>
        <div className="event-stream">
          {events.length === 0 && !loading && <Empty description="该会话无事件记录" className="pane-empty" />}
          {events
            .slice()
            .reverse()
            .map((e, i) => {
              const d = describe(e);
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
