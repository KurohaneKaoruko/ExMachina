//! `exm chat` —— 进程内直挂 core 的一等交互

use crate::render::*;
use exm_core::types::{CoreEvent, TaskGraph};
use exm_core::Core;
use owo_colors::OwoColorize;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

pub struct ChatOptions {
    pub session: Option<String>,
    pub quiet: bool,
}

pub async fn run_chat(core: Arc<Core>, text: &str, opts: &ChatOptions) -> anyhow::Result<()> {
    let session = match &opts.session {
        Some(id) => core
            .store
            .get_session(id)?
            .ok_or_else(|| anyhow::anyhow!("会话不存在: {id}"))?,
        None => core.create_session(text.chars().take(30).collect::<String>().as_str())?,
    };

    let channel_label = if core.is_mock() {
        "mock（模拟）".to_string()
    } else {
        core.config().llm.base_url.clone()
    };
    println!("{}", dim(format!("会话 {}｜LLM 通道：{}", session.id, channel_label)));
    println!("{}", render_role("user", None, &[exm_core::types::Statement::new(exm_core::types::SpeechTag::要求, text)]));
    println!();

    let mut rx = core.subscribe();
    let mut last_graph: Option<TaskGraph> = None;
    let mut current_unit = String::new();
    let mut orch_streaming = false;
    let session_id = session.id.clone();
    let quiet = opts.quiet;
    let orch_id_fallback = core.orchestrator_id();

    // 事件渲染循环与任务执行并行
    let render_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    if evt.session_id != session_id {
                        continue;
                    }
                    match evt.kind.as_str() {
                        "orchestrator.token" => {
                            if let Some(d) = evt.payload.get("delta").and_then(|v| v.as_str()) {
                                if !quiet {
                                    if !orch_streaming {
                                        println!("{}", format!(" {} ", "指挥体".on_blue().white()));
                                        orch_streaming = true;
                                    }
                                    print!("{d}");
                                    use std::io::Write;
                                    let _ = std::io::stdout().flush();
                                }
                            }
                        }
                        "unit.token" => {
                            if quiet {
                                continue;
                            }
                            let agent = evt
                                .payload
                                .get("agentId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unit")
                                .to_string();
                            if agent != current_unit {
                                if !current_unit.is_empty() {
                                    println!();
                                }
                                current_unit = agent.clone();
                                orch_streaming = false;
                                println!("{}", format!(" {agent} ").on_green().black());
                            }
                            if let Some(d) = evt.payload.get("delta").and_then(|v| v.as_str()) {
                                print!("{d}");
                                use std::io::Write;
                                let _ = std::io::stdout().flush();
                            }
                        }
                        "graph.updated" => {
                            if let Ok(g) = serde_json::from_value::<TaskGraph>(evt.payload.clone()) {
                                let changed: Vec<_> = g
                                    .nodes
                                    .iter()
                                    .filter(|n| {
                                        last_graph
                                            .as_ref()
                                            .and_then(|lg| lg.nodes.iter().find(|x| x.id == n.id))
                                            .map(|x| x.status != n.status)
                                            .unwrap_or(true)
                                    })
                                    .collect();
                                if !quiet && !changed.is_empty() {
                                    if orch_streaming || !current_unit.is_empty() {
                                        println!();
                                        orch_streaming = false;
                                        current_unit.clear();
                                    }
                                    for n in changed {
                                        println!("{}", node_line(n));
                                    }
                                }
                                last_graph = Some(g);
                            }
                        }
                        "sync.received" => {
                            if !current_unit.is_empty() {
                                println!();
                                current_unit.clear();
                            }
                            let agent = evt
                                .payload
                                .pointer("/report/sourceAgent")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let conf = evt
                                .payload
                                .pointer("/report/confidence")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            let summary = evt
                                .payload
                                .pointer("/report/summary")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            println!("{}", format!("  ⇦ 同步回流 [{agent}] 置信度 {conf:.2}").cyan());
                            println!("{}", dim(format!("     {}", summary.chars().take(120).collect::<String>())));
                        }
                        "memory.recall" => {
                            let n = evt
                                .payload
                                .get("hits")
                                .and_then(|v| v.as_array())
                                .map(|a| a.len())
                                .unwrap_or(0);
                            if n > 0 {
                                println!("{}", dim(format!("  ⇧ 记忆召回 {n} 条")));
                            }
                        }
                        "memory.written" => {
                            let n = evt
                                .payload
                                .get("entries")
                                .and_then(|v| v.as_array())
                                .map(|a| a.len())
                                .unwrap_or(0);
                            println!("{}", dim(format!("  ⇩ 记忆写入 {n} 条（基础记忆已更新）")));
                        }
                        "arbitration.required" => {
                            println!("{}", err("  ⚖ 冲突待裁决，已追加裁决节点"));
                        }
                        "run.finished" => {
                            let statements: Vec<exm_core::types::Statement> = evt
                                .payload
                                .get("statements")
                                .cloned()
                                .and_then(|v| serde_json::from_value(v).ok())
                                .unwrap_or_default();
                            let orch_id = evt
                                .payload
                                .get("agentId")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| orch_id_fallback.clone());
                            println!();
                            println!(
                                "{}",
                                render_role("orchestrator", Some(orch_id.as_str()), &statements)
                            );
                            return;
                        }
                        "run.error" => {
                            let msg = evt.payload.get("message").and_then(|v| v.as_str()).unwrap_or("");
                            println!("{}", err(format!("【警告】运行失败：{msg}")));
                            return;
                        }
                        _ => {}
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });

    let orch = core.orchestrator();
    orch.handle_user_message(&session.id, text).await?;
    // 等待渲染循环收到 run.finished 并退出
    let _ = tokio::time::timeout(std::time::Duration::from_secs(600), render_task).await;
    Ok(())
}

/// 交互模式：逐轮对话

pub async fn run_interactive(core: Arc<Core>, quiet: bool) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};
    let session = core.create_session("CLI 交互会话")?;
    println!("{}", dim(format!("交互模式｜会话 {}｜输入 exit 退出", session.id)));
    let stdin = std::io::stdin();
    loop {
        print!("{}", "用户> ".yellow());
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t == "exit" || t == "quit" {
            break;
        }
        run_chat(
            core.clone(),
            t,
            &ChatOptions { session: Some(session.id.clone()), quiet },
        )
        .await?;
    }
    Ok(())
}

/// 事件流给渠道消费的统一入口（Gateway/未来桌面端复用）
pub fn spawn_event_logger(mut rx: tokio::sync::broadcast::Receiver<CoreEvent>) {
    tokio::spawn(async move {
        while let Ok(evt) = rx.recv().await {
            println!("{}", dim(format!("[event] {}", evt.kind)));
        }
    });
}
