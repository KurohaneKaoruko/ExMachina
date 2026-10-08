/**
 * 记忆视图：深层记忆（条目/检索/固定/维护）+ 个体可靠性统计
 * 数据与操作契约见 docs/08 §6
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Card,
  Col,
  Descriptions,
  Empty,
  Input,
  List,
  message,
  Popconfirm,
  Row,
  Select,
  Space,
  Statistic,
  Table,
  Tag,
  Tooltip,
} from "antd";
import { DeleteOutlined, PushpinOutlined, ReloadOutlined, SearchOutlined } from "@ant-design/icons";
import { api, type LlmProfile, type MemoryEntry, type MemoryStats, type RecallHit } from "../api";
import { PageHeader } from "../components/PageHeader";
import { useExm } from "../store";
import { useT } from "../i18n/core";

function MdEditor(): React.ReactElement {
  const t = useT();
  const [content, setContent] = useState("");
  const [path, setPath] = useState("");
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        const key = localStorage.getItem("exm.key") ?? "";
        const resp = await fetch("/api/memory/md", {
          headers: key ? { "X-Auth-Key": key } : {},
        });
        const data = (await resp.json()) as { content: string; path: string; deepEnabled?: boolean };
        setContent(data.content);
        setPath(data.path);
      } catch (e) {
        message.error(t("memory.mdReadFailed", { err: String(e) }));
      }
    })();
  }, []);

  const save = async () => {
    setSaving(true);
    try {
      const key = localStorage.getItem("exm.key") ?? "";
      const resp = await fetch("/api/memory/md", {
        method: "PUT",
        headers: { "Content-Type": "application/json", ...(key ? { "X-Auth-Key": key } : {}) },
        body: JSON.stringify({ content }),
      });
      const data = (await resp.json()) as { ok?: boolean; compacting?: boolean; error?: string };
      setDirty(false);
      if (data.compacting) {
        message.info(t("memory.mdCompacting"));
      } else if (data.error) {
        message.error(data.error);
      } else {
        message.success(t("memory.mdSaved"));
      }
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="memory-wrap">
      <PageHeader
        en="MEMORY"
        title={t("nav.memory")}
        desc={t("memory.mdDesc")}
      />
      <Card
        size="small"
        title={t("memory.mdCard")}
        extra={
          <Button type="primary" size="small" loading={saving} disabled={!dirty} onClick={() => void save()}>
            {t("common.save")}
          </Button>
        }
      >
        <div className="dim" style={{ marginBottom: 8 }}>
          {t("memory.mdHint", { path: path || "memory.md", n: content.length })}
        </div>
        <Input.TextArea
          value={content}
          onChange={(e) => {
            setContent(e.target.value);
            setDirty(true);
          }}
          rows={22}
          style={{ fontFamily: "inherit" }}
        />
      </Card>
    </div>
  );
}

const KIND_COLORS: Record<string, string> = {
  fact: "blue",
  decision: "purple",
  preference: "gold",
  evidence: "cyan",
  digest: "default",
  lesson: "volcano",
  stat: "green",
};

export function MemoryView(): React.ReactElement {
  const t = useT();
  const deepEnabled = useExm((s) => s.config?.memory?.enabled !== false);
  const memoryVersion = useExm((s) => s.memoryVersion);
  const agents = useExm((s) => s.agents);
  const groups = useExm((s) => s.groups);
  const activeGroup = useExm((s) => s.activeGroup);
  const [entries, setEntries] = useState<MemoryEntry[]>([]);
  const [stats, setStats] = useState<MemoryStats | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<RecallHit[] | null>(null);
  const [kindFilter, setKindFilter] = useState<string | undefined>(undefined);
  const [agentFilter, setAgentFilter] = useState<string | undefined>(undefined);
  // 语义检索目标（档案ID，或 档案ID/模型名）；空 = 仅词项召回
  const semanticCfg = useExm((s) => s.config?.memory?.semanticModel ?? "");
  const [profiles, setProfiles] = useState<LlmProfile[]>([]);
  const [semanticDraft, setSemanticDraft] = useState(semanticCfg);
  useEffect(() => setSemanticDraft(semanticCfg), [semanticCfg]);
  useEffect(() => {
    void (async () => {
      try {
        setProfiles((await api.llmProfiles()).profiles);
      } catch {
        /* 提供商未加载时保持空列表 */
      }
    })();
  }, []);
  const semanticOptions = useMemo(() => {
    const opts = [{ value: "", label: t("memory.noSemantic") }];
    for (const p of profiles) {
      opts.push({ value: p.id, label: `${p.name}${p.model ? ` · ${p.model}` : ""}` });
    }
    return opts;
  }, [profiles, t]);
  const saveSemantic = async () => {
    try {
      await api.putConfig({ memory: { semanticModel: semanticDraft } });
      message.success(semanticDraft ? t("memory.semanticOn") : t("memory.semanticOff"));
    } catch (e) {
      message.error(t("common.saveFailed", { err: String(e) }));
    }
  };

  const reload = useCallback(async () => {
    const [list, st] = await Promise.all([
      api.listMemory({ kind: kindFilter, agent: agentFilter, limit: 60 }),
      api.memoryStats(),
    ]);
    setEntries(list);
    setStats(st);
  }, [kindFilter, agentFilter]);

  useEffect(() => {
    void reload();
  }, [reload, memoryVersion]);

  if (!deepEnabled) {
    return <MdEditor />;
  }

  return (
    <div className="memory-wrap">
      <PageHeader
        en="MEMORY"
        title={t("nav.memory")}
        desc={t("memory.desc")}
      />
      <Row gutter={16}>
        <Col span={6}>
          <Card size="small">
            <Statistic title={t("memory.statTotal")} value={stats?.memory?.total ?? 0} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small">
            <Statistic title={t("memory.statShared")} value={stats?.memory?.shared ?? 0} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small">
            <Statistic title={t("memory.statIndividual")} value={stats?.memory?.individual ?? 0} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small">
            <Statistic title={t("memory.statPinned")} value={stats?.memory?.pinned ?? 0} />
          </Card>
        </Col>
      </Row>

      <Card
        size="small"
        className="memory-card"
        title={t("memory.semanticCard")}
        extra={
          <Button size="small" type="primary" onClick={() => void saveSemantic()}>
            {t("common.save")}
          </Button>
        }
      >
        <div className="model-kv">
          <span className="k">{t("memory.embedModel")}</span>
          <Select
            size="small"
            style={{ minWidth: 280 }}
            value={semanticDraft}
            onChange={setSemanticDraft}
            options={semanticOptions}
          />
        </div>
        <div className="pane-hint" style={{ marginTop: 8 }}>
          {t("memory.semanticHint")}
        </div>
      </Card>

      <Card
        size="small"
        className="memory-card"
        title={t("memory.searchCard")}
        extra={
          <Space>
            <Button size="small" icon={<ReloadOutlined />} onClick={() => void reload()}>
              {t("common.refresh")}
            </Button>
            <Button
              size="small"
              onClick={async () => {
                await api.reindexMemory();
                message.success(t("memory.reindexed"));
                void reload();
              }}
            >
              {t("memory.reindex")}
            </Button>
            <Button
              size="small"
              onClick={async () => {
                await api.decayMemory();
                message.success(t("memory.decayed"));
                void reload();
              }}
            >
              {t("memory.decay")}
            </Button>
            <Button
              size="small"
              onClick={async () => {
                await api.renderMemory();
                message.success(t("memory.rendered"));
              }}
            >
              {t("memory.render")}
            </Button>
          </Space>
        }
      >
        <Space.Compact style={{ width: "100%" }}>
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("memory.searchPh")}
            onPressEnter={async () => {
              const r = await api.searchMemory(query, 8, agentFilter);
              setHits(r.hits);
            }}
          />
          <Button
            type="primary"
            icon={<SearchOutlined />}
            onClick={async () => {
              const r = await api.searchMemory(query, 8, agentFilter);
              setHits(r.hits);
            }}
          >
            {t("memory.search")}
          </Button>
        </Space.Compact>
        <div className="memory-scope-hint">
          {t("memory.scope")}{agentFilter ? t("memory.scopeAgent", { id: agentFilter }) : t("memory.scopeGroup")}
        </div>

        {hits !== null && (
          <List
            className="memory-hits"
            size="small"
            dataSource={hits}
            locale={{ emptyText: t("memory.noHits") }}
            renderItem={(h) => (
              <List.Item>
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Space>
                    <Tag color={KIND_COLORS[h.entry.kind] ?? "default"}>{h.entry.kind}</Tag>
                    <b>{h.entry.title}</b>
                    <Tag color="green">{t("memory.score", { s: h.score.toFixed(2) })}</Tag>
                    <span className="memory-reason">{h.reasons.join("；")}</span>
                  </Space>
                  <span>{h.entry.body.slice(0, 200)}</span>
                </Space>
              </List.Item>
            )}
          />
        )}
      </Card>

      <Card size="small" className="memory-card" title={t("memory.entriesCard")}>
        <Space className="memory-filters" wrap>
          {["", "fact", "decision", "preference", "evidence", "digest", "lesson"].map((k) => (
            <Tag.CheckableTag
              key={k || "all"}
              checked={(kindFilter ?? "") === k}
              onChange={() => setKindFilter(k || undefined)}
            >
              {k === "" ? t("memory.allKinds") : k}
            </Tag.CheckableTag>
          ))}
          <span className="memory-scope-label">{t("memory.agentScope")}</span>
          <Select
            size="small"
            allowClear
            placeholder={t("memory.allAgents")}
            style={{ minWidth: 220 }}
            value={agentFilter}
            onChange={(v) => setAgentFilter(v ?? undefined)}
            options={agents
              .filter(
                (a) => a.identifier !== groups.find((g) => g.id === activeGroup)?.primary,
              )
              .map((a) => ({ value: a.identifier, label: `${a.name} (${a.identifier})` }))}
          />
        </Space>
        <Table
          size="small"
          rowKey="id"
          dataSource={entries}
          pagination={{ pageSize: 10 }}
          locale={{ emptyText: <Empty description={t("memory.emptyEntries")} /> }}
          columns={[
            {
              title: t("memory.colKind"),
              dataIndex: "kind",
              width: 100,
              render: (k: string) => <Tag color={KIND_COLORS[k] ?? "default"}>{k}</Tag>,
            },
            {
              title: t("memory.colLayer"),
              key: "layer",
              width: 80,
              render: (_, e) =>
                e.agentId ? (
                  <Tag color="purple" title={t("memory.belongTo", { id: e.agentId })}>
                    {t("memory.layerIndividual")}
                  </Tag>
                ) : (
                  <Tag color="blue">{t("memory.layerGroup")}</Tag>
                ),
            },
            {
              title: t("memory.colTitle"),
              key: "title",
              render: (_, e) => (
                <Space direction="vertical" size={0}>
                  <span>
                    {e.pinned && <Tag color="orange">{t("memory.pinned")}</Tag>}
                    <b>{e.title}</b>
                  </span>
                  <span className="memory-body">{e.body.slice(0, 140)}</span>
                </Space>
              ),
            },
            {
              title: t("memory.colImportance"),
              dataIndex: "importance",
              width: 90,
              render: (v: number) => v.toFixed(2),
              sorter: (a: MemoryEntry, b: MemoryEntry) => a.importance - b.importance,
            },
            { title: t("memory.colConf"), dataIndex: "confidence", width: 90, render: (v: number) => v.toFixed(2) },
            { title: t("memory.colAccess"), dataIndex: "accessCount", width: 70 },
            {
              title: t("memory.colOps"),
              key: "ops",
              width: 120,
              render: (_, e) => (
                <Space>
                  <Tooltip title={e.pinned ? t("memory.unpin") : t("memory.pinTip")}>
                    <Button
                      size="small"
                      type="text"
                      icon={<PushpinOutlined />}
                      onClick={async () => {
                        await api.pinMemory(e.id, !e.pinned);
                        void reload();
                      }}
                    />
                  </Tooltip>
                  <Popconfirm
                    title={t("memory.forgetConfirm")}
                    onConfirm={async () => {
                      await api.forgetMemory(e.id);
                      void reload();
                    }}
                  >
                    <Button size="small" type="text" danger icon={<DeleteOutlined />} />
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </Card>

      <Card size="small" className="memory-card" title={t("memory.reliabilityCard")}>
        <Descriptions size="small" column={1} className="memory-dbpath">
          <Descriptions.Item label={t("memory.dbLabel")}>{stats?.memory?.dbPath ?? "-"}</Descriptions.Item>
        </Descriptions>
        <Table
          size="small"
          rowKey="agentId"
          dataSource={stats?.agentStats ?? []}
          pagination={{ pageSize: 8 }}
          locale={{ emptyText: t("memory.emptyStats") }}
          columns={[
            { title: t("memory.colAgent"), dataIndex: "agentId" },
            { title: t("memory.colRuns"), dataIndex: "runs", width: 70 },
            { title: t("memory.colDone"), dataIndex: "done", width: 70 },
            { title: t("memory.colBlocked"), dataIndex: "blocked", width: 70 },
            { title: t("memory.colFailed"), dataIndex: "failed", width: 70 },
            {
              title: t("memory.colAvgConf"),
              dataIndex: "avgConfidence",
              width: 120,
              render: (v: number) => v.toFixed(2),
            },
          ]}
        />
      </Card>
    </div>
  );
}
