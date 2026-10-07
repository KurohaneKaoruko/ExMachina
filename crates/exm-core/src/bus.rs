//! 全连结消息总线 —— docs/01 §2.4（集群兼容核心）
//! 单体实现 InProcessBus（tokio broadcast）；分布式实现替换本 trait 即可，业务零改动。

use crate::types::Envelope;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot, Mutex};

#[async_trait]
pub trait MessageBus: Send + Sync {
    async fn publish(&self, topic: &str, msg: Envelope) -> anyhow::Result<()>;
    fn subscribe(&self, topic: &str) -> broadcast::Receiver<Envelope>;
    async fn request(&self, topic: &str, msg: Envelope, timeout_ms: u64) -> anyhow::Result<Envelope>;
}

pub struct InProcessBus {
    tx: broadcast::Sender<Envelope>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Envelope>>>>,
}

impl InProcessBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(4096);
        InProcessBus { tx, pending: Arc::new(Mutex::new(HashMap::new())) }
    }
}

impl Default for InProcessBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MessageBus for InProcessBus {
    async fn publish(&self, topic: &str, msg: Envelope) -> anyhow::Result<()> {
        // response 消息直接唤醒等待中的 request
        if msg.kind == "response" {
            let mut pending = self.pending.lock().await;
            if let Some(tx) = pending.remove(&msg.id) {
                let _ = tx.send(msg);
                return Ok(());
            }
        }
        let _ = topic;
        let _ = self.tx.send(msg);
        Ok(())
    }

    fn subscribe(&self, topic: &str) -> broadcast::Receiver<Envelope> {
        // 全量扇出；调用侧按 `Envelope.topic` 过滤（与分布式实现语义一致）
        let _ = topic;
        self.tx.subscribe()
    }

    async fn request(
        &self,
        topic: &str,
        msg: Envelope,
        timeout_ms: u64,
    ) -> anyhow::Result<Envelope> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(msg.id.clone(), tx);
        self.publish(topic, msg.clone()).await?;
        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(anyhow::anyhow!("bus request 通道关闭: {topic}")),
            Err(_) => {
                self.pending.lock().await.remove(&msg.id);
                Err(anyhow::anyhow!("bus request 超时: {topic}"))
            }
        }
    }
}

/// 全量扇出接收器：按 topic 前缀过滤（供 WS/CLI 使用）
pub struct TopicFilter {
    topic_prefix: String,
}

impl TopicFilter {
    pub fn new(prefix: impl Into<String>) -> Self {
        TopicFilter { topic_prefix: prefix.into() }
    }
    pub fn matches(&self, topic: &str) -> bool {
        self.topic_prefix.is_empty() || topic.starts_with(&self.topic_prefix)
    }
}

/// RedisBus 兼容桩（T-060）：证明接口可被分布式实现承载。
/// 启用时安装 redis 依赖并按注释实现，上层业务代码零改动。
pub struct RedisBusStub {
    pub url: String,
    pub namespace: String,
}

impl RedisBusStub {
    pub fn new(url: impl Into<String>) -> Self {
        RedisBusStub { url: url.into(), namespace: "exmachina".into() }
    }
    fn channel(&self, topic: &str) -> String {
        format!("{}:{}", self.namespace, topic)
    }
}

#[async_trait]
impl MessageBus for RedisBusStub {
    async fn publish(&self, topic: &str, _msg: Envelope) -> anyhow::Result<()> {
        // 实现要点：PUBLISH self.channel(topic) <json>；response 走 RPUSH resp:{id}
        anyhow::bail!(
            "RedisBus 为兼容桩（channel={}，url={}）：启用时安装 redis crate 并实现 publish/subscribe/request，接口已冻结",
            self.channel(topic),
            self.url
        )
    }

    fn subscribe(&self, _topic: &str) -> broadcast::Receiver<Envelope> {
        // 实现要点：独立连接 SUBSCRIBE channel，解析后转发到本地 broadcast
        let (tx, rx) = broadcast::channel(1);
        drop(tx);
        rx
    }

    async fn request(
        &self,
        topic: &str,
        _msg: Envelope,
        _timeout_ms: u64,
    ) -> anyhow::Result<Envelope> {
        anyhow::bail!("RedisBus 为兼容桩（channel={}）", self.channel(topic))
    }
}
