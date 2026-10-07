//! 用量治理（capability-completion 组 4）：滑动窗口限流 + 周期配额计数。
//! 仅作用于外部入口（通道入站 / HTTP API）；组内派发不计费不限流（设计 D10）。
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

#[derive(Default)]
pub struct RateLimiter {
    /// key → 窗口内命中时间戳（毫秒）
    windows: Mutex<HashMap<String, Vec<u64>>>,
}

fn store() -> &'static RateLimiter {
    static L: OnceLock<RateLimiter> = OnceLock::new();
    L.get_or_init(RateLimiter::default)
}

impl RateLimiter {
    /// 记录一次命中并判定是否放行（滑动窗口：清理过期时间戳后计数 < max 则放行）
    pub fn check(key: &str, window_ms: u64, max: u32, now_ms: u64) -> bool {
        let store = store();
        let mut w = store.windows.lock().unwrap();
        let entry = w.entry(key.to_string()).or_default();
        entry.retain(|t| now_ms.saturating_sub(*t) < window_ms);
        if entry.len() >= max as usize {
            return false;
        }
        entry.push(now_ms);
        true
    }

    /// 主体在窗口内的当前计数（控制台可见性）
    pub fn count(key: &str, window_ms: u64, now_ms: u64) -> usize {
        let store = store();
        let w = store.windows.lock().unwrap();
        w.get(key)
            .map(|v| v.iter().filter(|t| now_ms.saturating_sub(**t) < window_ms).count())
            .unwrap_or(0)
    }

    /// 全部键的窗口内计数（管理视图）
    pub fn snapshot(window_ms: u64, now_ms: u64) -> Vec<(String, usize)> {
        let store = store();
        let w = store.windows.lock().unwrap();
        let mut out: Vec<(String, usize)> = w
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.iter().filter(|t| now_ms.saturating_sub(**t) < window_ms).count(),
                )
            })
            .filter(|(_, c)| *c > 0)
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }
}

/// 周期配额键：`quota:{subject}:{周期起始日}`（按日重置；weekly/monthly 以窗口长度近似为 7d/30d 滚动窗）
pub fn quota_key(subject: &str, period: &str, now_ms: u64) -> (String, u64) {
    let window_ms: u64 = match period {
        "weekly" => 7 * 86400 * 1000,
        "monthly" => 30 * 86400 * 1000,
        _ => 86400 * 1000, // daily（默认）
    };
    let day = now_ms / window_ms;
    (format!("quota:{subject}:{day}"), window_ms)
}

#[cfg(test)]
mod limits_tests {
    use super::*;

    #[test]
    fn 滑动窗口_超频拒绝与恢复() {
        let now = 1_000_000u64;
        assert!(RateLimiter::check("k", 1000, 2, now));
        assert!(RateLimiter::check("k", 1000, 2, now + 100));
        assert!(!RateLimiter::check("k", 1000, 2, now + 200), "窗口内第 3 次应拒绝");
        assert_eq!(RateLimiter::count("k", 1000, now + 300), 2);
        // 窗口滑过：最早命中的两条过期后恢复
        assert!(RateLimiter::check("k", 1000, 2, now + 1101));
    }

    #[test]
    fn 维度键相互独立() {
        let now = 2_000_000u64;
        assert!(RateLimiter::check("ch:a:u1", 1000, 1, now));
        assert!(!RateLimiter::check("ch:a:u1", 1000, 1, now + 10));
        assert!(RateLimiter::check("ch:a:u2", 1000, 1, now + 20), "其他主体不受影响");
        assert!(RateLimiter::check("api", 1000, 1, now + 30), "其他维度不受影响");
    }

    #[test]
    fn 配额键_按周期分桶() {
        let (k1, w1) = quota_key("user:u1", "daily", 1000);
        let (k2, _) = quota_key("user:u1", "daily", 1000 + 86400 * 1000);
        assert_ne!(k1, k2, "跨日应换桶");
        assert_eq!(w1, 86400 * 1000);
        let (k3, w3) = quota_key("user:u1", "weekly", 1000);
        assert_eq!(w3, 7 * 86400 * 1000);
        assert!(k3.starts_with("quota:user:u1:"));
    }
}

// ---------------------------------------------------------------- 管理视图（4.4）

use axum::{extract::State, response::{IntoResponse, Response}, routing::get, Json, Router};
use serde_json::{Value, json};

use crate::AppState;

/// GET /api/limits/stats：限流计数快照 + 配置口径（管理可见性）
pub async fn stats(State(st): State<AppState>) -> Response {
    let limits = st.core.config().limits.clone();
    let now = exm_core::types::now_ms();
    let window_ms = limits.window_secs * 1000;
    let counters: Vec<Value> = crate::limits::RateLimiter::snapshot(window_ms.max(1000), now)
        .into_iter()
        .map(|(k, c)| json!({ "key": k, "count": c }))
        .collect();
    Json(json!({
        "enabled": limits.enabled,
        "windowSecs": limits.window_secs,
        "maxRequests": limits.max_requests,
        "quotaPeriod": limits.quota_period,
        "quotaTokens": limits.quota_tokens,
        "quotaRequests": limits.quota_requests,
        "counters": counters,
    }))
    .into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/limits/stats", get(stats))
}
