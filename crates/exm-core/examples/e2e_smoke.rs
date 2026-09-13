//! 端到端冒烟示例：逐事件打印，便于定位卡点
//! 运行：cargo run -p exm-core --example e2e_smoke
use exm_core::config::ExmConfig;
use exm_core::Core;
use std::time::Duration;

fn find_workspace_root() -> anyhow::Result<std::path::PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("agents").join("definitions").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            anyhow::bail!("未找到包含 agents/definitions 的工作区根目录");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = find_workspace_root()?;
    let mut cfg = ExmConfig::load(&root);
    cfg.agents_dir = root.join("agents");
    cfg.data_dir = std::env::temp_dir().join(format!("exm-e2e-{}", uuid::Uuid::new_v4()));
    cfg.use_mock = true;
    cfg.max_concurrency = 4;
    println!("[1] data_dir = {}", cfg.data_dir.display());

    let core = Core::with_config(cfg)?;
    println!("[2] core ready: {} 个体", core.registry.count());

    let session = core.create_session("e2e")?;
    println!("[3] session = {}", session.id);

    let mut rx = core.subscribe();
    let printer = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    let brief = match evt.kind.as_str() {
                        "orchestrator.token" => format!(
                            "orchestrator.token({}B)",
                            evt.payload.get("delta").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0)
                        ),
                        "unit.token" => format!(
                            "unit.token({}: {}B)",
                            evt.payload.get("agentId").and_then(|v| v.as_str()).unwrap_or("?"),
                            evt.payload.get("delta").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0)
                        ),
                        other => other.to_string(),
                    };
                    println!("    · {brief}");
                    if evt.kind == "run.finished" || evt.kind == "run.error" {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    println!("    · (lagged {n})");
                }
                Err(_) => return,
            }
        }
    });

    println!("[4] 开始执行任务…");
    let orch = core.orchestrator();
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        orch.handle_user_message(&session.id, "评估 EXMACHINA 项目的架构风险"),
    )
    .await;
    match result {
        Ok(r) => println!("[5] handle_user_message 返回: {:?}", r.map(|_| "ok")),
        Err(_) => println!("[5] 超时 60s —— 流程卡住"),
    }

    let _ = tokio::time::timeout(Duration::from_secs(5), printer).await;

    if let Some(g) = core.store.latest_graph(&session.id)? {
        println!("[6] 图节点：");
        for n in g.nodes {
            println!("      {} {} ({}) -> {}", n.id, n.title, n.agent_identifier, n.status.label());
        }
    }
    println!("[7] 记忆统计: {}", core.memory_stats()?);
    Ok(())
}
