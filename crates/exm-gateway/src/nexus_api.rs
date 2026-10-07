//! 智能连结网络管理端点（实验性）——连结体清单 CRUD。
//! 存储于 config.nexus（apply_config 热生效），路由 /api/nexus/links。

use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use exm_core::config::{ExmConfig, LinkTarget};
use serde::Deserialize;
use serde_json::{json, Value};

fn links_of(st: &AppState) -> Vec<LinkTarget> {
    st.core.config().nexus.links.clone()
}

/// 连结体清单（GET /api/nexus/links）
pub async fn list_links(State(st): State<AppState>) -> impl IntoResponse {
    Json(json!({ "links": links_of(&st) })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkBody {
    pub id: String,
    pub name: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub command: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_kind() -> String {
    "exmachina".into()
}
fn default_enabled() -> bool {
    true
}

fn validate(body: &LinkBody) -> Result<(), String> {
    let id = body.id.trim();
    if id.is_empty() || id.len() > 48 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("id 须为 1-48 位字母/数字/-/_".into());
    }
    if body.name.trim().is_empty() {
        return Err("name 不能为空".into());
    }
    if !matches!(body.kind.as_str(), "exmachina" | "opencode" | "codex" | "claude" | "custom") {
        return Err(format!("未知连结体类型: {}", body.kind));
    }
    if body.kind == "exmachina" && body.endpoint.trim().is_empty() {
        return Err("exmachina 连结体须配置网关地址".into());
    }
    if body.kind == "custom" && body.command.trim().is_empty() {
        return Err("custom 连结体须配置命令行".into());
    }
    Ok(())
}

/// 新增/更新连结体（PUT /api/nexus/links）：直接改配置并热生效
pub async fn save_link(State(st): State<AppState>, Json(b): Json<LinkBody>) -> impl IntoResponse {
    if let Err(e) = validate(&b) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    }
    let mut cfg: ExmConfig = (*st.core.config()).clone();
    let link = LinkTarget {
        id: b.id.trim().to_string(),
        name: b.name.trim().to_string(),
        kind: b.kind.clone(),
        endpoint: b.endpoint.trim().to_string(),
        api_key: if b.api_key.trim().is_empty() {
            // 覆盖语义：同 id 已有配置则沿用旧密钥
            cfg.nexus.links.iter().find(|l| l.id == b.id).map(|l| l.api_key.clone()).unwrap_or_default()
        } else {
            b.api_key.trim().to_string()
        },
        command: b.command.trim().to_string(),
        enabled: b.enabled,
    };
    if let Some(slot) = cfg.nexus.links.iter_mut().find(|l| l.id == link.id) {
        *slot = link;
    } else {
        cfg.nexus.links.push(link);
    }
    match st.core.apply_config(cfg) {
        Ok(_) => {
            let saved = links_of(&st).into_iter().find(|l| l.id == b.id.trim());
            Json(json!({ "ok": true, "link": saved })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 删除连结体（DELETE /api/nexus/links/:id）
pub async fn delete_link(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let mut cfg: ExmConfig = (*st.core.config()).clone();
    let before = cfg.nexus.links.len();
    cfg.nexus.links.retain(|l| l.id != id);
    if cfg.nexus.links.len() == before {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "连结体不存在" }))).into_response();
    }
    match st.core.apply_config(cfg) {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/nexus/links", get(list_links).post(save_link))
        .route("/api/nexus/links/:id", axum::routing::delete(delete_link))
}
