//! 任务 DAG 与调度器 —— docs/01 §2.2
//! 图状态单线程维护（无锁），子个体执行在 tokio 并发池中；集群演进时状态机外化为存储。

use crate::types::*;
use futures_util::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::task::JoinSet;

pub type Reports = HashMap<String, SyncReport>;

/// 子个体执行结果（由执行器返回，调度器统一落到图上）
#[derive(Debug, Clone)]
pub struct NodeOutcome {
    pub node_id: String,
    pub status: TaskStatus,
    pub error: Option<String>,
    /// 裁决/扩图申请：由调度器追加为依赖本节点的新节点
    pub appends: Vec<NewNode>,
}

#[derive(Debug, Clone)]
pub struct NewNode {
    pub agent_identifier: String,
    pub title: String,
    pub objective: String,
    pub acceptance: Vec<String>,
    pub priority: Priority,
}

impl NodeOutcome {
    pub fn done(node_id: impl Into<String>) -> Self {
        NodeOutcome { node_id: node_id.into(), status: TaskStatus::Done, error: None, appends: vec![] }
    }
    pub fn blocked(node_id: impl Into<String>) -> Self {
        NodeOutcome { node_id: node_id.into(), status: TaskStatus::Blocked, error: None, appends: vec![] }
    }
    pub fn failed(node_id: impl Into<String>, err: impl Into<String>) -> Self {
        NodeOutcome {
            node_id: node_id.into(),
            status: TaskStatus::Failed,
            error: Some(err.into()),
            appends: vec![],
        }
    }
    pub fn with_appends(mut self, appends: Vec<NewNode>) -> Self {
        self.appends = appends;
        self
    }
}

pub struct TaskGraphModel {
    pub id: String,
    pub session_id: String,
    pub status: GraphStatus,
    pub created_at: String,
    nodes: HashMap<String, TaskNode>,
    order: Vec<String>,
}

impl TaskGraphModel {
    pub fn new(session_id: impl Into<String>) -> Self {
        TaskGraphModel {
            id: new_id(),
            session_id: session_id.into(),
            status: GraphStatus::Planning,
            created_at: now_iso(),
            nodes: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn from_plan(plan: &OrchestratorPlan, session_id: &str) -> Self {
        let mut g = TaskGraphModel::new(session_id);
        g.status = GraphStatus::Executing;
        for n in &plan.nodes {
            g.add_node(
                NewNode {
                    agent_identifier: n.agent_identifier.clone(),
                    title: n.title.clone(),
                    objective: n.objective.clone(),
                    acceptance: n.acceptance.clone(),
                    priority: n.priority,
                },
                Some(n.id.clone()),
                &n.depends_on,
            );
        }
        g
    }

    pub fn add_node(&mut self, spec: NewNode, id: Option<String>, depends_on: &[String]) -> String {
        let node_id = id.unwrap_or_else(|| format!("T{}", self.order.len() + 1));
        let node = TaskNode {
            id: node_id.clone(),
            session_id: self.session_id.clone(),
            graph_id: self.id.clone(),
            agent_identifier: spec.agent_identifier,
            title: spec.title,
            objective: spec.objective,
            acceptance: spec.acceptance,
            priority: spec.priority,
            depends_on: depends_on.to_vec(),
            status: TaskStatus::Pending,
            input_refs: vec![],
            sync_report_id: None,
            retry_count: 0,
            idempotency_key: new_id(),
            created_at: now_iso(),
            started_at: None,
            finished_at: None,
        };
        self.nodes.insert(node_id.clone(), node);
        self.order.push(node_id.clone());
        self.refresh_ready();
        node_id
    }

    pub fn get(&self, id: &str) -> Option<&TaskNode> {
        self.nodes.get(id)
    }

    pub fn list(&self) -> Vec<TaskNode> {
        self.order.iter().filter_map(|id| self.nodes.get(id).cloned()).collect()
    }

    pub fn set_status(&mut self, id: &str, status: TaskStatus) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.status = status;
            if status == TaskStatus::Running && n.started_at.is_none() {
                n.started_at = Some(now_iso());
            }
            if status.is_terminal() {
                n.finished_at = Some(now_iso());
            }
        }
        self.refresh_ready();
    }

    pub fn bump_retry(&mut self, id: &str) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.retry_count += 1;
        }
    }

    pub fn set_sync_report(&mut self, id: &str, report_id: &str, input_refs: Vec<String>) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.sync_report_id = Some(report_id.to_string());
            n.input_refs = input_refs;
        }
    }

    /// 依赖全部 done 的 pending 节点转 ready；上游失败/阻断则传播为 blocked
    pub fn refresh_ready(&mut self) {
        let mut changes: Vec<(String, TaskStatus)> = Vec::new();
        for id in &self.order {
            let n = match self.nodes.get(id) {
                Some(n) => n,
                None => continue,
            };
            if n.status != TaskStatus::Pending {
                continue;
            }
            let deps: Vec<Option<TaskStatus>> =
                n.depends_on.iter().map(|d| self.nodes.get(d).map(|x| x.status)).collect();
            if deps.iter().any(|s| {
                matches!(s, Some(TaskStatus::Failed) | Some(TaskStatus::Cancelled))
            }) {
                changes.push((id.clone(), TaskStatus::Blocked));
            } else if deps.iter().all(|s| matches!(s, Some(TaskStatus::Done))) {
                changes.push((id.clone(), TaskStatus::Ready));
            }
        }
        for (id, st) in changes {
            if let Some(n) = self.nodes.get_mut(&id) {
                n.status = st;
                if st == TaskStatus::Blocked {
                    n.finished_at = Some(now_iso());
                }
            }
        }
    }

    pub fn ready_nodes(&self) -> Vec<TaskNode> {
        self.order
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|n| n.status == TaskStatus::Ready)
            .cloned()
            .collect()
    }

    pub fn is_settled(&self) -> bool {
        self.nodes.values().all(|n| n.status.is_terminal())
    }

    pub fn has_failure(&self) -> bool {
        self.nodes.values().any(|n| n.status == TaskStatus::Failed)
    }

    pub fn to_graph(&self) -> TaskGraph {
        let status = if self.is_settled() {
            if self.has_failure() {
                GraphStatus::Aborted
            } else {
                GraphStatus::Converged
            }
        } else {
            self.status
        };
        TaskGraph {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            nodes: self.list(),
            status,
            created_at: self.created_at.clone(),
        }
    }
}

pub type Executor =
    Arc<dyn Fn(TaskNode) -> BoxFuture<'static, NodeOutcome> + Send + Sync + 'static>;

/// 调度器：拓扑序 + 并发池（tokio JoinSet），动态扩图（裁决节点）
pub struct Scheduler {
    max_concurrency: usize,
}

impl Scheduler {
    pub fn new(max_concurrency: usize) -> Self {
        Scheduler { max_concurrency: max_concurrency.max(1) }
    }

    pub async fn run<F>(
        &self,
        graph: &mut TaskGraphModel,
        executor: F,
        mut on_change: impl FnMut(&TaskGraphModel),
    ) where
        F: Fn(TaskNode) -> BoxFuture<'static, NodeOutcome> + Send + Sync + 'static,
    {
        let mut set: JoinSet<NodeOutcome> = JoinSet::new();
        let mut inflight: HashSet<String> = HashSet::new();

        loop {
            // 派发
            while inflight.len() < self.max_concurrency {
                let ready = graph.ready_nodes();
                let Some(node) = ready.into_iter().next() else { break };
                graph.set_status(&node.id, TaskStatus::Dispatched);
                on_change(graph);
                inflight.insert(node.id.clone());
                let fut = executor(node);
                set.spawn(fut);
            }

            if set.is_empty() {
                if graph.is_settled() || graph.ready_nodes().is_empty() {
                    break;
                }
                continue;
            }

            let Some(joined) = set.join_next().await else { break };
            let outcome = match joined {
                Ok(o) => o,
                Err(e) => {
                    // 任务 panic：标记任一 inflight 节点失败（保守处理）
                    if let Some(id) = inflight.iter().next().cloned() {
                        graph.bump_retry(&id);
                        graph.set_status(&id, TaskStatus::Failed);
                        inflight.remove(&id);
                    }
                    eprintln!("[scheduler] 执行任务异常: {e}");
                    on_change(graph);
                    continue;
                }
            };

            inflight.remove(&outcome.node_id);
            match outcome.status {
                TaskStatus::Failed => {
                    graph.bump_retry(&outcome.node_id);
                    graph.set_status(&outcome.node_id, TaskStatus::Failed);
                    if let Some(err) = &outcome.error {
                        eprintln!("[scheduler] 节点 {} 失败: {}", outcome.node_id, err);
                    }
                }
                TaskStatus::Blocked => graph.set_status(&outcome.node_id, TaskStatus::Blocked),
                _ => graph.set_status(&outcome.node_id, TaskStatus::Done),
            }

            // 动态扩图（裁决节点等）
            for extra in outcome.appends {
                graph.add_node(
                    extra,
                    None,
                    std::slice::from_ref(&outcome.node_id),
                );
            }
            on_change(graph);

            if graph.is_settled() && inflight.is_empty() && set.is_empty() {
                break;
            }
        }

        // 收尾：等待剩余任务（不应发生）
        while set.join_next().await.is_some() {}
    }
}
