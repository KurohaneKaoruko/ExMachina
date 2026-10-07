//! 图片附件暂存（多模态输入）：渠道收到图片 → 暂存 → 指挥体下一轮规划取走。
//! 进程级单例：网关为单 Core 进程模型；CLI/测试同样按会话存取。

use std::collections::HashMap;
use std::sync::OnceLock;

fn stash() -> &'static parking_lot::Mutex<HashMap<String, Vec<String>>> {
    static STASH: OnceLock<parking_lot::Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    STASH.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// 暂存图片（data URL），追加到该会话的待注入队列
pub fn stage(session_id: &str, images: Vec<String>) {
    if images.is_empty() {
        return;
    }
    stash()
        .lock()
        .entry(session_id.to_string())
        .or_default()
        .extend(images);
}

/// 取走该会话全部暂存图片（每轮取走即清）
pub fn take(session_id: &str) -> Vec<String> {
    stash().lock().remove(session_id).unwrap_or_default()
}
