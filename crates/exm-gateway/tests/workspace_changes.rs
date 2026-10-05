//! 工作区变更清单接口测试：git 口径与检查点回退口径
//! （openspec capability-completion 任务 2.3）

use exm_gateway::collect_workspace_changes;

fn unique_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("exm-ws-{tag}-{nanos}"));
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

#[tokio::test]
async fn 变更清单_git口径() {
    let dir = unique_dir("git");
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap(); // 已跟踪修改
    std::fs::write(dir.join("b.txt"), "new file\n").unwrap(); // 未跟踪新文件

    let v = collect_workspace_changes(&dir, &[]).await;
    assert_eq!(v["source"], "git");
    assert_eq!(v["branch"], "master");

    let files = v["files"].as_array().unwrap();
    let a = files.iter().find(|f| f["path"] == "a.txt").expect("缺 a.txt");
    assert_eq!(a["status"], "M");
    assert!(a["diff"]["text"].as_str().unwrap().contains("+two"), "diff 应含新增行");

    let b = files.iter().find(|f| f["path"] == "b.txt").expect("缺 b.txt");
    assert_eq!(b["status"], "A");
    assert!(b["diff"].is_null(), "未跟踪文件不产出 diff");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn 变更清单_检查点回退() {
    let dir = unique_dir("cp");
    std::fs::create_dir_all(dir.join(".exmachina/checkpoints/20260101/src")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join(".exmachina/checkpoints/20260101/src/c.rs"), "old\nline\n").unwrap();
    std::fs::write(dir.join("src/c.rs"), "new\nline\n").unwrap();

    let cps = vec![("20260101".to_string(), "src/c.rs".to_string(), 9u64)];
    let v = collect_workspace_changes(&dir, &cps).await;
    assert_eq!(v["source"], "checkpoint", "非 git 仓库应回退检查点口径");
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let diff = files[0]["diff"]["text"].as_str().unwrap();
    assert!(diff.contains("-old"), "检查点 diff 应含删除行: {diff}");
    assert!(diff.contains("+new"), "检查点 diff 应含新增行: {diff}");

    // 检查点内容与当前一致 → 不出现在清单里
    std::fs::write(dir.join("src/c.rs"), "old\nline\n").unwrap();
    let v = collect_workspace_changes(&dir, &cps).await;
    assert_eq!(v["files"].as_array().unwrap().len(), 0, "未变化的检查点不应列出");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn 变更清单_空目录回退检查点() {
    let dir = unique_dir("empty");
    let v = collect_workspace_changes(&dir, &[]).await;
    assert_eq!(v["source"], "checkpoint");
    assert_eq!(v["files"].as_array().unwrap().len(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------- 文件保存（任务 2.4）

use axum::{Json, extract::State, response::IntoResponse};
use exm_gateway::{AppState, fs_save};

fn test_core(dir: &std::path::Path) -> std::sync::Arc<exm_core::Core> {
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::sync::Arc::new(exm_core::Core::create(dir).expect("创建 Core 失败"))
}

async fn save(dir: &std::path::Path, path: &str, content: &str, core: &std::sync::Arc<exm_core::Core>) -> axum::response::Response {
    let state = AppState { core: core.clone() };
    let body = serde_json::json!({ "path": path, "content": content });
    fs_save(State(state), Json(body)).await.into_response()
}

#[tokio::test]
async fn 文件保存_成功与新文件与检查点() {
    let dir = unique_dir("save");
    let core = test_core(&dir);

    // 新文件：父目录存在即可保存
    let resp = save(&dir, "src/new.rs", "fn a() {}\n", &core).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(std::fs::read_to_string(dir.join("src/new.rs")).unwrap(), "fn a() {}\n");

    // 二次保存已有文件：内容更新 + 当日检查点记录首次改动前版本
    let resp = save(&dir, "src/new.rs", "fn b() {}\n", &core).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(std::fs::read_to_string(dir.join("src/new.rs")).unwrap(), "fn b() {}\n");
    // 检查点里应能找到首次版本
    let cps = core.list_checkpoints();
    assert!(
        cps.iter().any(|(_, rel, _)| rel == "src/new.rs"),
        "应有 src/new.rs 的检查点: {cps:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn 文件保存_越界拒绝() {
    let dir = unique_dir("save-deny");
    let core = test_core(&dir);

    for bad in ["../escape.txt", "a/../../b.txt", "/etc/passwd", ""] {
        let resp = save(&dir, bad, "x", &core).await;
        assert_eq!(resp.status(), 403, "越界路径应 403: {bad:?}");
    }
    // 工作区内没有任何文件被写出
    assert!(!dir.join("escape.txt").exists());
    assert!(!dir.join("etc").exists());

    // 对照：工作区内带缺失父目录的新文件允许保存（自动建目录）
    let resp = save(&dir, "no/such/dir/f.txt", "x", &core).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(std::fs::read_to_string(dir.join("no/such/dir/f.txt")).unwrap(), "x");
    std::fs::remove_dir_all(&dir).ok();
}
