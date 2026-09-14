/** 任务图视图：DAG 实时渲染（自绘 SVG，按拓扑深度分层） */
import React, { useMemo, useState } from "react";
import { Drawer, Empty, Tag, Typography } from "antd";
import { useExm } from "../store";
import { PageHeader } from "../components/PageHeader";
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
    return {
      pos,
      width,
      height,
      depth: Math.max(...layers.keys()) + 1,
      edges: graph.nodes.flatMap((n) => n.dependsOn.map((d) => [d, n.id] as const)),
    };
  }, [graph]);

  if (!layout) {
    return (
      <div className="pane-wrap">
        <PageHeader
          en="DAG"
          title="任务图"
          desc="指挥体拆解出的任务依赖图：按拓扑深度分层，节点随执行推进变色，点开可查看目标、验收断言与依赖。"
        />
        <Empty description="暂无任务图：发送一条任务后此处实时渲染 DAG" className="graph-empty" />
      </div>
    );
  }

  return (
    <div className="pane-wrap">
      <PageHeader
        en="DAG"
        title="任务图"
        desc="指挥体拆解出的任务依赖图：按拓扑深度分层，节点随执行推进变色，点开可查看目标、验收断言与依赖。"
        actions={
          <span className="readout">
            <span className="k">NODES</span>
            <span className="v">{graph!.nodes.length}</span>
            <span className="k" style={{ marginLeft: 10 }}>EDGES</span>
            <span className="v">{layout.edges.length}</span>
            <span className="k" style={{ marginLeft: 10 }}>DEPTH</span>
            <span className="v">{layout.depth}</span>
          </span>
        }
      />
      <div className="graph-wrap hud">
        <svg width={layout.width} height={layout.height} className="graph-svg">
          {/* 顶部与左侧坐标刻度：工程图纸的量度参照 */}
          <g className="graph-rule" aria-hidden>
            {Array.from({ length: Math.ceil(layout.width / 40) + 1 }, (_, i) => (
              <line key={`tx${i}`} x1={i * 40} y1={0} x2={i * 40} y2={i % 5 === 0 ? 8 : 4} />
            ))}
            {Array.from({ length: Math.ceil(layout.height / 40) + 1 }, (_, i) => (
              <line key={`ty${i}`} x1={0} y1={i * 40} x2={i % 5 === 0 ? 8 : 4} y2={i * 40} />
            ))}
          </g>
          {layout.edges.map(([from, to]) => {
            const a = layout.pos.get(from)!;
            const b = layout.pos.get(to)!;
            const mx = a.x + NODE_W + (b.x - a.x - NODE_W) / 2;
            return (
              <polyline
                key={`${from}-${to}`}
                points={`${a.x + NODE_W},${a.y + NODE_H / 2} ${mx},${a.y + NODE_H / 2} ${mx},${b.y + NODE_H / 2} ${b.x},${b.y + NODE_H / 2}`}
                fill="none"
                stroke="var(--line-strong)"
                strokeWidth={1.4}
                markerEnd="url(#arrow)"
              />
            );
          })}
          <defs>
            <marker id="arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto">
              <path d="M0,0 L8,4 L0,8 z" fill="var(--line-strong)" />
            </marker>
          </defs>
          {graph!.nodes.map((n) => {
            const p = layout.pos.get(n.id)!;
            const color = STATUS_COLORS[n.status];
            return (
              <g key={n.id} transform={`translate(${p.x},${p.y})`} onClick={() => setSelected(n)} className="graph-node">
                {/* 面板：状态色顶轨 + 底部量度刻线 */}
                <rect width={NODE_W} height={NODE_H} rx={0} fill="var(--bg-panel)" stroke="var(--line-strong)" strokeWidth={1} />
                <rect x={0} y={0} width={NODE_W} height={2.5} fill={color} />
                <rect x={0} y={0} width={3} height={NODE_H} fill={color} opacity={0.8} />
                <line x1={10} y1={NODE_H - 6} x2={NODE_W - 10} y2={NODE_H - 6} stroke="var(--line)" strokeWidth={1} />
                <rect x={10} y={NODE_H - 6} width={NODE_W * 0.32} height={2} fill={color} opacity={0.5} />
                <circle cx={16} cy={21} r={3.5} fill={color} />
                <text x={27} y={25} fontSize={12.5} fontWeight={600} fill="var(--text-hi)">
                  {n.id} {n.title.length > 10 ? n.title.slice(0, 10) + "…" : n.title}
                </text>
                <text x={12} y={43} fontSize={10} fill="var(--muted)" fontFamily="var(--mono)" letterSpacing="0.6">
                  {n.agentIdentifier} · {n.status}
                </text>
              </g>
            );
          })}
        </svg>
      </div>

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
