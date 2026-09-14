//! EXMACHINA Gateway —— HTTP REST + WebSocket 接入层（docs/02 M4）
//!
//! 边界：本层只做「协议翻译 + 事件转发」，不含任何调度/提示词逻辑。
//! 所有渠道（WebUI、桌面端、第三方 IM bot）共用同一 REST/WS 契约。

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use exm_core::config::{ExmConfig, LlmConfig};
use exm_core::types::CoreEvent;
use exm_core::Core;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub mod llm_admin;
pub mod singles;
pub mod platform;
pub mod telegram;
pub mod worker_hub;

#[derive(Clone)]
pub struct AppState {
    pub core: Arc<Core>,
}

/// 后台鉴权：auth_key 非空时 /api/* 需 X-Auth-Key（或 Bearer）；/api/auth/verify 与静态资源豁免
async fn auth_middleware(
    State(st): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let required = !st.core.config().security.auth_key.is_empty();
    let path = req.uri().path();
    // 守 /api/* 与 /ws（WS 在升级提取前完成鉴权）；静态资源（壳/登录页）与 verify 豁免
    let guarded = path.starts_with("/api/") || path == "/ws";
    if !required || path == "/api/auth/verify" || !guarded {
        return next.run(req).await;
    }
    let provided = req
        .headers()
        .get("x-auth-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            req.headers()
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ").map(|s| s.to_string()))
        });
    match provided {
        Some(k) if k == st.core.config().security.auth_key => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "需要访问密钥" }))).into_response(),
    }
}

async fn verify_auth(State(st): State<AppState>, Json(b): Json<Value>) -> impl IntoResponse {
    let required = !st.core.config().security.auth_key.is_empty();
    if !required {
        return Json(json!({ "ok": true, "required": false })).into_response();
    }
    let key = b.get("key").and_then(|v| v.as_str()).unwrap_or("");
    if key == st.core.config().security.auth_key {
        Json(json!({ "ok": true, "required": true })).into_response()
    } else {
        (StatusCode::UNAUTHORIZED, Json(json!({ "ok": false, "error": "密钥错误" }))).into_response()
    }
}

pub fn build_router(core: Arc<Core>) -> Router {
    let state = AppState { core: core.clone() };
    let mut router = Router::new()
        .route("/api/health", get(health))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/:id", get(get_session).delete(delete_session))
        .route("/api/sessions/:id/messages", get(list_messages))
        .route("/api/sessions/:id/chat", post(chat))
        .route("/api/sessions/:id/graph", get(get_graph))
        .route("/api/sessions/:id/evidence", get(get_evidence))
        .route("/api/agents", get(list_agents).post(create_agent))
        .route("/api/agents/:identifier", get(get_agent).delete(remove_agent))
        .route("/api/agents/:identifier/model", axum::routing::put(set_agent_model))
        .route("/api/agents/:identifier/persona", get(get_persona).put(put_persona).delete(reset_persona))
        .route("/api/groups", get(list_groups).post(create_group))
        .route("/api/groups/active", get(active_group).put(switch_group))
        .route("/api/groups/:id", axum::routing::delete(delete_group))
        .route("/api/groups/:id/activate", post(activate_group))
        .route("/api/playbooks", get(list_playbooks))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/config/schema", get(get_config_schema))
        .route("/api/mcp", get(mcp_servers))
        .route("/api/mcp/refresh", post(mcp_refresh))
        .route("/api/voice/transcribe", post(voice_transcribe))
        .route("/api/voice/speak", post(voice_speak))
        .route("/api/sessions/:id/tokens", get(session_tokens))
        // 记忆系统（docs/08 §6）
        .route("/api/memory", get(list_memory).post(add_memory))
        .route("/api/memory/search", post(search_memory))
        .route("/api/memory/stats", get(memory_stats))
        .route("/api/memory/reindex", post(memory_reindex))
        .route("/api/memory/decay", post(memory_decay))
        .route("/api/memory/render", post(memory_render))
        .route("/api/memory/:id", axum::routing::delete(forget_memory))
        .route("/api/memory/:id/pin", post(pin_memory))
        .route("/api/memory/md", get(get_memory_md).put(put_memory_md))
        .route("/ws", get(ws_handler))
        .merge(platform::routes())
        .merge(llm_admin::routes())
        .merge(singles::routes())
        .merge(worker_hub::routes())
        .route(
            "/api/groups/:gid/agents/:identifier",
            axum::routing::delete(remove_agent_from_group),
        )
        .route("/api/auth/verify", post(verify_auth))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state);

    // 静态托管 WebUI 构建产物（存在时）；根路径回落 index.html（SPA）
    let dist = core.config().webui_dist.clone();
    if dist.exists() {
        let index = tower_http::services::ServeFile::new(dist.join("index.html"));
        router = router.fallback_service(
            tower_http::services::ServeDir::new(dist).not_found_service(index),
        );
    }
    router
}

// ---------------------------------------------------------------- 健康与配置

async fn health(State(st): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "mock": st.core.is_mock(),
        "agents": st.core.registry.count(),
        "provider": if st.core.is_mock() { "mock" } else { "openai-compatible" },
    }))
}

async fn get_config(State(st): State<AppState>) -> impl IntoResponse {
    let cfg = st.core.config();
    let masked = if cfg.llm.api_key.is_empty() { String::new() } else { "***已配置***".to_string() };
    Json(json!({
        "llm": {
            "baseUrl": cfg.llm.base_url,
            "apiKey": masked,
            "orchModel": cfg.llm.orch_model,
            "unitModel": cfg.llm.unit_model,
        },
        "maxConcurrency": cfg.max_concurrency,
        "maxSessionTokens": cfg.max_session_tokens,
        "memory": {
            "enabled": cfg.memory_enabled,
            "recallLimit": cfg.memory_recall_limit,
            "halfLifeDays": cfg.memory_half_life_days,
        },
        "security": {
            "execApproval": cfg.security.exec_approval,
            "execAllowlist": cfg.security.exec_allowlist,
        },
        "automation": {
            "heartbeatEnabled": cfg.automation.heartbeat_enabled,
            "heartbeatIntervalMinutes": cfg.automation.heartbeat_interval_minutes,
            "heartbeatPrompt": cfg.automation.heartbeat_prompt,
            "autoAdapt": cfg.automation.auto_adapt,
        },
        "mock": st.core.is_mock(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigBody {
    #[serde(default)]
    llm: Option<PartialLlm>,
    #[serde(default)]
    max_concurrency: Option<usize>,
    #[serde(default)]
    max_session_tokens: Option<u64>,
    #[serde(default)]
    memory: Option<PartialMemory>,
    #[serde(default)]
    security: Option<PartialSecurity>,
    #[serde(default)]
    automation: Option<PartialAutomation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialSecurity {
    #[serde(default)]
    exec_approval: Option<String>,
    #[serde(default)]
    exec_allowlist: Option<Vec<String>>,
    #[serde(default)]
    auth_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialAutomation {
    #[serde(default)]
    heartbeat_enabled: Option<bool>,
    #[serde(default)]
    heartbeat_interval_minutes: Option<u32>,
    #[serde(default)]
    heartbeat_prompt: Option<String>,
    #[serde(default)]
    auto_adapt: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialMemory {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    recall_limit: Option<usize>,
    #[serde(default)]
    half_life_days: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialLlm {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    orch_model: Option<String>,
    #[serde(default)]
    unit_model: Option<String>,
}

async fn put_config(State(st): State<AppState>, Json(body): Json<ConfigBody>) -> impl IntoResponse {
    let current = st.core.config();
    let partial = body.llm.unwrap_or(PartialLlm {
        base_url: None,
        api_key: None,
        orch_model: None,
        unit_model: None,
    });
    let api_key = match partial.api_key.as_deref() {
        Some("***已配置***") | None => current.llm.api_key.clone(),
        Some(v) if v.trim().is_empty() => current.llm.api_key.clone(),
        Some(v) => v.to_string(),
    };
    let mem = body.memory.unwrap_or(PartialMemory {
        enabled: None,
        recall_limit: None,
        half_life_days: None,
    });
    let sec = body
        .security
        .unwrap_or(PartialSecurity { exec_approval: None, exec_allowlist: None, auth_key: None });
    let auto = body.automation.unwrap_or(PartialAutomation {
        heartbeat_enabled: None,
        heartbeat_interval_minutes: None,
        heartbeat_prompt: None,
        auto_adapt: None,
    });
    let mut next = ExmConfig {
        llm: LlmConfig {
            base_url: partial.base_url.unwrap_or_else(|| current.llm.base_url.clone()),
            api_key,
            api_keys: current.llm.api_keys.clone(),
            api_format: current.llm.api_format.clone(),
            orch_model: partial.orch_model.unwrap_or_else(|| current.llm.orch_model.clone()),
            unit_model: partial.unit_model.unwrap_or_else(|| current.llm.unit_model.clone()),
        },
        max_concurrency: body.max_concurrency.unwrap_or(current.max_concurrency),
        max_session_tokens: body.max_session_tokens.unwrap_or(current.max_session_tokens),
        memory_enabled: mem.enabled.unwrap_or(current.memory_enabled),
        memory_recall_limit: mem.recall_limit.unwrap_or(current.memory_recall_limit),
        memory_half_life_days: mem.half_life_days.unwrap_or(current.memory_half_life_days),
        use_mock: false,
        security: exm_core::config::SecurityConfig {
            exec_approval: sec.exec_approval.unwrap_or_else(|| current.security.exec_approval.clone()),
            exec_allowlist: sec.exec_allowlist.unwrap_or_else(|| current.security.exec_allowlist.clone()),
            // 掩码/缺省 = 沿用旧密钥；显式空串 = 清除（回到免鉴权）
            auth_key: match sec.auth_key.as_deref() {
                Some(k) if k == "***已配置***" => current.security.auth_key.clone(),
                Some(k) => k.trim().to_string(),
                None => current.security.auth_key.clone(),
            },
        },
        automation: exm_core::config::AutomationConfig {
            heartbeat_enabled: auto.heartbeat_enabled.unwrap_or(current.automation.heartbeat_enabled),
            heartbeat_interval_minutes: auto
                .heartbeat_interval_minutes
                .unwrap_or(current.automation.heartbeat_interval_minutes),
            heartbeat_prompt: auto.heartbeat_prompt.unwrap_or_else(|| current.automation.heartbeat_prompt.clone()),
            auto_adapt: auto.auto_adapt.unwrap_or(current.automation.auto_adapt),
        },
        ..(*current).clone()
    };
    // 测试替身状态由运行期/环境决定，配置载荷不改变它（设置页的 llm 修改只在"真实通道"语义内生效）
    next.use_mock = st.core.is_mock() || exm_core::config::mock_enabled_from_env();
    // 设置页对 llm 的修改同步回生效档案（避免档案切换时被旧值回退）
    if let Some(slot) = next.llm_profiles.iter_mut().find(|p| p.id == next.active_profile) {
        slot.base_url = next.llm.base_url.clone();
        slot.api_key = next.llm.api_key.clone();
        slot.api_keys = next.llm.api_keys.clone();
        slot.orch_model = next.llm.orch_model.clone();
        slot.unit_model = next.llm.unit_model.clone();
    }

    match st.core.apply_config(next) {
        Ok(_) => Json(json!({ "ok": true, "mock": st.core.is_mock() })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- 会话

async fn create_session(State(st): State<AppState>, body: Option<Json<Value>>) -> impl IntoResponse {
    let title = body
        .and_then(|Json(v)| v.get("title").and_then(|t| t.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "新会话".to_string());
    match st.core.create_session(&title) {
        Ok(s) => Json(serde_json::to_value(s).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn list_sessions(State(st): State<AppState>, Query(q): Query<SessionListQuery>) -> impl IntoResponse {
    // 组是交互对象：会话列表归属激活组（?all=1 管理视角看全部）
    let result = if q.all.unwrap_or(false) {
        st.core.list_sessions()
    } else {
        st.core.list_sessions_in_scope()
    };
    match result {
        Ok(list) => Json(serde_json::to_value(list).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionListQuery {
    #[serde(default)]
    all: Option<bool>,
}

async fn delete_session(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.delete_session(&id) {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn get_session(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.store.get_session(&id) {
        Ok(Some(s)) => Json(serde_json::to_value(s).unwrap_or(Value::Null)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "会话不存在" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn list_messages(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.store.list_messages(&id, 200) {
        Ok(msgs) => Json(serde_json::to_value(msgs).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct ChatBody {
    text: String,
    /// 图片附件（data URL，多模态输入；随本轮进入规划）
    #[serde(default)]
    images: Vec<String>,
}

async fn chat(State(st): State<AppState>, Path(id): Path<String>, Json(body): Json<ChatBody>) -> impl IntoResponse {
    if body.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "text 不能为空" }))).into_response();
    }
    if st.core.store.get_session(&id).ok().flatten().is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "会话不存在" }))).into_response();
    }
    // 202 受理，运行过程经 WS 推流（core.chat：会话串行 + followup/collect）
    let core = st.core.clone();
    let text = body.text.clone();
    let images = body.images.clone();
    if !images.is_empty() {
        core.stage_images(&id, images);
    }
    tokio::spawn(async move {
        if let Err(e) = core.chat(&id, &text).await {
            let _ = core.events.send(CoreEvent {
                kind: "run.error".into(),
                session_id: id.clone(),
                payload: json!({ "message": e.to_string() }),
            });
        }
    });
    (StatusCode::ACCEPTED, Json(json!({ "accepted": true }))).into_response()
}

async fn get_graph(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.store.latest_graph(&id) {
        Ok(Some(g)) => Json(serde_json::to_value(g).unwrap_or(Value::Null)).into_response(),
        Ok(None) => Json(json!({ "nodes": [] })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn get_evidence(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.store.list_evidence(&id) {
        Ok(v) => Json(Value::Array(v)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- 编制

async fn list_agents(State(st): State<AppState>) -> impl IntoResponse {
    Json(serde_json::to_value(st.core.registry().list_active()).unwrap_or(Value::Null))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAgentBody {
    name: String,
    identifier: String,
    description: String,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    /// 目标组；缺省 = 激活组（内置组受保护，写入会被拒绝）
    #[serde(default)]
    group: Option<String>,
    /// 默认模型（"档案ID" 或 "档案ID/模型名"；缺省 = 跟随所属组/全局生效档案）
    #[serde(default)]
    model: Option<String>,
}

async fn create_agent(State(st): State<AppState>, Json(b): Json<CreateAgentBody>) -> impl IntoResponse {
    let def = exm_core::types::AgentDefinition {
        name: b.name,
        identifier: b.identifier,
        domain: b.domain.unwrap_or_else(|| "自定义".into()),
        tier: if b.tier.as_deref() == Some("orchestrator") {
            exm_core::types::Tier::Orchestrator
        } else {
            exm_core::types::Tier::Unit
        },
        description: b.description,
        capabilities: vec![],
        tools: vec![],
        when_to_call: String::new(),
        dependencies: vec![],
        composable_with: vec![],
        input_schema: Default::default(),
        output_schema: Default::default(),
        prompt_file: String::new(),
        model_hint: b.model.filter(|s| !s.trim().is_empty()),
    };
    // 目标组：显式指定时写入该组（首个个体自动成为该组主智能体），否则激活组
    let result = match &b.group {
        Some(gid) => {
            let meta = st.core.group_meta(gid);
            let was_empty = st.core.registry().group_agent_count(gid) == 0;
            st.core
                .registry()
                .upsert_agent(gid, def, b.prompt.clone())
                .map(|saved| {
                    if was_empty || b.tier.as_deref() == Some("orchestrator") {
                        let _ = st.core.registry().set_primary(gid, &saved.identifier);
                    }
                    let _ = meta;
                    saved
                })
        }
        None => st.core.upsert_agent(def, b.prompt).map(|saved| {
            if st.core.registry().count() == 1 {
                let _ = st.core.set_primary(&saved.identifier);
            }
            saved
        }),
    };
    match result {
        Ok(saved) => Json(serde_json::to_value(saved).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 按组删除个体（子个体页可跨组管理，不依赖激活组）
async fn remove_agent_from_group(
    State(st): State<AppState>,
    Path((gid, identifier)): Path<(String, String)>,
) -> impl IntoResponse {
    match st.core.registry().remove_agent(&gid, &identifier) {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn remove_agent(State(st): State<AppState>, Path(identifier): Path<String>) -> impl IntoResponse {
    match st.core.remove_agent(&identifier) {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentModelBody {
    #[serde(default)]
    model: Option<String>,
}

/// 设置个体默认模型（智能体/个体设置页用；空 = 跟随所属组/全局生效档案）
async fn set_agent_model(
    State(st): State<AppState>,
    Path(identifier): Path<String>,
    Json(b): Json<AgentModelBody>,
) -> impl IntoResponse {
    let model = b.model.unwrap_or_default();
    match st.core.registry().set_agent_model(&identifier, &model) {
        Ok(_) => Json(json!({ "ok": true, "model": if model.trim().is_empty() { Value::Null } else { json!(model) } }))
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- 智能体组

async fn list_groups(State(st): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "active": st.core.active_group(),
        "groups": st.core.list_groups(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateGroupBody {
    name: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn create_group(State(st): State<AppState>, Json(b): Json<CreateGroupBody>) -> impl IntoResponse {
    match st.core.create_group(b.id, &b.name, b.description.as_deref().unwrap_or("")) {
        Ok(meta) => Json(serde_json::to_value(meta).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn active_group(State(st): State<AppState>) -> impl IntoResponse {
    Json(json!({ "active": st.core.active_group() }))
}

#[derive(Deserialize)]
struct SwitchGroupBody {
    id: String,
}

async fn switch_group(State(st): State<AppState>, Json(b): Json<SwitchGroupBody>) -> impl IntoResponse {
    match st.core.switch_group(&b.id) {
        Ok(_) => Json(json!({ "ok": true, "active": b.id })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn activate_group(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    switch_group(State(st), Json(SwitchGroupBody { id })).await
}

async fn delete_group(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.delete_group(&id) {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn get_agent(State(st): State<AppState>, Path(identifier): Path<String>) -> impl IntoResponse {
    match st.core.agent(&identifier) {
        Some(a) => Json(serde_json::to_value(a).unwrap_or(Value::Null)).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "个体不存在" }))).into_response(),
    }
}

// ---------------------------------------------------------------- 人设（说话风格）

async fn get_persona(State(st): State<AppState>, Path(identifier): Path<String>) -> impl IntoResponse {
    match st.core.persona(&identifier) {
        Ok(persona) => {
            let custom = st.core.persona_is_custom(&identifier).unwrap_or(false);
            Json(json!({
                "identifier": identifier,
                "persona": persona,
                "custom": custom,
                "default": exm_core::registry::LocalRegistry::DEFAULT_PERSONA,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct PersonaBody {
    persona: String,
}

async fn put_persona(
    State(st): State<AppState>,
    Path(identifier): Path<String>,
    Json(b): Json<PersonaBody>,
) -> impl IntoResponse {
    match st.core.set_persona(&identifier, &b.persona) {
        Ok(_) => Json(json!({ "ok": true, "custom": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn reset_persona(State(st): State<AppState>, Path(identifier): Path<String>) -> impl IntoResponse {
    match st.core.reset_persona(&identifier) {
        Ok(_) => Json(json!({ "ok": true, "custom": false })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn list_playbooks(State(st): State<AppState>) -> impl IntoResponse {
    match st.core.playbooks() {
        Ok(p) => Json(serde_json::to_value(p).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- 记忆系统

async fn get_config_schema() -> impl IntoResponse {
    Json(exm_core::config::config_schema())
}

// ---------------------------------------------------------------- MCP（第三方工具生态）

async fn mcp_servers(State(st): State<AppState>) -> impl IntoResponse {
    Json(json!({ "servers": st.core.mcp().servers_brief() }))
}

/// 语音转写：multipart 字段 audio（webm/mp3/wav）→ 文本（全局档案 whisper）
async fn voice_transcribe(
    State(st): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    let mut audio: Option<(Vec<u8>, String)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("audio") {
            let name = field.file_name().unwrap_or("audio.webm").to_string();
            match field.bytes().await {
                Ok(b) => audio = Some((b.to_vec(), name)),
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("读取音频失败: {e}") }))).into_response()
                }
            }
        }
    }
    let Some((bytes, name)) = audio else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 audio 字段" }))).into_response();
    };
    match st.core.transcribe(&bytes, &name).await {
        Ok(text) => Json(json!({ "text": text })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 语音合成：{text} → mp3 字节（audio/mpeg）
async fn voice_speak(State(st): State<AppState>, Json(b): Json<serde_json::Value>) -> impl IntoResponse {
    let text = b.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
    if text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "text 不能为空" }))).into_response();
    }
    match st.core.speak(&text).await {
        Ok(mp3) => (
            [(axum::http::header::CONTENT_TYPE, "audio/mpeg")],
            mp3,
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 会话 token 用量（估算口径）与预算
async fn session_tokens(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let used = st.core.session_tokens_estimate(&id);
    let budget = st.core.config().max_session_tokens;
    Json(json!({ "estimate": used, "budget": budget, "unlimited": budget == 0 }))
}

async fn mcp_refresh(State(st): State<AppState>) -> impl IntoResponse {
    let results = st.core.mcp().refresh_all().await;
    let out: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(id, r)| match r {
            Ok(n) => json!({ "id": id, "ok": true, "tools": n }),
            Err(e) => json!({ "id": id, "ok": false, "error": e.to_string() }),
        })
        .collect();
    Json(json!({ "refreshed": out }))
}

async fn list_memory(
    State(st): State<AppState>,
    Query(q): Query<MemoryQuery>,
) -> impl IntoResponse {
    let kind = q.kind.as_deref().and_then(exm_core::memory::MemoryKind::parse);
    match st.core.memory_list_filtered(kind, q.agent.as_deref(), q.limit.unwrap_or(30)) {
        Ok(items) => Json(serde_json::to_value(items).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct MemoryQuery {
    #[serde(default)]
    kind: Option<String>,
    /// 个体归属过滤：传入 identifier = 该个体私有 + 群体共享
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryBody {
    #[serde(default)]
    kind: Option<String>,
    title: String,
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    pin: bool,
    #[serde(default)]
    importance: Option<f64>,
    #[serde(default)]
    session_id: Option<String>,
    /// 个体归属：传入则为该智能体的私有记忆；缺省为群体共享
    #[serde(default)]
    agent_id: Option<String>,
    /// 跨组全局记忆（缺省归属激活组）
    #[serde(default)]
    global: bool,
}

async fn add_memory(State(st): State<AppState>, Json(b): Json<MemoryBody>) -> impl IntoResponse {
    let kind = b
        .kind
        .as_deref()
        .and_then(exm_core::memory::MemoryKind::parse)
        .unwrap_or(exm_core::memory::MemoryKind::Fact);
    let mut draft = exm_core::memory::MemoryDraft::new(kind, b.title, b.body)
        .importance(b.importance.unwrap_or(0.6))
        .tags(&[]);
    draft.tags = b.tags.clone();
    if let Some(sid) = &b.session_id {
        draft.session_id = Some(sid.clone());
    }
    if let Some(aid) = &b.agent_id {
        draft.agent_id = Some(aid.clone());
    }
    let result = if b.global { st.core.memory_remember_global(draft) } else { st.core.memory_remember(draft) };
    match result {
        Ok(entry) => {
            if b.pin {
                let _ = st.core.memory_pin(&entry.id, true);
            }
            Json(serde_json::to_value(entry).unwrap_or(Value::Null)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    /// 个体检索：传入 identifier = 该个体私有 + 群体共享
    #[serde(default)]
    agent: Option<String>,
}

async fn search_memory(State(st): State<AppState>, Json(b): Json<SearchBody>) -> impl IntoResponse {
    let result = match &b.agent {
        Some(agent) => st.core.memory_recall_for_agent(agent, &b.query, b.limit),
        None => st.core.memory_recall(&b.query, b.limit),
    };
    match result {
        Ok(hits) => Json(json!({
            "query": b.query,
            "hits": hits.iter().map(|h| json!({
                "entry": serde_json::to_value(&h.entry).unwrap_or(Value::Null),
                "score": h.score,
                "reasons": h.reasons,
            })).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn memory_stats(State(st): State<AppState>) -> impl IntoResponse {
    match st.core.memory_stats() {
        Ok(s) => {
            let stats = st.core.memory.agent_stats(20).unwrap_or_default();
            Json(json!({ "memory": s, "agentStats": stats })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn memory_reindex(State(st): State<AppState>) -> impl IntoResponse {
    match st.core.memory_reindex() {
        Ok(n) => Json(json!({ "ok": true, "reindexed": n })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn memory_decay(State(st): State<AppState>) -> impl IntoResponse {
    match st.core.memory_decay(0.05) {
        Ok(n) => Json(json!({ "ok": true, "changed": n })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 读取 memory.md（深层记忆关闭时的唯一记忆载体；OpenClaw/Hermes 文件记忆模式）
async fn get_memory_md(State(st): State<AppState>) -> impl IntoResponse {
    let path = st.core.config().memory_md_path.clone();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    Json(json!({
        "content": content,
        "path": path.display().to_string(),
        "deepEnabled": !st.core.config().memory_enabled,
    }))
}

/// 保存 memory.md
async fn put_memory_md(State(st): State<AppState>, Json(b): Json<serde_json::Value>) -> impl IntoResponse {
    let Some(content) = b.get("content").and_then(|c| c.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 content" }))).into_response();
    };
    let path = st.core.config().memory_md_path.clone();
    if let Err(e) = std::fs::write(&path, content) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("写入失败: {e}") }))).into_response();
    }
    // 超限自主压缩：后台执行（LLM 简略 + 旧文归档），写入立即返回
    let over = content.chars().count() > st.core.config().memory_md_max_chars;
    if over {
        let core = st.core.clone();
        tokio::spawn(async move {
            if let Err(e) = core.compact_memory_md().await {
                eprintln!("[memory] memory.md 自动压缩失败：{e}");
            }
        });
    }
    Json(json!({ "ok": true, "compacting": over })).into_response()
}

async fn memory_render(State(st): State<AppState>) -> impl IntoResponse {
    match st.core.render_memory_md() {
        Ok(n) => Json(json!({ "ok": true, "pinned": n })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn forget_memory(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.memory_forget(&id) {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct PinBody {
    #[serde(default)]
    pinned: bool,
}

async fn pin_memory(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<PinBody>,
) -> impl IntoResponse {
    match st.core.memory_pin(&id, b.pinned) {
        Ok(_) => Json(json!({ "ok": true, "pinned": b.pinned })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- WebSocket

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsQuery {
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct WsAuthQuery {
    #[serde(default)]
    key: Option<String>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    Query(a): Query<WsAuthQuery>,
    State(st): State<AppState>,
) -> impl IntoResponse {
    let required = !st.core.config().security.auth_key.is_empty();
    if required && a.key.as_deref() != Some(st.core.config().security.auth_key.as_str()) {
        return (StatusCode::UNAUTHORIZED, "需要访问密钥").into_response();
    }
    ws.on_upgrade(move |socket| ws_loop(socket, st.core, q.session_id))
}

async fn ws_loop(mut socket: WebSocket, core: Arc<Core>, session_filter: Option<String>) {
    let mut rx = core.subscribe();
    loop {
        tokio::select! {
            evt = rx.recv() => {
                match evt {
                    Ok(e) => {
                        if let Some(sid) = &session_filter {
                            if &e.session_id != sid {
                                continue;
                            }
                        }
                        let text = serde_json::to_string(&e).unwrap_or_default();
                        if socket.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 启动

pub async fn serve(core: Arc<Core>, port: u16) -> anyhow::Result<()> {
    platform::spawn_cron_scheduler(core.clone());
    telegram::spawn_supervisor(core.clone());
    // 分布式执行：工作者池注入（有工作者在线即自动路由远程，失败回落本地）
    {
        let st = AppState { core: core.clone() };
        core.set_remote(worker_hub::hub(&st))?;
    }
    // 断点续跑：上次进程中断的会话（图非终态）自动重新调度收束
    {
        let n = core.resume_interrupted().await;
        if n > 0 {
            println!("[gateway] 断点续跑：已恢复 {n} 个中断会话");
        }
    }
    // MCP 工具清单后台刷新（懒连接，失败仅记录；首次调用会重试）
    {
        let mcp = core.mcp();
        tokio::spawn(async move {
            for (id, r) in mcp.refresh_all().await {
                match r {
                    Ok(n) if n > 0 => println!("[mcp] {id}: {n} 个工具就绪"),
                    Ok(_) => {}
                    Err(e) => eprintln!("[mcp] {id}: 清单获取失败（首次调用时重试）: {e}"),
                }
            }
        });
    }
    let router = build_router(core.clone());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("[gateway] EXMACHINA Gateway 已启动");
    println!("[gateway]   REST  http://127.0.0.1:{port}/api");
    println!("[gateway]   WS    ws://127.0.0.1:{port}/ws?sessionId=<id>");
    println!(
        "[gateway]   LLM   {}",
        if core.is_mock() {
            "测试替身（EXM_LLM_MOCK=1）".to_string()
        } else {
            let c = core.config();
            let configured = !c.llm.api_key.trim().is_empty() || !c.llm.api_keys.is_empty();
            if configured {
                format!("{}（{}）", if c.active_profile.is_empty() { "全局通道" } else { c.active_profile.as_str() }, c.llm.base_url)
            } else {
                "未配置 —— 请在「模型提供商」页填写端点与 API Key".to_string()
            }
        }
    );
    println!("[gateway]   个体  {}（1 指挥体 + {} 子个体）", core.registry.count(), core.registry.units().len());
    let dist = core.config().webui_dist.clone();
    println!(
        "[gateway]   WebUI {}",
        if dist.exists() { format!("{}", dist.display()) } else { "未构建（运行 npm run build:webui 后可由本网关托管）".to_string() }
    );
    axum::serve(listener, router).await?;
    Ok(())
}
