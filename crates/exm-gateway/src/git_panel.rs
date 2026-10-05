//! Git 面板（编码工作台）：状态 / 日志 / 分支的只读视图 + 暂存 / 提交 / 切换等常规操作。
//! 安全口径：只读命令固定参数直发；常规写操作走固定参数数组（不经 shell）；
//! 破坏性操作（推送 / 硬重置 / 丢弃改动）一律生成审批单，由既有审批流批准后代执行。
use axum::{Json, Router, extract::State, http::StatusCode, routing::{get, post}};
use serde_json::{Value, json};

use crate::AppState;

/// git 只读调用（固定子命令，无用户输入拼接）
async fn git_read(ws: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(ws)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// git 写操作（固定参数数组，不经 shell；调用方负责参数合法性）
async fn git_run(ws: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(ws)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(if text.trim().is_empty() { err.trim().to_string() } else { text.trim().to_string() })
    } else {
        Err(if err.trim().is_empty() { text.trim().to_string() } else { err.trim().to_string() })
    }
}

/// 相对路径校验（与工作区文件保存同口径）
fn valid_rel_path(p: &str) -> bool {
    let norm = p.replace('\\', "/").trim_matches('/').to_string();
    !norm.is_empty()
        && !std::path::Path::new(p.trim()).is_absolute()
        && !norm.split('/').any(|seg| seg == "..")
}

/// 分支名校验：防参数注入（无空白 / 不以 '-' 开头 / 有限字符集）
fn valid_branch(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= 200
        && !b.starts_with('-')
        && !b.starts_with('/')
        && !b.ends_with('/')
        && b.chars().all(|c| c.is_alphanumeric() || "._/-+".contains(c))
        && !b.contains("..")
}

use axum::response::{IntoResponse, Response};

/// GET /api/git/overview：分支 + 最近提交（只读）
pub async fn overview(State(st): State<AppState>) -> Response {
    let ws = st.core.config().workspace_root.clone();
    let inside = git_read(&ws, &["rev-parse", "--is-inside-work-tree"])
        .await
        .map(|s| s.trim() == "true")
        .unwrap_or(false);
    if !inside {
        return Json(json!({ "repo": false })).into_response();
    }
    let branch = git_read(&ws, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let log_text = git_read(&ws, &["log", "--oneline", "-20"]).await.unwrap_or_default();
    let log: Vec<&str> = log_text.lines().collect();
    let branch_text = git_read(&ws, &["branch", "--list"]).await.unwrap_or_default();
    let mut branches: Vec<String> = Vec::new();
    let mut current = branch.clone();
    for l in branch_text.lines() {
        let name = l.trim();
        if let Some(starred) = name.strip_prefix("* ") {
            current = starred.trim().to_string();
            branches.insert(0, starred.trim().to_string());
        } else if !name.is_empty() {
            branches.push(name.to_string());
        }
    }
    Json(json!({ "repo": true, "branch": current, "log": log, "branches": branches }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub struct GitOpBody {
    /// stage | unstage | commit | switch | push | reset_hard | discard
    pub op: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

/// POST /api/git/op：常规操作直接执行；破坏性操作生成审批单（批准后由系统代执行）
pub async fn op(State(st): State<AppState>, Json(body): Json<GitOpBody>) -> Response {
    let ws = st.core.config().workspace_root.clone();
    let op = body.op.as_str();
    let result: Result<Value, String> = async {
        match op {
            "stage" => {
                let path = body.path.as_deref().filter(|p| valid_rel_path(p)).ok_or("path 非法或缺失")?;
                git_run(&ws, &["add", "--", path]).await.map(|out| json!({ "ok": true, "output": out }))
            }
            "unstage" => {
                let path = body.path.as_deref().filter(|p| valid_rel_path(p)).ok_or("path 非法或缺失")?;
                git_run(&ws, &["restore", "--staged", "--", path]).await.map(|out| json!({ "ok": true, "output": out }))
            }
            "commit" => {
                let msg = body.message.as_deref().map(str::trim).filter(|m| !m.is_empty()).ok_or("提交信息不能为空")?;
                let msg = msg.chars().take(500).collect::<String>();
                git_run(&ws, &["commit", "-m", &msg]).await.map(|out| json!({ "ok": true, "output": out }))
            }
            "switch" => {
                let branch = body.branch.as_deref().filter(|b| valid_branch(b)).ok_or("分支名非法或缺失")?;
                git_run(&ws, &["switch", branch]).await.map(|out| json!({ "ok": true, "output": out }))
            }
            // 以下为破坏性操作：走既有审批闸门（批准后由 approval_decide 代执行）
            "push" => {
                let cmd = "git push".to_string();
                let req = st.core.request_git_approval(&cmd).await.map_err(|e| e.to_string())?;
                Ok(json!({ "ok": true, "pending": true, "approvalId": req.id, "command": cmd }))
            }
            "reset_hard" => {
                let cmd = "git reset --hard".to_string();
                let req = st.core.request_git_approval(&cmd).await.map_err(|e| e.to_string())?;
                Ok(json!({ "ok": true, "pending": true, "approvalId": req.id, "command": cmd }))
            }
            "discard" => {
                let path = body.path.as_deref().filter(|p| valid_rel_path(p)).ok_or("path 非法或缺失")?;
                let cmd = format!("git checkout -- {path}");
                let req = st.core.request_git_approval(&cmd).await.map_err(|e| e.to_string())?;
                Ok(json!({ "ok": true, "pending": true, "approvalId": req.id, "command": cmd }))
            }
            other => Err(format!("未知操作: {other}")),
        }
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/git/overview", get(overview))
        .route("/api/git/op", post(op))
}
