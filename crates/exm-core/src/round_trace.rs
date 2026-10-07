//! 轮次过程轨迹暂存 —— 思维链与工具调用的会话级收集器
//!
//! 服务端在轮次进行中收集两类过程数据：
//! - 思维链增量（orchestrator 的 Thinking 流）
//! - 工具调用记录（每次 execute_named 完成后的摘要）
//!
//! 消息收束（最终 orchestrator 消息落盘前）由 `drain` 一次性取走并随消息持久化——
//! 这是「过程收起栏」在刷新/换端后仍然可回看的数据来源（process-transparency 能力）。
//! 另含会话级取消令牌：停止轮次 = 置位令牌，工具循环检查点轮询。

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 单次工具调用记录（与前端 ToolCallItem 契约一致，camelCase 序列化）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    pub call_id: String,
    pub agent_id: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub ok: bool,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub summary: String,
}

#[derive(Default)]
struct RoundState {
    thinking: String,
    tools: Vec<ToolCallRecord>,
    cancelled: Arc<AtomicBool>,
}

fn states() -> &'static Mutex<HashMap<String, RoundState>> {
    static S: std::sync::OnceLock<Mutex<HashMap<String, RoundState>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 追加思维链增量
pub fn push_thinking(session_id: &str, delta: &str) {
    if delta.is_empty() {
        return;
    }
    states()
        .lock()
        .entry(session_id.to_string())
        .or_default()
        .thinking
        .push_str(delta);
}

/// 记录一次完成的工具调用
pub fn push_tool(session_id: &str, record: ToolCallRecord) {
    states().lock().entry(session_id.to_string()).or_default().tools.push(record);
}

/// 轮次收束时取走全部过程数据（取走即清空该类数据，取消令牌不受影响）
pub fn drain(session_id: &str) -> (Option<String>, Vec<ToolCallRecord>) {
    let mut map = states().lock();
    let Some(st) = map.get_mut(session_id) else { return (None, Vec::new()) };
    let thinking = (!st.thinking.trim().is_empty()).then(|| std::mem::take(&mut st.thinking));
    let tools = std::mem::take(&mut st.tools);
    (thinking, tools)
}

/// 会话运行标志：chat 开始时置存在、结束移除（存在 = 有运行中轮次）
pub fn begin(session_id: &str) {
    states().lock().entry(session_id.to_string()).or_default();
}

/// 会话运行结束：移除整个轮次状态（含取消令牌）
pub fn end(session_id: &str) {
    states().lock().remove(session_id);
}

/// 会话是否有运行中轮次
pub fn is_running(session_id: &str) -> bool {
    states().lock().contains_key(session_id)
}

/// 请求停止：置位取消令牌；返回会话是否有运行中轮次
pub fn request_cancel(session_id: &str) -> bool {
    let mut map = states().lock();
    match map.get_mut(session_id) {
        Some(st) => {
            st.cancelled.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// 检查点：会话是否已被请求停止
pub fn is_cancelled(session_id: &str) -> bool {
    states()
        .lock()
        .get(session_id)
        .map(|st| st.cancelled.load(Ordering::Relaxed))
        .unwrap_or(false)
}

/// 本轮已记录的工具调用数
pub fn tool_count(session_id: &str) -> usize {
    states().lock().get(session_id).map(|st| st.tools.len()).unwrap_or(0)
}
