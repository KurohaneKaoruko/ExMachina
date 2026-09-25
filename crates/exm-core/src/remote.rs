//! 分布式执行节点（docs/架构与设计.md 扩展点）：
//! 淘汰设备 / 迷你主机作为远程工作者接入网关，承接子个体派发。
//! 协议（WebSocket，JSON 帧）：
//!   worker → hub ：{"type":"hello","id","agents":[]}（agents 空 = 承接全部个体）
//!   hub → worker ：{"type":"dispatch","did","def","order"}
//!   worker → hub ：{"type":"tokens","did","delta"} / {"type":"report","did","report"} / {"type":"error","did","message"}
//! 失败或超时自动回落本地执行——集群是加速器，不是单点。

use crate::types::{AgentDefinition, DispatchOrder, SyncReport};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 远程执行器：网关侧的派发路由（由 worker 池实现；Orchestrator 持有）
#[async_trait]
pub trait RemoteExecutor: Send + Sync {
    /// 是否有在线工作者承接该个体（注册表查询，同步即答）
    fn accepts(&self, def: &AgentDefinition) -> bool;
    /// 远程执行：成功返回 SyncReport；Err = 回落本地
    async fn execute(
        &self,
        session_id: &str,
        def: &AgentDefinition,
        order: &DispatchOrder,
        on_token: &(dyn Fn(String) + Send + Sync),
    ) -> anyhow::Result<SyncReport>;
}

// ---------------------------------------------------------------- 协议帧

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerFrame {
    Hello { id: String, agents: Vec<String> },
    Dispatch { did: String, def: AgentDefinition, order: DispatchOrder },
    Tokens { did: String, delta: String },
    Report { did: String, report: SyncReport },
    Error { did: String, message: String },
}

/// 工作者会话（客户端）：连接网关 /worker，收派发 → 本地执行 → 回流
pub struct WorkerSession;

impl WorkerSession {
    /// 运行工作者主循环：url 如 ws://hub:4173/worker；token 即网关 authKey（未配置则空）
    pub async fn run(
        url: &str,
        token: &str,
        worker_id: &str,
        core: std::sync::Arc<crate::Core>,
    ) -> anyhow::Result<()> {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message;

        let full = if token.is_empty() { url.to_string() } else { format!("{url}?key={token}") };
        let (ws, _) = tokio_tungstenite::connect_async(full).await?;
        let (sink, mut stream) = ws.split();
        // 发送通道：读循环与执行任务共用（写端由独立任务独占）
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            use futures_util::SinkExt;
            let mut sink = sink;
            let mut out_rx = out_rx;
            while let Some(msg) = out_rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });
        let _ = out_tx.send(Message::Text(
            serde_json::to_string(&WorkerFrame::Hello { id: worker_id.to_string(), agents: vec![] })?,
        ));
        eprintln!("[worker:{worker_id}] 已接入 {url}，等待派发（Ctrl+C 退出）");
        while let Some(msg) = stream.next().await {
            let text = match msg {
                Ok(Message::Text(t)) => t,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            let frame: WorkerFrame = match serde_json::from_str(&text) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let WorkerFrame::Dispatch { did, def, order } = frame else { continue };
            let out = out_tx.clone();
            let core = core.clone();
            tokio::spawn(async move {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                // 令牌回传泵：执行流 → 网关
                let pump = {
                    let did = did.clone();
                    let out = out.clone();
                    tokio::spawn(async move {
                        while let Some(delta) = rx.recv().await {
                            if let Ok(t) = serde_json::to_string(&WorkerFrame::Tokens { did: did.clone(), delta }) {
                                if out.send(Message::Text(t)).is_err() {
                                    break;
                                }
                            }
                        }
                    })
                };
                let result = core
                    .orchestrator()
                    .execute_unit(&def, &order, "", move |delta| {
                        let _ = tx.send(delta.to_string());
                    })
                    .await;
                pump.abort();
                let frame = match result {
                    Ok(report) => WorkerFrame::Report { did, report },
                    Err(e) => WorkerFrame::Error { did, message: e.to_string() },
                };
                if let Ok(t) = serde_json::to_string(&frame) {
                    let _ = out.send(Message::Text(t));
                }
            });
        }
        Ok(())
    }
}
