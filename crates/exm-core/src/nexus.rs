//! 智能连结网络执行器（实验性）——把任务派发给外部连结体执行。
//!
//! 三层结构：全连结指挥体（本集群指挥体）→ 连结指挥体（外部连结体的主智能体）→ 子个体。
//! 连结体两种形态（可混用）：
//! - exmachina：另一台机器/进程的 EXMACHINA 网关（REST + 轮询取结果）
//! - CLI 智能体：opencode / codex / claude 等本机命令行智能体（其子代理即子个体）
//!
//! 执行契约：输入目标与验收，输出（摘要, 全文）。失败/超时/非零退出返回 Err。

use crate::config::{ExmConfig, LinkTarget};
use anyhow::{Context, Result};

pub const DEFAULT_TIMEOUT_SECS: u64 = 900;

fn find_link<'a>(cfg: &'a ExmConfig, link_id: &str) -> Result<&'a LinkTarget> {
    cfg.nexus
        .links
        .iter()
        .find(|l| l.id == link_id && l.enabled)
        .context(format!("连结体不存在或未启用: {link_id}"))
}

/// 派发入口：按连结体类型路由
pub async fn execute_link(cfg: &ExmConfig, link_id: &str, objective: &str, acceptance: &[String]) -> Result<(String, String)> {
    let link = find_link(cfg, link_id)?;
    let prompt = build_prompt(objective, acceptance);
    match link.kind.as_str() {
        "exmachina" => execute_exmachina(cfg, link, &prompt).await,
        "opencode" | "codex" | "claude" | "custom" => execute_cli(link, &prompt).await,
        other => Err(anyhow::anyhow!("未知连结体类型: {other}")),
    }
}

fn build_prompt(objective: &str, acceptance: &[String]) -> String {
    if acceptance.is_empty() {
        objective.to_string()
    } else {
        format!("{objective}\n\n验收标准：\n{}", acceptance.iter().map(|a| format!("- {a}")).collect::<Vec<_>>().join("\n"))
    }
}

// ---------------------------------------------------------------- exmachina（远程网关）

async fn execute_exmachina(cfg: &ExmConfig, link: &LinkTarget, prompt: &str) -> Result<(String, String)> {
    let base = link.endpoint.trim_end_matches('/');
    if base.is_empty() {
        anyhow::bail!("连结体 {} 未配置网关地址", link.id);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let auth = [("X-Auth-Key", link.api_key.as_str())];

    // 1) 复用/创建专属会话（标题约定 nexus-<link_id>，同连结体的任务在同一会话内延续上下文）
    let title = format!("nexus-{}", link.id);
    let sessions: serde_json::Value = client
        .get(format!("{base}/api/sessions?all=true"))
        .header("X-Auth-Key", &link.api_key)
        .send()
        .await?
        .json()
        .await
        .context("会话清单解析失败")?;
    let session_id = sessions
        .as_array()
        .and_then(|list| list.iter().find(|s| s["title"] == serde_json::json!(title)))
        .and_then(|s| s["id"].as_str())
        .map(|s| s.to_string());
    let session_id = match session_id {
        Some(id) => id,
        None => {
            let r: serde_json::Value = client
                .post(format!("{base}/api/sessions"))
                .header("X-Auth-Key", &link.api_key)
                .json(&serde_json::json!({ "title": title }))
                .send()
                .await?
                .json()
                .await
                .context("会话创建失败")?;
            r["id"].as_str().context("会话创建响应缺少 id")?.to_string()
        }
    };

    // 2) 记录当前消息数（用于识别新回复），发起对话
    let before: serde_json::Value = client
        .get(format!("{base}/api/sessions/{session_id}/messages"))
        .header("X-Auth-Key", &link.api_key)
        .send()
        .await?
        .json()
        .await
        .unwrap_or(serde_json::Value::Null);
    let before_count = before.as_array().map(|a| a.len()).unwrap_or(0);

    let r = client
        .post(format!("{base}/api/sessions/{session_id}/chat"))
        .header("X-Auth-Key", &link.api_key)
        .json(&serde_json::json!({ "text": prompt }))
        .send()
        .await?;
    if r.status().as_u16() >= 400 {
        let body = r.text().await.unwrap_or_default();
        anyhow::bail!("对端拒绝对话: {}", body.chars().take(200).collect::<String>());
    }

    // 3) 轮询等待新回复（对端收束后会有新 orchestrator 消息）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    let poll_interval = std::time::Duration::from_secs(3);
    loop {
        tokio::time::sleep(poll_interval).await;
        if std::time::Instant::now() > deadline {
            anyhow::bail!("连结体执行超时（{}s）", DEFAULT_TIMEOUT_SECS);
        }
        let msgs: serde_json::Value = client
            .get(format!("{base}/api/sessions/{session_id}/messages"))
            .header("X-Auth-Key", &link.api_key)
            .send()
            .await?
            .json()
            .await
            .unwrap_or(serde_json::Value::Null);
        let Some(list) = msgs.as_array() else { continue };
        if list.len() <= before_count {
            continue;
        }
        // 取新增消息里最后一条 orchestrator 陈述文本
        let reply = list
            .iter()
            .skip(before_count)
            .rev()
            .find(|m| m["role"] == "orchestrator")
            .and_then(|m| m["statements"].as_array())
            .map(|sts| {
                sts.iter()
                    .filter_map(|s| s["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            });
        if let Some(text) = reply {
            if !text.trim().is_empty() {
                let summary: String = text.chars().take(400).collect();
                return Ok((summary, text));
            }
        }
    }
}

// ---------------------------------------------------------------- CLI 智能体

fn cli_command(link: &LinkTarget) -> Vec<String> {
    if !link.command.trim().is_empty() {
        return link.command.split_whitespace().map(|s| s.to_string()).collect();
    }
    match link.kind.as_str() {
        // 交互式 CLI 的非交互形态：提示词作为位置参数
        "opencode" => vec!["opencode".into(), "run".into()],
        "codex" => vec!["codex".into(), "exec".into()],
        "claude" => vec!["claude".into(), "-p".into()],
        _ => vec![],
    }
}

async fn execute_cli(link: &LinkTarget, prompt: &str) -> Result<(String, String)> {
    let mut cmd = cli_command(link);
    if cmd.is_empty() {
        anyhow::bail!("连结体 {} 未配置命令", link.id);
    }
    let program = cmd.remove(0);
    cmd.push(prompt.to_string());
    let output = tokio::process::Command::new(&program)
        .args(&cmd)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .with_context(|| format!("启动 CLI 智能体失败: {program}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("CLI 智能体退出码 {:?}: {}", output.status.code(), err.chars().take(200).collect::<String>());
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        anyhow::bail!("CLI 智能体无输出");
    }
    let summary: String = text.chars().take(400).collect();
    Ok((summary, text))
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(links: Vec<LinkTarget>) -> ExmConfig {
        let mut cfg = ExmConfig::default();
        cfg.nexus.links = links;
        cfg
    }

    #[tokio::test]
    async fn cli连结_执行与摘要() {
        #[cfg(windows)]
        let cmd = "cmd /c echo";
        #[cfg(not(windows))]
        let cmd = "echo";
        let link = LinkTarget { id: "cli-1".into(), name: "回显连结".into(), kind: "custom".into(), command: cmd.into(), ..Default::default() };
        let cfg = cfg_with(vec![link.clone()]);
        let (summary, output) = execute_link(&cfg, "cli-1", "hello nexus", &[]).await.unwrap();
        assert!(output.contains("hello nexus"), "{output}");
        assert!(!summary.is_empty());
    }

    #[tokio::test]
    async fn cli连结_非零退出报错() {
        #[cfg(windows)]
        let cmd = "cmd /c exit 3";
        #[cfg(not(windows))]
        let cmd = "sh -c \"exit 3\"";
        let link = LinkTarget { id: "cli-bad".into(), name: "失败连结".into(), kind: "custom".into(), command: cmd.into(), ..Default::default() };
        let cfg = cfg_with(vec![link]);
        let r = execute_link(&cfg, "cli-bad", "x", &[]).await;
        assert!(r.is_err());
    }

    #[test]
    fn 连结体不存在或未启用() {
        let cfg = cfg_with(vec![LinkTarget { id: "off".into(), name: "停用".into(), kind: "custom".into(), command: "echo".into(), enabled: false, ..Default::default() }]);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(execute_link(&cfg, "off", "x", &[]));
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("未启用"));
        let r2 = rt.block_on(execute_link(&cfg, "ghost", "x", &[]));
        assert!(r2.is_err());
    }
}
