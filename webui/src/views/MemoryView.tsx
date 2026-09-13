/**
 * 记忆视图：深层记忆（条目/检索/固定/维护）+ 个体可靠性统计
 * 数据与操作契约见 docs/08 §6
 */
import React, { useCallback, useEffect, useState } from "react";
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
import { api, type MemoryEntry, type MemoryStats, type RecallHit } from "../api";
import { useExm } from "../store";

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

  return (
    <div className="memory-wrap">
      <Row gutter={16}>
        <Col span={6}>
          <Card size="small">
            <Statistic title="记忆条目" value={stats?.memory.total ?? 0} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small">
            <Statistic title="群体记忆（共享）" value={stats?.memory.shared ?? 0} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small">
            <Statistic title="个体记忆（私有）" value={stats?.memory.individual ?? 0} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small">
            <Statistic title="固定记忆（进入 memory.md）" value={stats?.memory.pinned ?? 0} />
          </Card>
        </Col>
      </Row>

      <Card
        size="small"
        className="memory-card"
        title="检索深层记忆"
        extra={
          <Space>
            <Button size="small" icon={<ReloadOutlined />} onClick={() => void reload()}>
              刷新
            </Button>
            <Button
              size="small"
              onClick={async () => {
                await api.reindexMemory();
                message.success("倒排索引已重建");
                void reload();
              }}
            >
              重建索引
            </Button>
            <Button
              size="small"
              onClick={async () => {
                await api.decayMemory();
                message.success("已执行时间衰减整理");
                void reload();
              }}
            >
              衰减整理
            </Button>
            <Button
              size="small"
              onClick={async () => {
                await api.renderMemory();
                message.success("memory.md 已重渲染");
              }}
            >
              重渲染基础记忆
            </Button>
          </Space>
        }
      >
        <Space.Compact style={{ width: "100%" }}>
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="输入关键词（支持中文，按二元切分匹配）"
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
            检索
          </Button>
        </Space.Compact>
        <div className="memory-scope-hint">
          检索范围：{agentFilter ? `个体 ${agentFilter} 可见（私有 + 群体共享）` : "群体记忆"}
        </div>

        {hits !== null && (
          <List
            className="memory-hits"
            size="small"
            dataSource={hits}
            locale={{ emptyText: "无命中" }}
            renderItem={(h) => (
              <List.Item>
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Space>
                    <Tag color={KIND_COLORS[h.entry.kind] ?? "default"}>{h.entry.kind}</Tag>
                    <b>{h.entry.title}</b>
                    <Tag color="green">得分 {h.score.toFixed(2)}</Tag>
                    <span className="memory-reason">{h.reasons.join("；")}</span>
                  </Space>
                  <span>{h.entry.body.slice(0, 200)}</span>
                </Space>
              </List.Item>
            )}
          />
        )}
      </Card>

      <Card size="small" className="memory-card" title="记忆条目（按重要性排序）">
        <Space className="memory-filters" wrap>
          {["", "fact", "decision", "preference", "evidence", "digest", "lesson"].map((k) => (
            <Tag.CheckableTag
              key={k || "all"}
              checked={(kindFilter ?? "") === k}
              onChange={() => setKindFilter(k || undefined)}
            >
              {k === "" ? "全部类型" : k}
            </Tag.CheckableTag>
          ))}
          <span className="memory-scope-label">个体范围：</span>
          <Select
            size="small"
            allowClear
            placeholder="全部（含所有个体）"
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
          locale={{ emptyText: <Empty description="记忆库为空：执行任务或手动添加后写入" /> }}
          columns={[
            {
              title: "类型",
              dataIndex: "kind",
              width: 100,
              render: (k: string) => <Tag color={KIND_COLORS[k] ?? "default"}>{k}</Tag>,
            },
            {
              title: "层",
              key: "layer",
              width: 80,
              render: (_, e) =>
                e.agentId ? (
                  <Tag color="purple" title={`归属: ${e.agentId}`}>
                    个体
                  </Tag>
                ) : (
                  <Tag color="blue">群体</Tag>
                ),
            },
            {
              title: "标题 / 内容",
              key: "title",
              render: (_, e) => (
                <Space direction="vertical" size={0}>
                  <span>
                    {e.pinned && <Tag color="orange">固定</Tag>}
                    <b>{e.title}</b>
                  </span>
                  <span className="memory-body">{e.body.slice(0, 140)}</span>
                </Space>
              ),
            },
            {
              title: "重要性",
              dataIndex: "importance",
              width: 90,
              render: (v: number) => v.toFixed(2),
              sorter: (a: MemoryEntry, b: MemoryEntry) => a.importance - b.importance,
            },
            { title: "置信度", dataIndex: "confidence", width: 90, render: (v: number) => v.toFixed(2) },
            { title: "访问", dataIndex: "accessCount", width: 70 },
            {
              title: "操作",
              key: "ops",
              width: 120,
              render: (_, e) => (
                <Space>
                  <Tooltip title={e.pinned ? "取消固定" : "固定到 memory.md"}>
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
                    title="确认遗忘该条记忆？"
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

      <Card size="small" className="memory-card" title="个体可靠性统计（自我进化反馈：选路依据）">
        <Descriptions size="small" column={1} className="memory-dbpath">
          <Descriptions.Item label="记忆库">{stats?.memory.dbPath ?? "-"}</Descriptions.Item>
        </Descriptions>
        <Table
          size="small"
          rowKey="agentId"
          dataSource={stats?.agentStats ?? []}
          pagination={{ pageSize: 8 }}
          locale={{ emptyText: "暂无：执行任务后累积" }}
          columns={[
            { title: "个体", dataIndex: "agentId" },
            { title: "执行", dataIndex: "runs", width: 70 },
            { title: "完成", dataIndex: "done", width: 70 },
            { title: "受阻", dataIndex: "blocked", width: 70 },
            { title: "失败", dataIndex: "failed", width: 70 },
            {
              title: "平均置信度",
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
