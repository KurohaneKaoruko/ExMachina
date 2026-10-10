//! `exm config` —— 配置查看/修改/schema 输出（CLI 与 WebUI 同源，schema 驱动全键覆盖）
//!
//! 键口径与 config_schema() 完全一致（`组.字段` 或顶层字段，共 63 键）：
//! - `exm config`             摘要视图
//! - `exm config get <key>`   读单键（password 类打码）
//! - `exm config set <key> <value>`  写单键（按 schema kind 类型归一：boolean/number/string）
//! - `exm config schema`      输出完整 schema
//!
//! 写入 = 修改配置文件（cfg.save）。运行中的网关在内存里持有旧值，需重启生效；
//! 即时生效口径请用 WebUI / 桌面端设置页（PUT /config）。

use crate::render::*;
use exm_core::config::{config_schema, ExmConfig};
use owo_colors::OwoColorize;
use std::path::Path;

/// schema 全部字段：(key, kind, 组label)
fn schema_fields() -> Vec<(String, String, String)> {
    let schema = config_schema();
    let mut out = Vec::new();
    if let Some(groups) = schema.get("groups").and_then(|v| v.as_array()) {
        for g in groups {
            let label = g.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(fields) = g.get("fields").and_then(|v| v.as_array()) {
                for f in fields {
                    out.push((
                        f.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        f.get("kind").and_then(|v| v.as_str()).unwrap_or("string").to_string(),
                        label.clone(),
                    ));
                }
            }
        }
    }
    out
}

fn kind_of(key: &str) -> Option<(String, String)> {
    schema_fields().into_iter().find(|(k, kind, _)| k == key).map(|(_, kind, label)| (kind, label))
}

fn parse_bool(v: &str) -> anyhow::Result<bool> {
    match v.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(anyhow::anyhow!("布尔值应为 true / false（收到：{other}）")),
    }
}

pub fn run_config_list(workspace_root: &Path) -> anyhow::Result<()> {
    let cfg = ExmConfig::load(workspace_root);
    println!("{}", "当前配置".bold().to_string());
    println!("  {} {}", dim("配置文件"), cfg.config_path.display());
    println!("  {} {}", dim("结构版本"), cfg.config_version);
    println!("  {} {}", dim("LLM Base URL"), cfg.llm.base_url);
    println!("  {} {}", dim("API Key"), if cfg.llm.api_key.is_empty() { "(未配置 —— 请在「提供商」页配置)".to_string() } else { "***已配置***".into() });
    println!("  {} {}", dim("默认模型"), cfg.llm.model);
    println!("  {} {}", dim("并发数"), cfg.max_concurrency);
    println!("  {} {}", dim("记忆系统"), if cfg.memory_enabled { "启用" } else { "关闭" });
    println!("  {} {}", dim("召回条数"), cfg.memory_recall_limit);
    println!("  {} {}", dim("衰减半衰期(天)"), cfg.memory_half_life_days);
    println!("  {} {}", dim("语义检索模型"), if cfg.memory_semantic_model.is_empty() { "(未配置 → 仅词项召回)".to_string() } else { cfg.memory_semantic_model.clone() });
    println!("  {} {}", dim("工作区"), cfg.workspace_root.display());
    println!("  {} {}", dim("编成目录"), cfg.agents_dir.display());
    if let Some(m) = cfg.load_manifest() {
        println!("  {} {}（{}，安装于 {}）", dim("安装清单"), m.profile, m.channels.gateway_port, m.installed_at);
    }
    println!("\n  {} `exm config schema` 查看全部 63 键；`exm config get/set <键> <值>` 读写单键", dim("提示"));
    Ok(())
}

pub fn run_config_schema() -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&config_schema())?);
    Ok(())
}

/// 按键读值（password 类打码；与 WebUI 掩码口径一致）
fn cfg_read(cfg: &ExmConfig, key: &str) -> anyhow::Result<String> {
    let v = match key {
        "llm.baseUrl" => cfg.llm.base_url.clone(),
        "llm.apiKey" => if cfg.llm.api_key.is_empty() { "(未配置)".into() } else { "***已配置***".into() },
        "llm.model" => cfg.llm.model.clone(),
        "maxConcurrency" => cfg.max_concurrency.to_string(),
        "maxSessionTokens" => cfg.max_session_tokens.to_string(),
        "memory.enabled" => cfg.memory_enabled.to_string(),
        "memory.mdMaxChars" => cfg.memory_md_max_chars.to_string(),
        "memory.recallLimit" => cfg.memory_recall_limit.to_string(),
        "memory.halfLifeDays" => cfg.memory_half_life_days.to_string(),
        "security.execApproval" => cfg.security.exec_approval.clone(),
        "security.execAllowlist" => cfg.security.exec_allowlist.join(","),
        "security.authKey" => if cfg.security.auth_key.is_empty() { "(未配置)".into() } else { "***已配置***".into() },
        "security.terminalTimeoutSecs" => cfg.security.terminal_timeout_secs.to_string(),
        "security.toolOutputSpillChars" => cfg.security.tool_output_spill_chars.to_string(),
        "security.approvalWaitSecs" => cfg.security.approval_wait_secs.to_string(),
        "computer.enabled" => cfg.computer.enabled.to_string(),
        "computer.approval" => cfg.computer.approval.to_string(),
        "computer.maxEdge" => cfg.computer.max_edge.to_string(),
        "computer.actionIntervalMs" => cfg.computer.action_interval_ms.to_string(),
        "automation.heartbeatEnabled" => cfg.automation.heartbeat_enabled.to_string(),
        "automation.heartbeatIntervalMinutes" => cfg.automation.heartbeat_interval_minutes.to_string(),
        "automation.autoAdapt" => cfg.automation.auto_adapt.to_string(),
        "automation.heartbeatPrompt" => cfg.automation.heartbeat_prompt.clone(),
        "automation.heartbeatAgent" => cfg.automation.heartbeat_agent.clone().unwrap_or_default(),
        "automation.unitMaxSteps" => cfg.automation.unit_max_steps.to_string(),
        "search.provider" => cfg.search.provider.clone(),
        "search.endpoint" => cfg.search.endpoint.clone(),
        "search.apiKey" => if cfg.search.api_key.is_empty() { "(未配置)".into() } else { "***已配置***".into() },
        "search.maxResults" => cfg.search.max_results.to_string(),
        "sandbox.mode" => cfg.sandbox.mode.clone(),
        "sandbox.allowNetwork" => cfg.sandbox.allow_network.to_string(),
        "sandbox.useBwrap" => cfg.sandbox.use_bwrap.to_string(),
        "sandbox.memoryMb" => cfg.sandbox.memory_mb.to_string(),
        "sandbox.maxProcesses" => cfg.sandbox.max_processes.to_string(),
        "browser.executable" => cfg.browser.executable.clone(),
        "browser.headless" => cfg.browser.headless.to_string(),
        "browser.timeoutSecs" => cfg.browser.timeout_secs.to_string(),
        "browser.maxChars" => cfg.browser.max_chars.to_string(),
        "tools.custom" => cfg.tools.custom.iter().map(|t| serde_json::to_string(t).unwrap_or_default()).collect::<Vec<_>>().join("\n"),
        "hooks.preTool" => cfg.hooks.pre_tool.join(","),
        "hooks.postTool" => cfg.hooks.post_tool.join(","),
        "hooks.onRunEnd" => cfg.hooks.on_run_end.join(","),
        "identity.identityRequired" => cfg.identity.identity_required.to_string(),
        "identity.pairingTtlSecs" => cfg.identity.pairing_ttl_secs.to_string(),
        "identity.groupGateDefault" => cfg.identity.group_gate_default.to_string(),
        "limits.enabled" => cfg.limits.enabled.to_string(),
        "limits.windowSecs" => cfg.limits.window_secs.to_string(),
        "limits.maxRequests" => cfg.limits.max_requests.to_string(),
        "limits.quotaPeriod" => cfg.limits.quota_period.clone(),
        "limits.quotaTokens" => cfg.limits.quota_tokens.to_string(),
        "limits.quotaRequests" => cfg.limits.quota_requests.to_string(),
        "limits.quotaRunningPolicy" => cfg.limits.quota_running_policy.clone(),
        "triggers.fileWatch.enabled" => cfg.triggers.file_watch.enabled.to_string(),
        "triggers.fileWatch.debounceMs" => cfg.triggers.file_watch.debounce_ms.to_string(),
        "triggers.fileWatch.exclude" => cfg.triggers.file_watch.exclude.join(","),
        "triggers.eventWebhook.enabled" => cfg.triggers.event_webhook.enabled.to_string(),
        "triggers.eventWebhook.secret" => if cfg.triggers.event_webhook.secret.is_empty() { "(未配置)".into() } else { "***已配置***".into() },
        "mcpServe.enabled" => cfg.mcp_serve.enabled.to_string(),
        "mcpServe.httpPath" => cfg.mcp_serve.http_path.clone(),
        "mcpServe.allowedGroups" => cfg.mcp_serve.allowed_groups.join(","),
        "mcpServe.allowedTools" => cfg.mcp_serve.allowed_tools.join(","),
        "notify.enabled" => cfg.notify.enabled.to_string(),
        "notify.maxChars" => cfg.notify.max_chars.to_string(),
        other => anyhow::bail!("未知配置键：{other}（`exm config schema` 查看全部键）"),
    };
    Ok(v)
}

pub fn run_config_get(workspace_root: &Path, key: &str) -> anyhow::Result<()> {
    let cfg = ExmConfig::load(workspace_root);
    println!("{}", cfg_read(&cfg, key)?);
    Ok(())
}

/// 按键写值（值类型按 schema kind 归一），返回是否实际变更
fn cfg_write(cfg: &mut ExmConfig, key: &str, value: &str) -> anyhow::Result<()> {
    let (kind, _label) = kind_of(key).ok_or_else(|| anyhow::anyhow!("未知配置键：{key}（`exm config schema` 查看全部键）"))?;
    let list = |v: &str| -> Vec<String> { v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() };
    match key {
        // llm.* 顶层是种子值；存在生效档案时它才是真正生效值——双写保持一致（与 load 口径对齐）
        "llm.baseUrl" | "llm.apiKey" | "llm.model" => {
            let field = key.split('.').next_back().unwrap();
            match field {
                "baseUrl" => cfg.llm.base_url = value.to_string(),
                "apiKey" => cfg.llm.api_key = value.to_string(),
                "model" => cfg.llm.model = value.to_string(),
                _ => unreachable!(),
            }
            if let Some(p) = cfg.llm_profiles.iter_mut().find(|p| p.id == cfg.active_profile) {
                match field {
                    "baseUrl" => p.base_url = value.to_string(),
                    "apiKey" => p.api_key = value.to_string(),
                    "model" => p.model = value.to_string(),
                    _ => unreachable!(),
                }
            }
        }
        "maxConcurrency" => cfg.max_concurrency = value.parse()?,
        "maxSessionTokens" => cfg.max_session_tokens = value.parse()?,
        "memory.enabled" => cfg.memory_enabled = parse_bool(value)?,
        "memory.mdMaxChars" => cfg.memory_md_max_chars = value.parse()?,
        "memory.recallLimit" => cfg.memory_recall_limit = value.parse()?,
        "memory.halfLifeDays" => cfg.memory_half_life_days = value.parse()?,
        "security.execApproval" => cfg.security.exec_approval = value.to_string(),
        "security.execAllowlist" => cfg.security.exec_allowlist = list(value),
        "security.authKey" => cfg.security.auth_key = value.to_string(),
        "security.terminalTimeoutSecs" => cfg.security.terminal_timeout_secs = value.parse()?,
        "security.toolOutputSpillChars" => cfg.security.tool_output_spill_chars = value.parse()?,
        "security.approvalWaitSecs" => cfg.security.approval_wait_secs = value.parse()?,
        "computer.enabled" => cfg.computer.enabled = parse_bool(value)?,
        "computer.approval" => cfg.computer.approval = parse_bool(value)?,
        "computer.maxEdge" => cfg.computer.max_edge = value.parse()?,
        "computer.actionIntervalMs" => cfg.computer.action_interval_ms = value.parse()?,
        "automation.heartbeatEnabled" => cfg.automation.heartbeat_enabled = parse_bool(value)?,
        "automation.heartbeatIntervalMinutes" => cfg.automation.heartbeat_interval_minutes = value.parse()?,
        "automation.autoAdapt" => cfg.automation.auto_adapt = parse_bool(value)?,
        "automation.heartbeatPrompt" => cfg.automation.heartbeat_prompt = value.to_string(),
        "automation.heartbeatAgent" => cfg.automation.heartbeat_agent = Some(value.to_string()).filter(|s| !s.is_empty()),
        "automation.unitMaxSteps" => cfg.automation.unit_max_steps = value.parse()?,
        "search.provider" => cfg.search.provider = value.to_string(),
        "search.endpoint" => cfg.search.endpoint = value.to_string(),
        "search.apiKey" => cfg.search.api_key = value.to_string(),
        "search.maxResults" => cfg.search.max_results = value.parse()?,
        "sandbox.mode" => cfg.sandbox.mode = value.to_string(),
        "sandbox.allowNetwork" => cfg.sandbox.allow_network = parse_bool(value)?,
        "sandbox.useBwrap" => cfg.sandbox.use_bwrap = parse_bool(value)?,
        "sandbox.memoryMb" => cfg.sandbox.memory_mb = value.parse()?,
        "sandbox.maxProcesses" => cfg.sandbox.max_processes = value.parse()?,
        "browser.executable" => cfg.browser.executable = value.to_string(),
        "browser.headless" => cfg.browser.headless = parse_bool(value)?,
        "browser.timeoutSecs" => cfg.browser.timeout_secs = value.parse()?,
        "browser.maxChars" => cfg.browser.max_chars = value.parse()?,
        "tools.custom" => { /* 自定义工具为 JSON 契约，CLI 直改易损坏——请用配置页（schema 渲染校验） */ anyhow::bail!("tools.custom 为结构化契约，请用 WebUI / 桌面端配置页修改"); }
        "hooks.preTool" => cfg.hooks.pre_tool = list(value),
        "hooks.postTool" => cfg.hooks.post_tool = list(value),
        "hooks.onRunEnd" => cfg.hooks.on_run_end = list(value),
        "identity.identityRequired" => cfg.identity.identity_required = parse_bool(value)?,
        "identity.pairingTtlSecs" => cfg.identity.pairing_ttl_secs = value.parse()?,
        "identity.groupGateDefault" => cfg.identity.group_gate_default = parse_bool(value)?,
        "limits.enabled" => cfg.limits.enabled = parse_bool(value)?,
        "limits.windowSecs" => cfg.limits.window_secs = value.parse()?,
        "limits.maxRequests" => cfg.limits.max_requests = value.parse()?,
        "limits.quotaPeriod" => cfg.limits.quota_period = value.to_string(),
        "limits.quotaTokens" => cfg.limits.quota_tokens = value.parse()?,
        "limits.quotaRequests" => cfg.limits.quota_requests = value.parse()?,
        "limits.quotaRunningPolicy" => cfg.limits.quota_running_policy = value.to_string(),
        "triggers.fileWatch.enabled" => cfg.triggers.file_watch.enabled = parse_bool(value)?,
        "triggers.fileWatch.debounceMs" => cfg.triggers.file_watch.debounce_ms = value.parse()?,
        "triggers.fileWatch.exclude" => cfg.triggers.file_watch.exclude = list(value),
        "triggers.eventWebhook.enabled" => cfg.triggers.event_webhook.enabled = parse_bool(value)?,
        "triggers.eventWebhook.secret" => cfg.triggers.event_webhook.secret = value.to_string(),
        "mcpServe.enabled" => cfg.mcp_serve.enabled = parse_bool(value)?,
        "mcpServe.httpPath" => cfg.mcp_serve.http_path = value.to_string(),
        "mcpServe.allowedGroups" => cfg.mcp_serve.allowed_groups = list(value),
        "mcpServe.allowedTools" => cfg.mcp_serve.allowed_tools = list(value),
        "notify.enabled" => cfg.notify.enabled = parse_bool(value)?,
        "notify.maxChars" => cfg.notify.max_chars = value.parse()?,
        _ => unreachable!("schema 与写入分支不同步：{key}"),
    }
    let _ = kind;
    Ok(())
}

pub fn run_config_set(workspace_root: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    let mut cfg = ExmConfig::load(workspace_root);
    cfg_write(&mut cfg, key, value)?;
    cfg.use_mock = cfg.use_mock || exm_core::config::mock_enabled_from_env();
    cfg.save()?;
    println!("{} {} = {}", "✓ 已写入".green(), key.bright_cyan(), if kind_of(key).map(|(k, _)| k == "password").unwrap_or(false) && !value.is_empty() { "***已配置***".into() } else { value.to_string().bright_cyan().to_string() });

    // 运行中的网关持有内存态旧值：提示重启或改用 UI（PUT /config 即时生效）
    if let Some(m) = cfg.load_manifest() {
        let port = m.channels.gateway_port;
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            println!("{}", format!("⚠ 检测到网关运行中（端口 {port}）：本次修改需重启网关后生效；即时生效请用 WebUI / 桌面端设置页（PUT /config）").yellow());
        }
    }
    Ok(())
}
