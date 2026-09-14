//! 工作者节点池（分布式执行）：/worker WebSocket 接入 + 派发路由。
//! 在线工作者承接子个体派发（token 流即时回推事件总线）；失败/超时由 Orchestrator 回落本地。

use crate::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use exm_core::remote::{RemoteExecutor, WorkerFrame};
use exm_core::types::{AgentDefinition, DispatchOrder, SyncReport};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// 在线工作者：出站通道 + 承接清单（空 = 承接全部个体）
struct WorkerConn {
    out: mpsc::UnboundedSender<Message>,
    agents: Vec<String>,
}

/// 待回流派发
struct Pending {
    report_tx: oneshot::Sender<Result<SyncReport, String>>,
    token_tx: mpsc::UnboundedSender<String>,
    session_id: String,
    agent_id: String,
}

pub struct WorkerHub {
    workers: parking_lot::Mutex<HashMap<String, WorkerConn>>,
    pending: parking_lot::Mutex<HashMap<String, Pending>>,
    events: tokio::sync::broadcast::Sender<exm_core::types::CoreEvent>,
}

#[derive(Deserialize)]
struct WorkerQuery {
    #[serde(default)]
    key: Option<String>,
}

pub async fn worker_ws(
    ws: WebSocketUpgrade,
    Query(q): Query<WorkerQuery>,
    State(st): State<AppState>,
) -> impl IntoResponse {
    let required = !st.core.config().security.auth_key.is_empty();
    if required && q.key.as_deref() != Some(st.core.config().security.auth_key.as_str()) {
        return (StatusCode::UNAUTHORIZED, "需要工作者密钥").into_response();
    }
    ws.on_upgrade(move |socket| worker_loop(socket, st))
}

async fn worker_loop(mut socket: WebSocket, st: AppState) {
    let hub = worker_hub_of(&st);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let mut my_id: Option<String> = None;

    loop {
        tokio::select! {
            out = out_rx.recv() => {
                match out {
                    Some(m) => { if socket.send(m).await.is_err() { break; } }
                    None => break,
                }
            }
            msg = socket.recv() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => continue,
                };
                let frame: WorkerFrame = match serde_json::from_str(&text) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                match frame {
                    WorkerFrame::Hello { id, agents } => {
                        hub.workers.lock().insert(
                            id.clone(),
                            WorkerConn { out: out_tx.clone(), agents },
                        );
                        eprintln!("[worker-hub] 工作者接入：{id}");
                        my_id = Some(id);
                    }
                    WorkerFrame::Tokens { did, delta } => {
                        let evt = {
                            let mut pending = hub.pending.lock();
                            pending.get(&did).map(|p| {
                                let _ = p.token_tx.send(delta.clone());
                                exm_core::types::CoreEvent {
                                    kind: "unit.token".into(),
                                    session_id: p.session_id.clone(),
                                    payload: serde_json::json!({ "agentId": p.agent_id, "delta": delta }),
                                }
                            })
                        };
                        if let Some(evt) = evt {
                            let _ = hub.events.send(evt);
                        }
                    }
                    WorkerFrame::Report { did, report } => {
                        if let Some(p) = hub.pending.lock().remove(&did) {
                            let _ = p.report_tx.send(Ok(report));
                        }
                    }
                    WorkerFrame::Error { did, message } => {
                        if let Some(p) = hub.pending.lock().remove(&did) {
                            let _ = p.report_tx.send(Err(message));
                        }
                    }
                    WorkerFrame::Dispatch { .. } => {}
                }
            }
        }
    }
    if let Some(id) = my_id {
        hub.workers.lock().remove(&id);
        eprintln!("[worker-hub] 工作者离线：{id}");
    }
}

/// 池：进程级单例（网关单实例语义；events 从 Core 取）
fn worker_hub_of(st: &AppState) -> &'static Arc<WorkerHub> {
    static HUB: std::sync::OnceLock<Arc<WorkerHub>> = std::sync::OnceLock::new();
    HUB.get_or_init(|| {
        Arc::new(WorkerHub {
            workers: parking_lot::Mutex::new(HashMap::new()),
            pending: parking_lot::Mutex::new(HashMap::new()),
            events: st.core.events.clone(),
        })
    })
}

/// 取池单例（serve 时注入 Core 用）
pub fn hub(st: &AppState) -> Arc<WorkerHub> {
    worker_hub_of(st).clone()
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new().route("/worker", axum::routing::get(worker_ws))
}

#[async_trait::async_trait]
impl RemoteExecutor for WorkerHub {
    fn accepts(&self, def: &AgentDefinition) -> bool {
        self.workers
            .lock()
            .values()
            .any(|w| w.agents.is_empty() || w.agents.contains(&def.identifier))
    }

    async fn execute(
        &self,
        session_id: &str,
        def: &AgentDefinition,
        order: &DispatchOrder,
        on_token: &(dyn Fn(String) + Send + Sync),
    ) -> anyhow::Result<SyncReport> {
        // 挑一个承接该个体的在线工作者（v1：首个；后续可负载分派）
        let out = {
            let workers = self.workers.lock();
            workers
                .values()
                .find(|w| w.agents.is_empty() || w.agents.contains(&def.identifier))
                .map(|w| w.out.clone())
        };
        let Some(out) = out else { anyhow::bail!("无在线工作者") };
        let did = format!("d{}", &exm_core::types::new_id()[..10]);
        let (report_tx, report_rx) = oneshot::channel();
        let (token_tx, mut token_rx) = mpsc::unbounded_channel::<String>();
        self.pending.lock().insert(
            did.clone(),
            Pending {
                report_tx,
                token_tx,
                session_id: session_id.to_string(),
                agent_id: def.identifier.clone(),
            },
        );
        let frame = serde_json::to_string(&WorkerFrame::Dispatch {
            did: did.clone(),
            def: def.clone(),
            order: order.clone(),
        })?;
        out.send(Message::Text(frame)).map_err(|e| anyhow::anyhow!("工作者连接已断: {e}"))?;

        // 超时 = 派发约束；期间 token 泵即时回调（report 就绪前持续消费）
        let timeout = std::time::Duration::from_millis(order.constraints.timeout_ms.max(5_000).into());
        let mut report_rx = std::pin::pin!(report_rx);
        let result = tokio::time::timeout(timeout, async {
            loop {
                tokio::select! {
                    biased;
                    r = &mut report_rx => break r,
                    d = token_rx.recv() => {
                        if let Some(delta) = d {
                            on_token(delta);
                        }
                    }
                }
            }
        })
        .await;
        self.pending.lock().remove(&did);
        match result {
            Ok(Ok(Ok(report))) => Ok(report),
            Ok(Ok(Err(msg))) => anyhow::bail!("工作者执行失败: {msg}"),
            Ok(Err(_)) => anyhow::bail!("工作者连接中断"),
            Err(_) => anyhow::bail!("工作者执行超时"),
        }
    }
}
