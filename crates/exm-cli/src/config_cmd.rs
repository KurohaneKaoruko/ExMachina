//! `exm config` —— 配置查看/修改/schema 输出（CLI 与 WebUI 同源）

use crate::render::*;
use exm_core::config::{config_schema, ExmConfig};
use owo_colors::OwoColorize;
use std::path::Path;

pub fn run_config_list(workspace_root: &Path) -> anyhow::Result<()> {
    let cfg = ExmConfig::load(workspace_root);
    println!("{}", "当前配置".bold().to_string());
    println!("  {} {}", dim("配置文件"), cfg.config_path.display());
    println!("  {} {}", dim("结构版本"), cfg.config_version);
    println!("  {} {}", dim("LLM Base URL"), cfg.llm.base_url);
    println!("  {} {}", dim("API Key"), if cfg.llm.api_key.is_empty() { "(未配置 → Mock 通道)".to_string() } else { "***已配置***".into() });
    println!("  {} {}", dim("指挥体模型"), cfg.llm.orch_model);
    println!("  {} {}", dim("子个体模型"), cfg.llm.unit_model);
    println!("  {} {}", dim("并发数"), cfg.max_concurrency);
    println!("  {} {}", dim("记忆系统"), if cfg.memory_enabled { "启用" } else { "关闭" });
    println!("  {} {}", dim("召回条数"), cfg.memory_recall_limit);
    println!("  {} {}", dim("衰减半衰期(天)"), cfg.memory_half_life_days);
    println!("  {} {}", dim("工作区"), cfg.workspace_root.display());
    println!("  {} {}", dim("编成目录"), cfg.agents_dir.display());
    if let Some(m) = cfg.load_manifest() {
        println!("  {} {}（{}，安装于 {}）", dim("安装清单"), m.profile, m.channels.gateway_port, m.installed_at);
    }
    Ok(())
}

pub fn run_config_get(workspace_root: &Path, key: &str) -> anyhow::Result<()> {
    let cfg = ExmConfig::load(workspace_root);
    let value = match key {
        "llm.baseUrl" => cfg.llm.base_url.clone(),
        "llm.apiKey" => if cfg.llm.api_key.is_empty() { "".into() } else { "***已配置***".into() },
        "llm.orchModel" => cfg.llm.orch_model.clone(),
        "llm.unitModel" => cfg.llm.unit_model.clone(),
        "maxConcurrency" => cfg.max_concurrency.to_string(),
        "memory.enabled" => cfg.memory_enabled.to_string(),
        "memory.recallLimit" => cfg.memory_recall_limit.to_string(),
        "memory.halfLifeDays" => cfg.memory_half_life_days.to_string(),
        other => anyhow::bail!("未知配置键：{other}（可用 `exm config schema` 查看全部键）"),
    };
    println!("{value}");
    Ok(())
}

pub fn run_config_set(workspace_root: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    let mut cfg = ExmConfig::load(workspace_root);
    match key {
        "llm.baseUrl" => cfg.llm.base_url = value.to_string(),
        "llm.apiKey" => cfg.llm.api_key = value.to_string(),
        "llm.orchModel" => cfg.llm.orch_model = value.to_string(),
        "llm.unitModel" => cfg.llm.unit_model = value.to_string(),
        "maxConcurrency" => cfg.max_concurrency = value.parse()?,
        "memory.enabled" => cfg.memory_enabled = matches!(value, "true" | "1" | "yes"),
        "memory.recallLimit" => cfg.memory_recall_limit = value.parse()?,
        "memory.halfLifeDays" => cfg.memory_half_life_days = value.parse()?,
        other => anyhow::bail!("未知配置键：{other}"),
    }
    cfg.use_mock = cfg.llm.api_key.trim().is_empty();
    cfg.save()?;
    println!("{} {} = {}", ok("已写入"), key, if key.ends_with("apiKey") { "***" } else { value });
    println!("{}", dim("提示：正在运行的网关/会话需重新加载配置（WebUI 设置页可热生效）"));
    Ok(())
}

pub fn run_config_schema() -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&config_schema())?);
    Ok(())
}
