/** 任务图视图：DAG 实时渲染（自绘 SVG，按拓扑深度分层） */
import React, { useMemo, useState } from "react";
import { Drawer, Empty, Tag, Typography } from "antd";
import { useExm } from "../store";
import type { TaskNode, TaskStatus } from "../types";

const STATUS_COLORS: Record<TaskStatus, string> = {
  pending: "#bfbfbf",
  ready: "#1677ff",
  dispatched: "#2f54eb",
  running: "#faad14",
  syncing: "#722ed1",
  done: "#52c41a",
  blocked: "#f5222d",
  failed: "#cf1322",
  arbitrating: "#eb2f96",
  cancelled: "#8c8c8c",
};

const NODE_W = 200;
const NODE_H = 56;
const GAP_X = 80;
const GAP_Y = 24;

function depthOf(n: TaskNode, byId: Map<string, TaskNode>, memo: Map<string, number>): number {
  if (memo.has(n.id)) return memo.get(n.id)!;
  if (n.dependsOn.length === 0) {
    memo.set(n.id, 0);
    return 0;
  }
  const d = Math.max(
    ...n.dependsOn.map((dep) => {
      const parent = byId.get(dep);
      return parent ? depthOf(parent, byId, memo) + 1 : 1;
    }),
  );
  memo.set(n.id, d);
  return d;
}

export function GraphView(): React.ReactElement {
  const { graph } = useExm();
  const [selected, setSelected] = useState<TaskNode | null>(null);

  const layout = useMemo(() => {
    if (!graph?.nodes?.length) return null;
    const byId = new Map(graph.nodes.map((n) => [n.id, n]));
    const memo = new Map<string, number>();
    const layers = new Map<number, TaskNode[]>();
    for (const n of graph.nodes) {
      const d = depthOf(n, byId, memo);
      if (!layers.has(d)) layers.set(d, []);
      layers.get(d)!.push(n);
    }
    const pos = new Map<string, { x: number; y: number }>();
    for (const [d, nodes] of layers) {
      nodes.forEach((n, i) => {
        pos.set(n.id, { x: d * (NODE_W + GAP_X) + 20, y: i * (NODE_H + GAP_Y) + 20 });
      });
    }
    const width = (Math.max(...layers.keys()) + 1) * (NODE_W + GAP_X) + 40;
    const height = Math.max(...[...layers.values()].map((l) => l.length)) * (NODE_H + GAP_Y) + 40;
    return { pos, width, height, edges: graph.nodes.flatMap((n) => n.dependsOn.map((d) => [d, n.id] as const)) };
  }, [graph]);

  if (!layout) return <Empty description="暂无任务图：发送一条任务后此处实时渲染 DAG" className="graph-empty" />;

  return (
    <div className="graph-wrap">
      <svg width={layout.width} height={layout.height} className="graph-svg">
        {layout.edges.map(([from, to]) => {
          const a = layout.pos.get(from)!;
          const b = layout.pos.get(to)!;
          return (
            <line
              key={`${from}-${to}`}
              x1={a.x + NODE_W}
              y1={a.y + NODE_H / 2}
              x2={b.x}
              y2={b.y + NODE_H / 2}
              stroke="#2a3d4d"
              strokeWidth={2}
              markerEnd="url(#arrow)"
            />
          );
        })}
        <defs>
          <marker id="arrow" markerWidth="8" markerHeight="8" refX="8" refY="4" orient="auto">
            <path d="M0,0 L8,4 L0,8 z" fill="#2a3d4d" />
          </marker>
        </defs>
        {graph!.nodes.map((n) => {
          const p = layout.pos.get(n.id)!;
          const color = STATUS_COLORS[n.status];
          return (
            <g key={n.id} transform={`translate(${p.x},${p.y})`} onClick={() => setSelected(n)} className="graph-node">
              <rect width={NODE_W} height={NODE_H} rx={2} fill="#0f151c" stroke={color} strokeWidth={2} />
              <circle cx={14} cy={14} r={5} fill={color} />
              <text x={26} y={19} fontSize={13} fontWeight={600} fill="#d6e4ec">
                {n.id} {n.title.length > 10 ? n.title.slice(0, 10) + "…" : n.title}
              </text>
              <text x={14} y={40} fontSize={11} fill="#6d8296">
                {n.agentIdentifier} · {n.status}
              </text>
            </g>
          );
        })}
      </svg>

      <Drawer open={!!selected} onClose={() => setSelected(null)} title={selected ? `${selected.id} ${selected.title}` : ""} width={420}>
        {selected && (
          <div>
            <p><Tag color={STATUS_COLORS[selected.status]}>{selected.status}</Tag> <b>{selected.agentIdentifier}</b> <Tag>{selected.priority}</Tag></p>
            <Typography.Title level={5}>目标</Typography.Title>
            <p>{selected.objective}</p>
            <Typography.Title level={5}>验收断言</Typography.Title>
            <ul>{selected.acceptance.map((a, i) => <li key={i}>{a}</li>)}</ul>
            <Typography.Title level={5}>依赖</Typography.Title>
            <p>{selected.dependsOn.join("、") || "（根节点）"}</p>
            {selected.syncReportId && <p>回流 ID：{selected.syncReportId}</p>}
          </div>
        )}
      </Drawer>
    </div>
  );
}
