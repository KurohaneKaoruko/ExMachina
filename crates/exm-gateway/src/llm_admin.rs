//! 多厂商模型档案接入层（docs/11）：档案 CRUD / 热切换 / 连通测试。
//! 所有档案都是 OpenAI 兼容端点（OpenAI/DeepSeek/通义/Kimi/智谱/Ollama/vLLM…），
//! 切换档案 = 更新配置并 apply_config 热重建运行时。

use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use exm_core::config::LlmProfile;
use serde::Deserialize;
use serde_json::{json, Value};

const KEY_MASK: &str = "***已配置***";

fn mask_key(k: &str) -> String {
    if k.trim().is_empty() { String::new() } else { KEY_MASK.to_string() }
}

fn profile_json(p: &LlmProfile) -> Value {
    let keys: Vec<Value> = p.api_keys.iter().map(|k| Value::String(mask_key(k))).collect();
    json!({
        "id": p.id, "name": p.name, "baseUrl": p.base_url,
        "apiKey": mask_key(&p.api_key),
        "apiKeys": keys,
        "apiFormat": if p.api_format.is_empty() { "openai" } else { &p.api_format },
        "orchModel": p.orch_model, "unitModel": p.unit_model,
        "fallback": p.fallback,
        "embedModel": p.embed_model,
    })
}

/// 模型档案列表（apiKey 掩码回显）
pub async fn list_profiles(State(st): State<AppState>) -> impl IntoResponse {
    let cfg = st.core.config();
    let profiles: Vec<Value> = cfg.llm_profiles.iter().map(profile_json).collect();
    Json(json!({ "active": cfg.active_profile, "mock": cfg.use_mock, "profiles": profiles }))
        .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProfileBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// 多 Key 池（掩码位沿用旧池同位键）
    #[serde(default)]
    pub api_keys: Option<Vec<String>>,
    #[serde(default)]
    pub api_format: Option<String>,
    #[serde(default)]
    pub orch_model: Option<String>,
    #[serde(default)]
    pub unit_model: Option<String>,
    /// 失败回退：下一个档案 id（空串清除；缺省 = 沿用）
    #[serde(default)]
    pub fallback: Option<String>,
    /// 嵌入模型（混合记忆检索；空 = 不提供嵌入）
    #[serde(default)]
    pub embed_model: Option<String>,
}

fn resolve_profile_input(
    existing: Option<&LlmProfile>,
    v: &LlmProfileBody,
) -> LlmProfile {
    let key = match v.api_key.as_deref() {
        Some(KEY_MASK) | None => existing.map(|e| e.api_key.clone()).unwrap_or_default(),
        Some(k) if k.trim().is_empty() => existing.map(|e| e.api_key.clone()).unwrap_or_default(),
        Some(k) => k.to_string(),
    };
    // Key 池：掩码位 = 沿用旧池同位键；明文 = 新键
    let mut api_keys: Vec<String> = Vec::new();
    if let Some(list) = &v.api_keys {
        for (i, k) in list.iter().enumerate() {
            if k == KEY_MASK {
                if let Some(old_k) = existing.and_then(|e| e.api_keys.get(i)) {
                    api_keys.push(old_k.clone());
                }
            } else if !k.trim().is_empty() {
                api_keys.push(k.clone());
            }
        }
    }
    if api_keys.is_empty() {
        api_keys = existing.map(|e| e.api_keys.clone()).unwrap_or_default();
    }
    LlmProfile {
        id: v.id
            .clone()
            .unwrap_or_else(|| existing.map(|e| e.id.clone()).unwrap_or_default()),
        name: v.name
            .clone()
            .unwrap_or_else(|| existing.map(|e| e.name.clone()).unwrap_or_else(|| "未命名".into())),
        base_url: v.base_url
            .clone()
            .unwrap_or_else(|| existing.map(|e| e.base_url.clone()).unwrap_or_default()),
        api_key: key,
        api_keys,
        api_format: v.api_format.clone().unwrap_or_else(|| existing.map(|e| e.api_format.clone()).unwrap_or_default()),
        orch_model: v.orch_model
            .clone()
            .unwrap_or_else(|| existing.map(|e| e.orch_model.clone()).unwrap_or_default()),
        unit_model: v.unit_model
            .clone()
            .unwrap_or_else(|| existing.map(|e| e.unit_model.clone()).unwrap_or_default()),
        fallback: match v.fallback.as_deref() {
            None => existing.and_then(|e| e.fallback.clone()),
            Some(f) if f.trim().is_empty() => None,
            Some(f) => Some(f.trim().to_string()),
        },
        embed_model: match v.embed_model.as_deref() {
            None => existing.and_then(|e| e.embed_model.clone()),
            Some(m) if m.trim().is_empty() => None,
            Some(m) => Some(m.trim().to_string()),
        },
    }
}

/// 新增/更新档案；若更新的是生效档案则同步热生效
pub async fn save_profile(State(st): State<AppState>, Json(b): Json<LlmProfileBody>) -> impl IntoResponse {
    let mut cfg = (*st.core.config()).clone();
    let existing = b
        .id
        .as_deref()
        .and_then(|id| cfg.llm_profiles.iter().find(|p| p.id == id));
    let mut profile = resolve_profile_input(existing, &b);
    if profile.id.is_empty() {
        profile.id = format!("p{}", &exm_core::types::new_id()[..6]);
    }
    if profile.base_url.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "baseUrl 不能为空" }))).into_response();
    }
    if let Some(slot) = cfg.llm_profiles.iter_mut().find(|p| p.id == profile.id) {
        *slot = profile.clone();
    } else {
        cfg.llm_profiles.push(profile.clone());
    }
    if cfg.active_profile == profile.id {
        let mut keys = profile.api_keys.clone();
        if keys.is_empty() && !profile.api_key.trim().is_empty() {
            keys.push(profile.api_key.clone());
        }
        cfg.llm = exm_core::config::LlmConfig {
            base_url: profile.base_url.clone(),
            api_key: profile.api_key.clone(),
            api_keys: keys,
            api_format: profile.api_format.clone(),
            orch_model: profile.orch_model.clone(),
            unit_model: profile.unit_model.clone(),
        };
    }
    cfg.use_mock = cfg.use_mock || exm_core::config::mock_enabled_from_env();
    match st.core.apply_config(cfg) {
        Ok(_) => Json(json!({ "ok": true, "id": profile.id, "mock": st.core.is_mock() })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn delete_profile(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let mut cfg = (*st.core.config()).clone();
    if cfg.llm_profiles.len() <= 1 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "至少保留一个模型档案" }))).into_response();
    }
    let Some(pos) = cfg.llm_profiles.iter().position(|p| p.id == id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "档案不存在" }))).into_response();
    };
    cfg.llm_profiles.remove(pos);
    if cfg.active_profile == id {
        let first = cfg.llm_profiles[0].clone();
        cfg.active_profile = first.id.clone();
        let mut keys = first.api_keys.clone();
        if keys.is_empty() && !first.api_key.trim().is_empty() {
            keys.push(first.api_key.clone());
        }
        cfg.llm = exm_core::config::LlmConfig {
            base_url: first.base_url.clone(),
            api_key: first.api_key.clone(),
            api_keys: keys,
            api_format: first.api_format.clone(),
            orch_model: first.orch_model.clone(),
            unit_model: first.unit_model.clone(),
        };
    }
    cfg.use_mock = cfg.use_mock || exm_core::config::mock_enabled_from_env();
    let active = cfg.active_profile.clone();
    match st.core.apply_config(cfg) {
        Ok(_) => Json(json!({ "ok": true, "active": active })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 切换生效档案（热生效：重建 Provider / Runtime / Orchestrator）
pub async fn activate_profile(State(st): State<AppState>, Json(b): Json<LlmProfileBody>) -> impl IntoResponse {
    let Some(id) = b.id.clone() else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "需要 id" }))).into_response();
    };
    let mut cfg = (*st.core.config()).clone();
    let Some(p) = cfg.llm_profiles.iter().find(|p| p.id == id).cloned() else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "档案不存在" }))).into_response();
    };
    cfg.active_profile = p.id.clone();
    let mut keys = p.api_keys.clone();
    if keys.is_empty() && !p.api_key.trim().is_empty() {
        keys.push(p.api_key.clone());
    }
    cfg.llm = exm_core::config::LlmConfig {
        base_url: p.base_url,
        api_key: p.api_key,
        api_keys: keys,
        api_format: p.api_format.clone(),
        orch_model: p.orch_model,
        unit_model: p.unit_model,
    };
    cfg.use_mock = cfg.use_mock || exm_core::config::mock_enabled_from_env();
    match st.core.apply_config(cfg) {
        Ok(_) => Json(json!({ "ok": true, "active": id, "mock": st.core.is_mock() })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 连通测试：向端点发一条 1-token 请求（真实网络，10s 超时）
pub async fn test_profile(State(st): State<AppState>, Json(b): Json<LlmProfileBody>) -> impl IntoResponse {
    let cfg = st.core.config();
    let profile = match &b.id {
        Some(id) => cfg.llm_profiles.iter().find(|p| p.id == *id).cloned(),
        None => Some(LlmProfile {
            id: "resolved".into(),
            name: "resolved".into(),
            base_url: cfg.llm.base_url.clone(),
            api_key: cfg.llm.api_key.clone(),
            api_keys: cfg.llm.api_keys.clone(),
            api_format: cfg.llm.api_format.clone(),
            orch_model: cfg.llm.orch_model.clone(),
            unit_model: cfg.llm.unit_model.clone(),
            fallback: None,
            embed_model: None,
        }),
    };
    let Some(p) = profile else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "档案不存在" }))).into_response();
    };
    if p.api_key.trim().is_empty() && p.api_keys.is_empty() {
        return Json(json!({ "ok": false, "configured": false, "message": "该档案尚未配置 API Key" }))
            .into_response();
    }
    // 按档案协议构造 1-token 探针（URL / 鉴权头 / 请求体各不相同）
    let base = p.base_url.trim_end_matches('/');
    let model = p.orch_model.as_str();
    let fmt = if p.api_format.is_empty() { "openai" } else { p.api_format.as_str() };
    let client = reqwest::Client::new();
    let rb = match fmt {
        "anthropic" => client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", &p.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({
                "model": model, "max_tokens": 1,
                "messages": [{ "role": "user", "content": "ping" }],
            })),
        "azure" => client
            .post(format!("{base}/openai/deployments/{model}/chat/completions?api-version=2024-10-21"))
            .header("api-key", &p.api_key)
            .json(&json!({
                "messages": [{ "role": "user", "content": "ping" }], "max_tokens": 1,
            })),
        "gemini" => client
            .post(format!("{base}/v1beta/models/{model}:generateContent"))
            .header("x-goog-api-key", &p.api_key)
            .json(&json!({
                "contents": [{ "role": "user", "parts": [{ "text": "ping" }] }],
                "generationConfig": { "maxOutputTokens": 1 },
            })),
        _ => client
            .post(format!("{base}/chat/completions"))
            .bearer_auth(&p.api_key)
            .json(&json!({
                "model": model,
                "messages": [{ "role": "user", "content": "ping" }],
                "max_tokens": 1,
            })),
    };
    let resp = rb.timeout(std::time::Duration::from_secs(10)).send().await;
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            Json(json!({
                "ok": (200..300).contains(&status),
                "status": status,
                "snippet": body.chars().take(300).collect::<String>(),
            }))
            .into_response()
        }
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })).into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/llm/profiles", get(list_profiles).post(save_profile))
        .route("/api/llm/profiles/:id", axum::routing::delete(delete_profile))
        .route("/api/llm/active", axum::routing::put(activate_profile))
        .route("/api/llm/test", post(test_profile))
}
