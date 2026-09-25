//! 单体智能体与交互目标（docs/09 §8）：
//! 目标 = 智能体组（集体协作）| 单体智能体（独立工作，不属于任何组）。
//! 用户可切换到某个组或某个单体，但组的成员不提供单体切换（组内个体按协作设计）。

use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use exm_core::types::AgentDefinition;
use serde::Deserialize;
use serde_json::{json, Value};

/// 当前交互目标
pub async fn get_target(State(st): State<AppState>) -> impl IntoResponse {
    let reg = st.core.registry();
    match reg.active_single() {
        Some(id) => {
            let def = reg.single(&id);
            Json(json!({
                "mode": "single",
                "id": id,
                "name": def.as_ref().map(|d| d.name.clone()),
                "primary": id,
            }))
            .into_response()
        }
        None => {
            let meta = st.core.active_group_meta();
            Json(json!({
                "mode": "group",
                "id": st.core.active_group(),
                "name": meta.as_ref().map(|m| m.name.clone()),
                "primary": meta.and_then(|m| m.primary),
            }))
            .into_response()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetBody {
    pub mode: String,
    #[serde(default)]
    pub id: Option<String>,
}

/// 切换交互目标：mode=group 回到激活组；mode=single 切到指定单体
pub async fn set_target(State(st): State<AppState>, Json(b): Json<TargetBody>) -> impl IntoResponse {
    let result = match b.mode.as_str() {
        "group" => st.core.registry().set_active_single(None).map(|_| json!({ "mode": "group" })),
        "single" => {
            let id = b.id.clone().unwrap_or_default();
            st.core
                .registry()
                .set_active_single(Some(&id))
                .map(|_| json!({ "mode": "single", "id": id }))
        }
        _ => Err(anyhow::anyhow!("mode 须为 group 或 single")),
    };
    match result {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("ok".into(), json!(true));
            }
            Json(v).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn list_singles(State(st): State<AppState>) -> impl IntoResponse {
    let singles = st.core.registry().list_singles();
    let active = st.core.registry().active_single();
    Json(json!({
        "active": active,
        "singles": singles.iter().map(|d| {
            json!({
                "identifier": d.identifier, "name": d.name, "description": d.description,
                "domain": d.domain, "tier": d.tier, "modelHint": d.model_hint,
                "capabilities": d.capabilities,
            })
        }).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleBody {
    pub name: String,
    pub identifier: String,
    pub description: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    /// 默认模型（"档案ID" 或 "档案ID/模型名"；缺省 = 跟随全局生效档案）
    #[serde(default)]
    pub model: Option<String>,
}

pub async fn create_single(State(st): State<AppState>, Json(b): Json<SingleBody>) -> impl IntoResponse {
    let def = AgentDefinition {
        name: b.name,
        identifier: b.identifier,
        domain: b.domain.unwrap_or_else(|| "单体".into()),
        tier: exm_core::types::Tier::Orchestrator,
        description: b.description,
        capabilities: vec![],
        tools: vec![exm_core::types::ToolName::Read, exm_core::types::ToolName::Filesystem, exm_core::types::ToolName::Terminal],
        when_to_call: String::new(),
        dependencies: vec![],
        composable_with: vec![],
        input_schema: Default::default(),
        output_schema: Default::default(),
        prompt_file: String::new(),
        model_hint: b.model.filter(|s| !s.trim().is_empty()),
    };
    match st.core.registry().upsert_single(def, b.prompt) {
        Ok(saved) => Json(serde_json::to_value(saved).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn delete_single(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.registry().remove_single(&id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "智能体不存在" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 更新单体智能体可编辑字段（name/domain/description/capabilities/tools/model_hint）。
/// 与 /api/agents/:identifier 共用 registry::update_agent（单体优先命中）；identifier 不可改。
pub async fn update_single(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<crate::AgentUpdateBody>,
) -> impl IntoResponse {
    match st.core.registry().update_agent(&id, &b.into_patch()) {
        Ok(saved) => Json(serde_json::to_value(saved).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/target", get(get_target).put(set_target))
        .route("/api/singles", get(list_singles).post(create_single))
        .route(
            "/api/singles/:id",
            axum::routing::put(update_single).delete(delete_single),
        )
}
