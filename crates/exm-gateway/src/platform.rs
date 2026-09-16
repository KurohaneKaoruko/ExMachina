//! 平台能力接入层 —— 定时任务/心跳、执行审批、通道网关、技能包（docs/10）
//!
//! 边界：协议翻译与事件转发；调度判定与执行在 exm-core（cron.rs / tools.rs / Core）。

use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router};
use exm_core::types::CoreEvent;
use serde::Deserialize;
use serde_json::{json, Value};
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

// ---------------------------------------------------------------- 技能包

pub async fn list_skills(State(st): State<AppState>) -> impl IntoResponse {
    match st.core.skills() {
        Ok(s) => Json(serde_json::to_value(s).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillBody {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    pub instructions: String,
    #[serde(default)]
    pub agents: Vec<String>,
}

pub async fn create_skill(State(st): State<AppState>, Json(b): Json<SkillBody>) -> impl IntoResponse {
    let skill = exm_core::types::SkillDef {
        id: b.id.trim().to_string(),
        name: b.name.trim().to_string(),
        description: b.description,
        triggers: b.triggers.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        instructions: b.instructions.trim().to_string(),
        agents: b.agents.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        created_at: exm_core::types::now_iso(),
    };
    match st.core.add_skill(skill.clone()) {
        Ok(_) => Json(serde_json::to_value(skill).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn delete_skill(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.remove_skill(&id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "技能不存在" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 组总览（组是交互对象）：编成 / 链路 / 会话 / 记忆 / 成员可靠性 一块仪表数据
pub async fn group_overview(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    if st.core.group_meta(&id).is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "组不存在" }))).into_response();
    }
    let agents = st.core.registry().agents_in_group(&id);
    let playbooks = st.core.registry().playbooks_in_group(&id).unwrap_or_default();
    let sessions = st.core.list_sessions_in_group(&id).unwrap_or_default();
    let stats = st.core.memory_stats().unwrap_or(json!({}));
    // memory_stats() 返回原始统计：byGroup / shared 在顶层
    let group_memory = stats
        .pointer("/byGroup")
        .and_then(|m| m.get(&id))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let global_memory = stats
        .get("shared")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let member_ids: std::collections::HashSet<String> =
        agents.iter().map(|a| a.identifier.clone()).collect();
    let agent_stats: Vec<Value> = stats["agentStats"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|s| {
                    s["agentId"]
                        .as_str()
                        .map(|a| member_ids.contains(a))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Json(json!({
        "group": st.core.group_meta(&id),
        "agents": agents.len(),
        "playbooks": playbooks.len(),
        "sessions": sessions.len(),
        "memory": { "group": group_memory, "shared": global_memory },
        "agentStats": agent_stats,
    }))
    .into_response()
}

/// 指定组的个体清单（组管理页用，不依赖激活组）
pub async fn group_agents(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    Json(serde_json::to_value(st.core.registry().agents_in_group(&id)).unwrap_or(Value::Null))
        .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPrimaryBody {
    pub identifier: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupWorkspaceBody {
    #[serde(default)]
    pub workspace: Option<String>,
}

/// 设置组工作区（空 = 回退全局工作区）
pub async fn set_group_workspace(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<GroupWorkspaceBody>,
) -> impl IntoResponse {
    let ws = b.workspace.unwrap_or_default();
    match st.core.registry().set_workspace(&id, &ws) {
        Ok(_) => Json(json!({ "ok": true, "workspace": if ws.trim().is_empty() { Value::Null } else { json!(ws) } }))
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 设置组内主智能体（内置组会被核心拒绝）
pub async fn set_group_primary(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<GroupPrimaryBody>,
) -> impl IntoResponse {
    match st.core.registry().set_primary(&id, &b.identifier) {
        Ok(_) => Json(json!({ "ok": true, "primary": b.identifier })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupModelBody {
    #[serde(default)]
    pub model: Option<String>,
}

/// 设置组默认模型（空 = 跟随全局生效档案；组内未显式指定模型的个体随组）
pub async fn set_group_model(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<GroupModelBody>,
) -> impl IntoResponse {
    let m = b.model.unwrap_or_default();
    match st.core.registry().set_group_model(&id, &m) {
        Ok(_) => Json(json!({ "ok": true, "model": if m.trim().is_empty() { Value::Null } else { json!(m) } }))
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupCapsBody {
    #[serde(default)]
    pub speech: Option<String>,
    #[serde(default)]
    pub transcribe: Option<String>,
    #[serde(default)]
    pub vision_relay: Option<String>,
    #[serde(default)]
    pub embedding: Option<String>,
}

/// 设置组级能力模型覆盖（每组可用不同模型栈；空串 = 清除该项跟随全局）
pub async fn set_group_capabilities(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<GroupCapsBody>,
) -> impl IntoResponse {
    let caps = exm_core::types::GroupCapabilities {
        speech: b.speech,
        transcribe: b.transcribe,
        vision_relay: b.vision_relay,
        embedding: b.embedding,
    };
    match st.core.registry().set_group_capabilities(&id, caps) {
        Ok(_) => Json(json!({
            "ok": true,
            "capabilities": st.core.group_meta(&id).and_then(|m| m.capabilities),
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 会话事件溯源（活动页用，取最近 limit 条）
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

pub async fn session_events(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> impl IntoResponse {
    match st.core.store.list_events(&id) {
        Ok(mut events) => {
            let limit = q.limit.unwrap_or(200).min(1000);
            if events.len() > limit {
                events = events.split_off(events.len() - limit);
            }
            Json(serde_json::to_value(events).unwrap_or(Value::Null)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- 经验优化（子个体自适应）

/// 手动触发：基于教训与统计合成个体的经验改进要点
pub async fn optimize_agent(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.orchestrator().optimize_agent(&id).await {
        Ok(a) => Json(serde_json::to_value(a).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn get_adaptation(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.registry().load_adaptation(&id) {
        Ok(Some(a)) => Json(serde_json::to_value(a).unwrap_or(Value::Null)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "该个体尚无经验要点" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn reset_adaptation(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.registry().reset_adaptation(&id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "该个体尚无经验要点" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- 定时任务与调度器

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronBody {
    pub name: String,
    pub prompt: String,
    /// 五段 cron 表达式；与 at 二选一
    #[serde(default)]
    pub cron: Option<String>,
    /// 一次性触发时间（ISO8601）
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub session_title: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

pub async fn list_cron(State(st): State<AppState>) -> impl IntoResponse {
    Json(serde_json::to_value(st.core.cron.list().unwrap_or_default()).unwrap_or(Value::Null)).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronRunsQuery {
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub async fn list_cron_runs(State(st): State<AppState>, Query(q): Query<CronRunsQuery>) -> impl IntoResponse {
    match st.core.cron.runs(q.job.as_deref(), q.limit.unwrap_or(20)) {
        Ok(runs) => Json(serde_json::to_value(runs).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn create_cron(State(st): State<AppState>, Json(b): Json<CronBody>) -> impl IntoResponse {
    if b.name.trim().is_empty() || b.prompt.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "name/prompt 不能为空" }))).into_response();
    }
    if b.cron.is_none() && b.at.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "需要 cron 五段表达式或 at 一次性时间" })),
        )
            .into_response();
    }
    if let Some(expr) = &b.cron {
        if expr.split_whitespace().count() != 5 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "cron 须为五段表达式：分 时 日 月 周" })),
            )
                .into_response();
        }
    }
    let job = exm_core::types::CronJob {
        id: String::new(),
        name: b.name.trim().to_string(),
        prompt: b.prompt.clone(),
        cron: b.cron,
        at: b.at,
        group: b.group,
        session_title: b.session_title,
        enabled: b.enabled.unwrap_or(true),
        last_run_at: None,
        last_status: None,
        last_run_minute: None,
        created_at: String::new(),
    };
    match st.core.cron.upsert(job) {
        Ok(j) => Json(serde_json::to_value(j).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
}

pub async fn update_cron(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<CronUpdate>,
) -> impl IntoResponse {
    match st.core.cron.get(&id) {
        Ok(Some(mut job)) => {
            if let Some(e) = b.enabled {
                job.enabled = e;
            }
            match st.core.cron.upsert(job) {
                Ok(j) => Json(serde_json::to_value(j).unwrap_or(Value::Null)).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
            }
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "任务不存在" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn delete_cron(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.cron.remove(&id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "任务不存在" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 手动触发一次（测试/补跑）
pub async fn run_cron_now(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.cron.get(&id) {
        Ok(Some(job)) => {
            let run = st.core.run_cron_job(&job).await;
            Json(serde_json::to_value(run).unwrap_or(Value::Null)).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "任务不存在" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 调度循环：每 20s 扫描到期任务（cron 命中 / 一次性 at 到点）；心跳由配置驱动
pub fn spawn_cron_scheduler(core: std::sync::Arc<exm_core::Core>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let _ = tick_jobs(&core).await;
        }
    });
}

async fn tick_jobs(core: &exm_core::Core) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let jobs = core.cron.list()?;
    let mut changed = false;
    for mut job in jobs {
        if !exm_core::cron::job_due(&job, now) {
            continue;
        }
        let _ = core.events.send(CoreEvent {
            kind: "cron.fired".into(),
            session_id: String::new(),
            payload: json!({ "jobId": job.id, "jobName": job.name, "prompt": job.prompt }),
        });
        job.last_run_at = Some(exm_core::types::now_iso());
        job.last_run_minute = Some(exm_core::cron::minute_key(now));
        let run = core.run_cron_job(&job).await;
        job.last_status = Some(run.status.clone());
        if job.at.is_some() {
            // 一次性任务：触发后停用
            job.enabled = false;
        }
        core.cron.upsert(job)?;
        changed = true;
    }

    // 心跳巡检：配置驱动的内置周期任务（会话固定「心跳巡检」）
    let cfg = core.config();
    if cfg.automation.heartbeat_enabled {
        let interval = cfg.automation.heartbeat_interval_minutes.max(1) as i64;
        let minute = now.timestamp() / 60;
        if minute % interval == 0 {
            let minute_key = exm_core::cron::minute_key(now);
            let already = core
                .cron
                .get("__heartbeat__")?
                .map(|j| j.last_run_minute.as_deref() == Some(minute_key.as_str()))
                .unwrap_or(false);
            if !already {
                let hb = exm_core::types::CronJob {
                    id: "__heartbeat__".into(),
                    name: "心跳巡检".into(),
                    prompt: cfg.automation.heartbeat_prompt.clone(),
                    cron: None,
                    at: None,
                    group: None,
                    session_title: Some("心跳巡检".into()),
                    enabled: true,
                    last_run_at: None,
                    last_status: None,
                    last_run_minute: Some(minute_key),
                    created_at: String::new(),
                };
                let run = core.run_cron_job(&hb).await;
                core.cron.upsert(exm_core::types::CronJob {
                    last_run_at: Some(exm_core::types::now_iso()),
                    last_status: Some(run.status),
                    ..hb
                })?;
                changed = true;
            }
        }
    }

    if changed {
        let _ = core.events.send(CoreEvent {
            kind: "cron.updated".into(),
            session_id: String::new(),
            payload: json!({}),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------- 执行审批

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalsQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub async fn list_approvals(State(st): State<AppState>, Query(q): Query<ApprovalsQuery>) -> impl IntoResponse {
    match st.core.approval_list(q.status.as_deref(), q.limit.unwrap_or(50)) {
        Ok(items) => Json(serde_json::to_value(items).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn approve_request(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.approval_decide(&id, true).await {
        Ok(r) => Json(serde_json::to_value(r).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn deny_request(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match st.core.approval_decide(&id, false).await {
        Ok(r) => Json(serde_json::to_value(r).unwrap_or(Value::Null)).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ---------------------------------------------------------------- 通道网关（webhook 通道）

/// 支持的平台清单：
/// - `webhook` / `qq` / `wechat`：webhook 桥接语义（社区桥把消息 POST 到 inbound，回复走回调）；
/// - `telegram`：内置长轮询适配器（telegram.rs）；
/// - `qqbot`：QQ 官方机器人（q.qq.com 开放平台，WebSocket 长连接，qqbot.rs）；
/// - `napcat`：OneBot 11 实现（NapCat / Lagrange 等，正向 WebSocket，napcat.rs）。
pub const CHANNEL_PLATFORMS: &[&str] = &["webhook", "telegram", "qqbot", "napcat", "qq", "wechat"];

/// 通道定义：外部消息源接入点。平台 = `type`（webhook / telegram / qqbot / napcat …）。
/// 多平台多账号：同类平台可并存多条（每条一个账号），每条可绑定不同智能体组。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    /// 平台：webhook | telegram | qqbot | napcat | qq | wechat
    #[serde(rename = "type", default = "default_platform")]
    pub kind: String,
    #[serde(default = "default_true_channel")]
    pub enabled: bool,
    /// 账号绑定组：该账号的入站消息在此组上下文执行；缺省 = 激活组
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// 会话白名单：允许交互的会话键（Telegram chat id / QQ openid / OneBot 群号或 QQ 号；空 = 不限）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_chats: Vec<String>,
    /// 账号备注（同平台多账号时区分用途）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// webhook / qq / wechat 桥接语义：入站密钥校验
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_webhook: Option<String>,
    /// telegram 平台：BotFather 签发的 bot token
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// 平台扩展配置（新平台零结构体改动）：qqbot → appId/appSecret/sandbox；
    /// napcat → url/token。空值键不入库。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, String>,
    pub created_at: String,
}

fn default_platform() -> String {
    "webhook".into()
}
fn default_true_channel() -> bool {
    true
}

fn channels_path(core: &exm_core::Core) -> std::path::PathBuf {
    core.config().data_dir.join("channels.json")
}

pub(crate) fn load_channels(core: &exm_core::Core) -> Vec<Channel> {
    std::fs::read_to_string(channels_path(core))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_channels(core: &exm_core::Core, channels: &[Channel]) -> anyhow::Result<()> {
    let p = channels_path(core);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(channels)?)?;
    Ok(())
}

impl Channel {
    /// 平台扩展配置读取（config 键，去除首尾空白）
    pub(crate) fn cfg(&self, key: &str) -> Option<String> {
        self.config.get(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }
}

/// 运行收束事件 → 纯文本回帖（telegram / napcat / qqbot 三个适配器共用）。
pub(crate) fn flatten_statements(payload: &serde_json::Value, max_chars: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(list) = payload.get("statements").and_then(|v| v.as_array()) {
        for s in list {
            let tag = s.get("tag").and_then(|v| v.as_str()).unwrap_or("报告");
            let text = s.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.trim().is_empty() {
                continue;
            }
            lines.push(format!("【{tag}】{text}"));
        }
    }
    let mut text = lines.join("\n");
    if text.trim().is_empty() {
        text = "（本轮无收束输出）".into();
    }
    text.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------- 通道运行状态

/// 通道运行状态（内置适配器上报；webhook/qq/wechat 桥接无运行时，不产生状态）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChanStatus {
    /// ok | error
    pub state: String,
    /// 人类可读明细（机器人身份 / 最近一次错误原因）
    pub detail: String,
    /// 最近上报时刻（ISO8601）
    pub at: String,
}

fn chan_statuses() -> &'static Mutex<HashMap<String, ChanStatus>> {
    static S: OnceLock<Mutex<HashMap<String, ChanStatus>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 适配器上报连接状态（连上 / 断开 / 出错均覆盖写，UI 只看最近一次）
pub(crate) fn report_status(id: &str, state: &str, detail: impl Into<String>) {
    chan_statuses().lock().insert(
        id.to_string(),
        ChanStatus { state: state.to_string(), detail: detail.into(), at: exm_core::types::now_iso() },
    );
}

/// 通道删除 / 停用时清除残留状态（下线的账号不该在状态表里阴魂不散）
pub(crate) fn clear_status(id: &str) {
    chan_statuses().lock().remove(id);
}

/// 通道运行状态总览（通道页状态列轮询）
pub async fn channel_status() -> impl IntoResponse {
    let snapshot = chan_statuses().lock().clone();
    Json(serde_json::to_value(snapshot).unwrap_or(Value::Null)).into_response()
}

// ---------------------------------------------------------------- 通道 CRUD（脱敏回显）

/// 平台 config 中的敏感键（掩码回显；更新时掩码值 = 沿用旧值）
fn secret_config_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "qqbot" => &["appSecret"],
        "napcat" => &["token"],
        _ => &[],
    }
}

/// 通道脱敏序列化：顶层 token/secret 与平台敏感 config 键掩码回显，任何接口都不出明文密钥
fn channel_json(ch: &Channel) -> Value {
    let mut v = serde_json::to_value(ch).unwrap_or(Value::Null);
    let Some(obj) = v.as_object_mut() else { return v };
    let mask = Value::String(crate::llm_admin::KEY_MASK.into());
    if ch.secret.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
        obj.insert("secret".into(), mask.clone());
    }
    if ch.token.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
        obj.insert("token".into(), mask.clone());
    }
    if let Some(cfg) = obj.get_mut("config").and_then(|c| c.as_object_mut()) {
        for k in secret_config_keys(&ch.kind) {
            if ch.cfg(k).is_some() {
                cfg.insert((*k).to_string(), mask.clone());
            }
        }
    }
    v
}

pub async fn list_channels(State(st): State<AppState>) -> impl IntoResponse {
    let list: Vec<Value> = load_channels(&st.core).iter().map(channel_json).collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelBody {
    /// 创建必填；更新走路径参数，body 可不带（缺省即空串）
    #[serde(default)]
    pub id: String,
    /// 平台：webhook（默认）| telegram | qqbot | napcat | qq | wechat
    #[serde(default)]
    pub platform: Option<String>,
    /// 账号绑定组
    #[serde(default)]
    pub group: Option<String>,
    /// 会话白名单（空 = 不限）
    #[serde(default)]
    pub allowed_chats: Option<Vec<String>>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub reply_webhook: Option<String>,
    /// telegram bot token
    #[serde(default)]
    pub token: Option<String>,
    /// 平台扩展配置（qqbot: appId/appSecret/sandbox；napcat: url/token）
    #[serde(default)]
    pub config: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

pub async fn create_channel(State(st): State<AppState>, Json(b): Json<ChannelBody>) -> impl IntoResponse {
    let id = b.id.trim().to_string();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "非法通道 id" }))).into_response();
    }
    let platform = b.platform.clone().unwrap_or_else(|| "webhook".into());
    if !CHANNEL_PLATFORMS.contains(&platform.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("不支持的平台（可选 {}）", CHANNEL_PLATFORMS.join(" / ")) })),
        )
            .into_response();
    }
    if platform == "telegram" && b.token.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "telegram 通道需要 bot token" }))).into_response();
    }
    let config: BTreeMap<String, String> = b
        .config
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .collect();
    // 新平台必填项：qqbot 需要 appId + appSecret；napcat 需要 WS 地址
    if platform == "qqbot" && (config.get("appId").map_or(true, |v| v.trim().is_empty()) || config.get("appSecret").map_or(true, |v| v.trim().is_empty())) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "qqbot 通道需要 config.appId 与 config.appSecret" }))).into_response();
    }
    if platform == "napcat" && config.get("url").map_or(true, |v| v.trim().is_empty()) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "napcat 通道需要 config.url（OneBot 11 正向 WebSocket 地址）" }))).into_response();
    }
    let mut channels = load_channels(&st.core);
    if channels.iter().any(|c| c.id == id) {
        return (StatusCode::CONFLICT, Json(json!({ "error": "通道已存在" }))).into_response();
    }
    let ch = Channel {
        id,
        kind: platform,
        enabled: b.enabled.unwrap_or(true),
        group: b.group.clone().filter(|s| !s.trim().is_empty()),
        allowed_chats: b.allowed_chats.clone().unwrap_or_default(),
        account: b.account.clone().filter(|s| !s.trim().is_empty()),
        secret: b.secret.filter(|s| !s.trim().is_empty()),
        reply_webhook: b.reply_webhook.filter(|s| !s.trim().is_empty()),
        token: b.token.filter(|s| !s.trim().is_empty()),
        config,
        created_at: exm_core::types::now_iso(),
    };
    channels.push(ch.clone());
    if let Err(e) = save_channels(&st.core, &channels) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }
    Json(channel_json(&ch)).into_response()
}

/// 更新通道（启停 / 换组 / 换 token 等）
pub async fn update_channel(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<ChannelBody>,
) -> impl IntoResponse {
    let mut channels = load_channels(&st.core);
    let Some(ch) = channels.iter_mut().find(|c| c.id == id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "通道不存在" }))).into_response();
    };
    if b.enabled == Some(false) {
        // 停用即下线：清掉运行状态，避免「已停用」的账号还挂着旧状态
        clear_status(&id);
    }
    if let Some(v) = &b.enabled {
        ch.enabled = *v;
    }
    if let Some(v) = &b.group {
        ch.group = if v.trim().is_empty() { None } else { Some(v.clone()) };
    }
    if let Some(v) = &b.allowed_chats {
        ch.allowed_chats = v.iter().filter(|s| !s.trim().is_empty()).cloned().collect();
    }
    if let Some(v) = &b.account {
        ch.account = if v.trim().is_empty() { None } else { Some(v.clone()) };
    }
    // 敏感字段三态：掩码 = 沿用旧值；空 = 清除；其余 = 新值（与提供商页同一语义）
    if let Some(v) = &b.secret {
        let t = v.trim();
        if t == crate::llm_admin::KEY_MASK {
            // 掩码回传，保留旧值
        } else if t.is_empty() {
            ch.secret = None;
        } else {
            ch.secret = Some(t.to_string());
        }
    }
    if let Some(v) = &b.reply_webhook {
        ch.reply_webhook = if v.trim().is_empty() { None } else { Some(v.clone()) };
    }
    if let Some(v) = &b.token {
        let t = v.trim();
        if t == crate::llm_admin::KEY_MASK {
            // 掩码回传，保留旧值
        } else if t.is_empty() {
            ch.token = None;
        } else {
            ch.token = Some(t.to_string());
        }
    }
    if let Some(v) = &b.config {
        // 整表替换：UI 编辑时按平台提交完整配置；启停等局部更新不带 config 即保持不变。
        // 敏感键掩码回传 = 从旧表取回原值；空值键不入库（用户清空敏感键即删除）。
        let mut next: BTreeMap<String, String> = BTreeMap::new();
        for (k, val) in v {
            let t = val.trim();
            if t.is_empty() {
                continue;
            }
            if t == crate::llm_admin::KEY_MASK {
                if let Some(old) = ch.config.get(k) {
                    next.insert(k.clone(), old.clone());
                }
                continue;
            }
            next.insert(k.clone(), t.to_string());
        }
        ch.config = next;
    }
    let saved = ch.clone();
    match save_channels(&st.core, &channels) {
        Ok(_) => Json(channel_json(&saved)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn delete_channel(State(st): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let mut channels = load_channels(&st.core);
    let before = channels.len();
    channels.retain(|c| c.id != id);
    if channels.len() == before {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "通道不存在" }))).into_response();
    }
    match save_channels(&st.core, &channels) {
        Ok(_) => {
            clear_status(&id);
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundBody {
    #[serde(default)]
    pub secret: Option<String>,
    pub text: String,
    #[serde(default)]
    pub sender_id: Option<String>,
    /// 会话键：相同键复用同一会话；缺省按 senderId；再缺省 default
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default)]
    pub reply_webhook: Option<String>,
}

pub async fn channel_inbound(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<InboundBody>,
) -> impl IntoResponse {
    let Some(channel) = load_channels(&st.core).into_iter().find(|c| c.id == id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "通道不存在" }))).into_response();
    };
    if !channel.enabled {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "通道已停用" }))).into_response();
    }
    if let Some(secret) = &channel.secret {
        if b.secret.as_deref() != Some(secret.as_str()) {
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "通道密钥不符" }))).into_response();
        }
    }
    if b.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "text 不能为空" }))).into_response();
    }
    // 账号绑定组：入站消息在绑定的组上下文执行（会话也归属该组）
    let prev_group = st.core.active_group();
    let mut switched = false;
    if let Some(g) = &channel.group {
        if *g != prev_group && st.core.group_meta(g).is_some() {
            switched = st.core.registry().set_active_group(g).is_ok();
        }
    }
    // 会话复用（组内）：channel:{id}:{sessionKey|senderId|default}
    let key = b
        .session_key
        .clone()
        .or_else(|| b.sender_id.clone())
        .unwrap_or_else(|| "default".into());
    let title = format!("channel:{}:{}", channel.id, key);
    let gid = st.core.active_group();
    let session = match st
        .core
        .list_sessions_in_group(&gid)
        .ok()
        .and_then(|list| list.into_iter().find(|s| s.title == title))
    {
        Some(s) => s,
        None => match st.core.create_session(&title) {
            Ok(s) => s,
            Err(e) => {
                if switched {
                    let _ = st.core.registry().set_active_group(&prev_group);
                }
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
                    .into_response();
            }
        },
    };
    // 出站回调：通道级 replyWebhook 或请求级覆盖；运行结束后推送最终事件载荷
    let reply_url = b.reply_webhook.clone().or_else(|| channel.reply_webhook.clone());
    if let Some(url) = reply_url {
        let core = st.core.clone();
        let sid = session.id.clone();
        let mut rx = core.subscribe();
        tokio::spawn(async move {
            while let Ok(evt) = rx.recv().await {
                if evt.session_id != sid {
                    continue;
                }
                if evt.kind == "run.finished" || evt.kind == "run.error" {
                    let _ = reqwest::Client::new().post(&url).json(&evt.payload).send().await;
                    break;
                }
            }
        });
    }
    // 202 受理，运行过程经 WS 推流；运行结束后恢复激活组
    let core = st.core.clone();
    let text = b.text.clone();
    let sid = session.id.clone();
    tokio::spawn(async move {
        let r = core.chat(&sid, &text).await;
        if switched {
            let _ = core.registry().set_active_group(&prev_group);
        }
        if let Err(e) = r {
            let _ = core.events.send(CoreEvent {
                kind: "run.error".into(),
                session_id: sid,
                payload: json!({ "message": e.to_string() }),
            });
        }
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({ "accepted": true, "sessionId": session.id, "group": st.core.active_group() })),
    )
        .into_response()
}

/// 平台能力路由（挂到 /api 下）
pub fn routes() -> Router<AppState> {
    use axum::routing::{get, post};
    Router::new()
        .route("/api/skills", get(list_skills).post(create_skill))
        .route("/api/skills/:id", axum::routing::delete(delete_skill))
        .route("/api/groups/:id/agents", get(group_agents))
        .route("/api/groups/:id/primary", post(set_group_primary))
        .route("/api/groups/:id/model", axum::routing::put(set_group_model))
        .route("/api/groups/:id/capabilities", axum::routing::put(set_group_capabilities))
        .route("/api/groups/:id/workspace", axum::routing::put(set_group_workspace))
        .route("/api/groups/:id/overview", get(group_overview))
        .route("/api/sessions/:id/events", get(session_events))
        .route("/api/agents/:id/optimize", post(optimize_agent))
        .route(
            "/api/agents/:id/adaptation",
            get(get_adaptation).delete(reset_adaptation),
        )
        .route("/api/cron", get(list_cron).post(create_cron))
        .route("/api/cron/runs", get(list_cron_runs))
        .route("/api/cron/:id", axum::routing::put(update_cron).delete(delete_cron))
        .route("/api/cron/:id/run", post(run_cron_now))
        .route("/api/approvals", get(list_approvals))
        .route("/api/approvals/:id/approve", post(approve_request))
        .route("/api/approvals/:id/deny", post(deny_request))
        .route("/api/channels/status", get(channel_status))
        .route("/api/channels", get(list_channels).post(create_channel))
        .route(
            "/api/channels/:id",
            axum::routing::put(update_channel).delete(delete_channel),
        )
        .route("/api/channels/:id/inbound", post(channel_inbound))
}
