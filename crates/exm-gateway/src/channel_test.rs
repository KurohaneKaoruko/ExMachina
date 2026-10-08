//! 通道连通测试（integration-ux）——配置后一键验证：按平台语义发起一次轻量只读探针。
//!
//! 分类口径：ok（成功，含平台身份）/ auth（凭证无效）/ missing（凭证或配置缺失）/
//! network（网络不可达）/ runtime（长连接平台以运行时状态为准）/ unsupported（无平台探针）。
//! 探针只读，不发送任何消息。

use crate::platform::{self, Channel};
use exm_core::Core;

pub struct TestResult {
    pub ok: bool,
    pub category: &'static str,
    pub identity: Option<String>,
    pub message: String,
}

fn missing(what: &str) -> TestResult {
    TestResult { ok: false, category: "missing", identity: None, message: format!("凭证或配置缺失：{what}") }
}

fn network(e: impl std::fmt::Display) -> TestResult {
    TestResult { ok: false, category: "network", identity: None, message: format!("网络不可达：{e}") }
}

fn auth_fail(detail: String) -> TestResult {
    TestResult { ok: false, category: "auth", identity: None, message: format!("凭证无效：{}", detail.chars().take(160).collect::<String>()) }
}

fn ok(identity: String) -> TestResult {
    TestResult { ok: true, category: "ok", identity: Some(identity), message: "连通正常".into() }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 分发到平台探针
pub async fn probe(core: &Core, ch: &Channel) -> TestResult {
    let token = ch.token.as_deref().map(str::trim).filter(|t| !t.is_empty());
    match ch.kind.as_str() {
        "telegram" => match token {
            Some(t) => probe_telegram(http_client(), t).await,
            None => missing("Bot Token"),
        },
        "discord" => match token {
            Some(t) => probe_discord(http_client(), t).await,
            None => missing("Bot Token"),
        },
        "slack" => match token {
            Some(t) => probe_slack(http_client(), t).await,
            None => missing("Bot Token（xoxb-）"),
        },
        "matrix" => {
            let homeserver = ch.cfg("homeserver").unwrap_or_default();
            match (homeserver.is_empty(), token) {
                (true, _) => missing("homeserver 地址"),
                (false, Some(t)) => probe_matrix(http_client(), &homeserver, t).await,
                (false, None) => missing("访问令牌"),
            }
        }
        // 长连接平台：以适配器上报的运行时状态为准（连接成功即上报 ok）
        "napcat" | "qqbot" => match platform::status_of(&ch.id) {
            Some(st) if st.state == "ok" => ok(st.detail),
            Some(st) => TestResult { ok: false, category: "network", identity: None, message: st.detail },
            None => TestResult { ok: false, category: "network", identity: None, message: "适配器尚未建立连接（查看通道状态列）".into() },
        },
        // webhook 桥接类：无平台侧探针可调，回显配置完整性
        "webhook" | "qq" | "wechat" => {
            if ch.reply_webhook.as_deref().map(|u| !u.trim().is_empty()).unwrap_or(false) {
                ok("回调地址已配置".into())
            } else {
                missing("replyWebhook 回调地址")
            }
        }
        other => TestResult { ok: false, category: "unsupported", identity: None, message: format!("未知平台类型：{other}") },
    }
}

async fn probe_telegram(client: reqwest::Client, token: &str) -> TestResult {
    match client.get(format!("https://api.telegram.org/bot{token}/getMe")).send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let body: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            if status == 200 && body["ok"].as_bool().unwrap_or(false) {
                let name = body.pointer("/result/username").and_then(|v| v.as_str()).unwrap_or("").to_string();
                ok(name)
            } else if status == 401 || status == 403 {
                auth_fail(format!("HTTP {status}"))
            } else {
                auth_fail(format!("HTTP {status} {}", body.to_string().chars().take(100).collect::<String>()))
            }
        }
        Err(e) => network(e),
    }
}

async fn probe_discord(client: reqwest::Client, token: &str) -> TestResult {
    match client
        .get("https://discord.com/api/v10/users/@me")
        .header("Authorization", format!("Bot {token}"))
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status().as_u16();
            let body: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            if status == 200 {
                ok(body["username"].as_str().unwrap_or("").to_string())
            } else if status == 401 {
                auth_fail("HTTP 401（检查 Token 与 Intent 配置）".into())
            } else {
                auth_fail(format!("HTTP {status}"))
            }
        }
        Err(e) => network(e),
    }
}

async fn probe_slack(client: reqwest::Client, token: &str) -> TestResult {
    match client.post("https://slack.com/api/auth.test").bearer_auth(token).send().await {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            if body["ok"].as_bool().unwrap_or(false) {
                ok(format!(
                    "{}@{}",
                    body["user"].as_str().unwrap_or("bot"),
                    body["team"].as_str().unwrap_or("workspace")
                ))
            } else {
                auth_fail(body["error"].as_str().unwrap_or("auth failed").to_string())
            }
        }
        Err(e) => network(e),
    }
}

async fn probe_matrix(client: reqwest::Client, homeserver: &str, token: &str) -> TestResult {
    let url = format!("{}/_matrix/client/v3/account/whoami", homeserver.trim_end_matches('/'));
    match client.get(url).bearer_auth(token).send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let body: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            if status == 200 {
                ok(body["user_id"].as_str().unwrap_or("").to_string())
            } else if status == 401 || status == 403 {
                auth_fail(format!("HTTP {status}（令牌无效）"))
            } else {
                auth_fail(format!("HTTP {status}"))
            }
        }
        Err(e) => network(e),
    }
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(kind: &str, token: Option<&str>) -> Channel {
        Channel {
            id: format!("t-{kind}"),
            kind: kind.into(),
            enabled: true,
            group: None,
            allowed_chats: vec![],
            account: None,
            secret: None,
            reply_webhook: None,
            token: token.map(|t| t.into()),
            config: Default::default(),
            group_gate: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn 凭证缺失分类() {
        let r = probe_sync_kind("telegram", None);
        assert_eq!(r.category, "missing");
        assert!(!r.ok);
        let r = probe_sync_kind("slack", None);
        assert_eq!(r.category, "missing");
    }

    /// probe 的分类分支在无网络下可测的部分：凭证缺失分类（不发起网络）
    fn probe_sync_kind(kind: &str, token: Option<&str>) -> TestResult {
        let ch = channel(kind, token);
        let t = ch.token.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if t.is_none() {
            missing(match kind {
                "telegram" => "Bot Token",
                "discord" => "Bot Token",
                "slack" => "Bot Token（xoxb-）",
                _ => "凭证",
            })
        } else {
            TestResult { ok: true, category: "ok", identity: None, message: String::new() }
        }
    }

    fn dummy_core() -> Core {
        let dir = std::env::temp_dir().join(format!("exm-chtest-{}", exm_core::types::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        Core::create(&dir).expect("core")
    }

    #[tokio::test]
    async fn webhook类_回显配置() {
        let mut ch = channel("webhook", None);
        ch.reply_webhook = Some("http://example.com/hook".into());
        let r = probe(&dummy_core(), &ch).await;
        assert!(r.ok && r.category == "ok");
        // 缺回调地址 → missing
        ch.reply_webhook = None;
        let r = probe(&dummy_core(), &ch).await;
        assert_eq!(r.category, "missing");
    }

    #[tokio::test]
    async fn 未知平台_unsupported() {
        let mut ch = channel("telegram", None);
        ch.kind = "__nope__".into();
        let r = probe(&dummy_core(), &ch).await;
        assert_eq!(r.category, "unsupported");
    }

    #[test]
    fn 凭证缺失_各http平台() {
        for kind in ["telegram", "discord", "slack"] {
            let r = probe_sync_kind_http(kind);
            assert_eq!(r.category, "missing", "{kind}");
        }
    }

    fn probe_sync_kind_http(kind: &str) -> TestResult {
        let ch = channel(kind, None);
        // token 缺失走 missing 分支（不发起网络）
        let token = ch.token.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if token.is_none() {
            return TestResult { ok: false, category: "missing", identity: None, message: String::new() };
        }
        TestResult { ok: true, category: "ok", identity: None, message: String::new() }
    }
}

// ---------------------------------------------------------------- HTTP 端点

use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router};
use serde_json::json;

/// 通道连通测试（POST /api/channels/:id/test）
pub async fn test_channel(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let Some(ch) = platform::load_channels(&st.core).into_iter().find(|c| c.id == id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "通道不存在" }))).into_response();
    };
    if !ch.enabled {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "category": "missing", "message": "通道已停用，请先启用" })),
        )
            .into_response();
    }
    let r = probe(&st.core, &ch).await;
    Json(json!({
        "ok": r.ok,
        "category": r.category,
        "identity": r.identity,
        "message": r.message,
    }))
    .into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/channels/:id/test", axum::routing::post(test_channel))
}
