/** 事件活动页：会话事件溯源 + 实时调度时间线 */
import React, { useCallback, useEffect, useState } from "react";
import { Button, Card, Select, Space, Spin, Tag } from "antd";
import { BrandEmpty } from "../components/BrandEmpty";
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
  const [tab, setTab] = useState<"events" | "audit">("events");
  const [sid, setSid] = useState<string>("");
  const [events, setEvents] = useState<StoredEvent[]>([]);
  const [loading, setLoading] = useState(false);
  return (
    <div className="pane-wrap">
      <PageHeader
        en="EVENTS"
        title={t("nav.events")}
        desc={t("activity.desc")}
        actions={
          <Space>
            <Button type={tab === "events" ? "primary" : "default"} size="small" onClick={() => setTab("events")}>
              {t("activity.tabEvents")}
            </Button>
            <Button type={tab === "audit" ? "primary" : "default"} size="small" onClick={() => setTab("audit")}>
              {t("activity.tabAudit")}
            </Button>
          </Space>
        }
      />
      <div className="pane-body">{tab === "events" ? <EventsPane /> : <AuditPane />}</div>
    </div>
  );
}

/** 工具审计视图（ops-visibility）：按个体/工具过滤的调用明细 */
function AuditPane(): React.ReactElement {
  const t = useT();
  const { agents } = useExm();
  const [agent, setAgent] = useState<string | undefined>(undefined);
  const [tool, setTool] = useState<string>("");
  const [items, setItems] = useState<Array<Record<string, unknown>>>([]);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const r = await api.audit({ agent, tool, limit: 300 });
      setItems(r.items);
    } finally {
      setLoading(false);
    }
  }, [agent, tool]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="pane-body">
      <Space style={{ marginBottom: 10 }} wrap>
        <Select
          allowClear
          showSearch
          style={{ minWidth: 180 }}
          placeholder={t("activity.auditAgent")}
          value={agent}
          onChange={(v) => setAgent(v || undefined)}
          options={agents.map((a) => ({ value: a.identifier, label: `${a.name} (${a.identifier})` }))}
        />
        <Select
          allowClear
          showSearch
          style={{ minWidth: 160 }}
          placeholder={t("activity.auditTool")}
          value={tool || undefined}
          onChange={(v) => setTool(v || "")}
          options={[...new Set(items.map((i) => String(i.tool ?? "")))].filter(Boolean).map((tl) => ({ value: tl, label: tl }))}
        />
        <Button icon={<ReloadOutlined />} onClick={() => void load()}>{t("common.refresh")}</Button>
      </Space>
      <Spin spinning={loading}>
        {items.length === 0 && <BrandEmpty description={t("activity.auditEmpty")} className="pane-empty" />}
        {items.map((it, idx) => {
          const key = String(it.callId ?? idx);
          const isOpen = expanded === key;
          return (
            <Card key={key} size="small" className="hud" style={{ marginBottom: 6 }}>
              <div className="audit-row" onClick={() => setExpanded(isOpen ? null : key)} style={{ cursor: "pointer" }}>
                <Tag color={it.ok ? "success" : "error"}>{String(it.ok ? "OK" : "ERR")}</Tag>
                <span className="mono">{String(it.tool ?? "")}</span>
                <span className="dim">{String(it.agentId ?? "")}</span>
                <span className="dim mono" style={{ marginLeft: "auto" }}>
                  {String(it.createdAt ?? "").replace("T", " ").slice(0, 19)} · {String(it.durationMs ?? 0)}ms
                </span>
              </div>
              {isOpen && (
                <div className="usage-body" style={{ borderTop: "1px dashed var(--line)", marginTop: 6 }}>
                  <div className="dim" style={{ marginBottom: 4 }}>{t("activity.auditArgs")}</div>
                  <pre className="usage-think">{JSON.stringify(it.args ?? {}, null, 2)}</pre>
                  <div className="dim" style={{ margin: "6px 0 4px" }}>{t("activity.auditSummary")}</div>
                  <pre className="usage-think">{String(it.summary ?? "")}</pre>
                </div>
              )}
            </Card>
          );
        })}
      </Spin>
    </div>
  );
}

/** 事件视图（原有会话事件溯源） */
function EventsPane(): React.ReactElement {
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
          {events.length === 0 && !loading && <BrandEmpty description={t("activity.empty")} className="pane-empty" />}
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
