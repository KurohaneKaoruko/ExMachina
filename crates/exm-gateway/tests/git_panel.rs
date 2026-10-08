//! Git 面板接口测试：只读概览 / 常规操作直发 / 破坏性操作走审批单
//! （openspec capability-completion 任务 2.6）

use axum::{Json, extract::State, response::IntoResponse};
use exm_gateway::{AppState, git_panel};
use serde_json::json;

fn unique_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("exm-git-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git 不可用");
    assert!(out.status.success(), "git {args:?} 失败: {}", String::from_utf8_lossy(&out.stderr));
}

fn test_core(dir: &std::path::Path) -> std::sync::Arc<exm_core::Core> {
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::sync::Arc::new(exm_core::Core::create(dir).expect("创建 Core 失败"))
}

async fn op(st: &std::sync::Arc<exm_core::Core>, body: serde_json::Value) -> axum::response::Response {
    let parsed: exm_gateway::git_panel::GitOpBody = serde_json::from_value(body).unwrap();
    git_panel::op(State(AppState { core: st.clone() }), Json(parsed)).await.into_response()
}

#[tokio::test]
async fn git概览_分支与日志() {
    let dir = unique_dir("ov");
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "first"]);

    let st = test_core(&dir);
    let resp = git_panel::overview(State(AppState { core: st })).await.into_response();
    assert_eq!(resp.status(), 200);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn git操作_暂存提交直发() {
    let dir = unique_dir("op");
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    let st = test_core(&dir);
    std::fs::write(dir.join("f.txt"), "hello\n").unwrap();

    // 非法路径拒绝
    let resp = op(&st, json!({ "op": "stage", "path": "../x" })).await;
    assert_eq!(resp.status(), 400);

    // 暂存 → 提交 → 工作区干净（status 无输出）
    let resp = op(&st, json!({ "op": "stage", "path": "f.txt" })).await;
    assert_eq!(resp.status(), 200);
    let resp = op(&st, json!({ "op": "commit", "message": "c1" })).await;
    assert_eq!(resp.status(), 200);
    let out = std::process::Command::new("git").args(["status", "--porcelain"]).current_dir(&dir).output().unwrap();
    let status_text = String::from_utf8_lossy(&out.stdout).to_string();
    // 注册表播种的编成目录(entities/ + 兼容旧布局的 agents/)是测试环境噪声,不属于提交流程
    let leftover: Vec<&str> = status_text
        .lines()
        .filter(|l| !l.contains(".exmachina/") && !l.contains("agents/") && !l.contains("entities/") && !l.contains("skills/"))
        .collect();
    assert!(leftover.is_empty(), "提交后 f.txt 不应残留未提交变更: {leftover:?}");

    // 非法分支名拒绝
    let resp = op(&st, json!({ "op": "switch", "branch": "-oProxyCommand=x" })).await;
    assert_eq!(resp.status(), 400);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn git操作_破坏性走审批单() {
    let dir = unique_dir("danger");
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("f.txt"), "v1\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "c1"]);
    let st = test_core(&dir);

    std::fs::write(dir.join("f.txt"), "v2 改动\n").unwrap();
    // 丢弃改动：不直接执行，而是生成待批审批单
    let resp = op(&st, json!({ "op": "discard", "path": "f.txt" })).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = serde_json::from_str(
        &String::from_utf8_lossy(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()),
    )
    .unwrap();
    assert_eq!(body["pending"], true, "破坏性操作必须挂审批单");
    let approval_id = body["approvalId"].as_str().unwrap().to_string();
    // 未批准前：文件内容未变
    assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "v2 改动\n");

    // 批准 → 系统代执行 → 改动被丢弃
    let done = st.approval_decide(&approval_id, true).await.unwrap();
    assert_eq!(done.status, "executed");
    assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "v1\n");

    // push 同样走审批单
    let resp = op(&st, json!({ "op": "push" })).await;
    assert_eq!(resp.status(), 200);
    std::fs::remove_dir_all(&dir).ok();
}
