//! 通道身份管理（控制台侧）：配对码签发 / 绑定清单 / 角色调整。
//! 全部经后台鉴权中间件（auth_key）保护——能调这些端点即管理员。
use axum::{extract::{Path, State}, Json, Router, http::StatusCode, response::{IntoResponse, Response}, routing::{get, post}};
use serde_json::{Value, json};

use crate::AppState;

/// GET /api/identity/list：绑定清单 + 未过期配对码
async fn list(State(st): State<AppState>) -> Response {
    let now = exm_core::types::now_ms();
    let identities: Vec<Value> = st
        .core
        .identities()
        .iter()
        .map(|i| {
            json!({
                "id": i.id, "channel": i.channel, "externalId": i.external_id,
                "displayName": i.display_name, "role": i.role, "pairedAt": i.paired_at, "note": i.note,
            })
        })
        .collect();
    let codes: Vec<Value> = st
        .core
        .pairing_codes()
        .iter()
        .map(|c| {
            json!({
                "code": c.code, "note": c.note, "platform": c.channel_platform,
                "createdAt": c.created_at, "expiresAtMs": c.expires_at_ms,
                "expired": now > c.expires_at_ms,
            })
        })
        .collect();
    Json(json!({
        "identityRequired": st.core.config().identity.identity_required,
        "identities": identities,
        "codes": codes,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub struct PairingBody {
    #[serde(default)]
    pub note: Option<String>,
    /// 限定平台（通道账号 id 前缀；空 = 通用）
    #[serde(default)]
    pub platform: Option<String>,
    /// 有效期（秒）；缺省取全局配置
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

/// POST /api/identity/code：签发配对码
async fn issue(State(st): State<AppState>, Json(b): Json<PairingBody>) -> Response {
    let ttl = b.ttl_secs.unwrap_or(st.core.config().identity.pairing_ttl_secs).clamp(60, 86400);
    match st.core.issue_pairing_code(b.note.as_deref().unwrap_or(""), b.platform.as_deref(), ttl) {
        Ok(c) => Json(json!({
            "code": c.code, "expiresAtMs": c.expires_at_ms,
            "hint": format!("让用户在通道内发送 /pair {}", c.code),
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// DELETE /api/identity/:id：解绑
async fn unbind(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.core.drop_identity(&id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct RoleBody {
    pub id: String,
    /// admin | member
    pub role: String,
}

/// POST /api/identity/role：调整角色
async fn set_role(State(st): State<AppState>, Json(b): Json<RoleBody>) -> Response {
    if !matches!(b.role.as_str(), "admin" | "member") {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "role 仅支持 admin | member" }))).into_response();
    }
    let Some(mut idn) = st.core.identity_of_id(&b.id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "绑定不存在" }))).into_response();
    };
    idn.role = b.role;
    match st.core.save_identity(&idn) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/identity/list", get(list))
        .route("/api/identity/code", post(issue))
        .route("/api/identity/role", post(set_role))
        .route("/api/identity/:id", axum::routing::delete(unbind))
}
