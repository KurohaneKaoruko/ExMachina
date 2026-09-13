//! `exm group` / `exm agent` —— 智能体组管理（docs/09）

use crate::render::*;
use exm_core::types::{AgentDefinition, Tier};
use exm_core::Core;
use owo_colors::OwoColorize;

pub fn run_list(core: &Core) -> anyhow::Result<()> {
    let active = core.active_group();
    println!("{}", "智能体组（激活组标注 ●）".bold().to_string());
    for g in core.list_groups() {
        let mark = if g.id == active { "●".green().to_string() } else { " ".to_string() };
        let builtin = if g.builtin { dim("内置") } else { String::new() };
        let primary = g.primary.clone().unwrap_or_else(|| dim("未设置").to_string());
        println!(
            "{} {} {} {} {}",
            mark,
            g.id.cyan(),
            g.name.bold(),
            dim(format!("主智能体: {primary}")),
            builtin
        );
        if !g.description.is_empty() {
            println!("    {}", dim(&g.description));
        }
    }
    Ok(())
}

pub fn run_create(core: &Core, name: &str, id: Option<String>, description: &str) -> anyhow::Result<()> {
    let meta = core.create_group(id, name, description)?;
    println!("{} 组已创建：{}（id={}）", ok("完成"), meta.name, meta.id.cyan());
    println!("{}", dim("提示：组内还没有个体；创建第一个个体时它将自动成为主智能体："));
    println!(
        "  {}  {}",
        dim("1."),
        format!("exm agent create \"<名称>\" --identifier <id> --description \"<职责>\"").cyan()
    );
    println!("  {}  {}", dim("2."), "exm group switch <groupId>".cyan());
    Ok(())
}

pub fn run_switch(core: &Core, id: &str) -> anyhow::Result<()> {
    core.switch_group(id)?;
    let meta = core.group_meta(id);
    println!(
        "{} 激活组已切换：{}",
        ok("完成"),
        meta.map(|m| format!("{}（{}）", m.name, m.id)).unwrap_or_else(|| id.into())
    );
    Ok(())
}

pub fn run_info(core: &Core, id: Option<String>) -> anyhow::Result<()> {
    let gid = id.unwrap_or_else(|| core.active_group());
    let Some(meta) = core.group_meta(&gid) else {
        anyhow::bail!("组不存在: {gid}");
    };
    println!("{}", format!("组 {}（{}）", meta.name, meta.id).bold().to_string());
    println!("  {} {}", dim("内置"), meta.builtin);
    println!("  {} {}", dim("描述"), if meta.description.is_empty() { "-" } else { &meta.description });
    println!("  {} {}", dim("主智能体"), meta.primary.clone().unwrap_or_else(|| "未设置".into()));
    println!("  {} {}", dim("个体数"), core.registry().group_agent_count(&meta.id));
    for a in core.registry().agents_in_group(&meta.id) {
        let role = if Some(a.identifier.as_str()) == meta.primary.as_deref() { "主智能体".on_blue().white().to_string() } else { "子个体".dimmed().to_string() };
        println!("  {} {} {} — {}", role, a.identifier.cyan(), a.name.bold(), dim(&a.description));
    }
    Ok(())
}

pub fn run_export(core: &Core, id: &str, out: &str) -> anyhow::Result<()> {
    let bundle = core.registry().export_group(id)?;
    let text = serde_json::to_string_pretty(&bundle)?;
    if out.trim().is_empty() {
        println!("{text}");
    } else {
        std::fs::write(out.trim(), text)?;
        let n = bundle["agents"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("{} 组已导出：{id}（{} 个个体）→ {}", ok("完成"), n, out.trim());
    }
    Ok(())
}

pub fn run_import(core: &Core, file: &str, new_id: Option<&str>) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(file)?;
    let bundle: serde_json::Value = serde_json::from_str(&raw)?;
    let imported = core.registry().import_group(&bundle, new_id.map(String::from))?;
    let meta = core.group_meta(&imported.id).unwrap_or(imported);
    let n = bundle["agents"].as_array().map(|a| a.len()).unwrap_or(0);
    println!(
        "{} 组已导入：{}（{}，{} 个个体，主智能体 {}）",
        ok("完成"),
        meta.name,
        meta.id.cyan(),
        n,
        meta.primary.unwrap_or_else(|| "未设".into())
    );
    Ok(())
}

pub fn run_delete(core: &Core, id: &str) -> anyhow::Result<()> {
    core.delete_group(id)?;
    println!("{} 组已删除：{id}", ok("完成"));
    Ok(())
}

pub fn run_agent_create(
    core: &Core,
    name: &str,
    identifier: &str,
    description: &str,
    domain: &str,
    tier: &str,
    group: Option<&str>,
    prompt: Option<String>,
) -> anyhow::Result<()> {
    let gid = match group {
        Some(g) => g.to_string(),
        None => core.active_group(),
    };
    let meta = core
        .group_meta(&gid)
        .ok_or_else(|| anyhow::anyhow!("组不存在: {gid}"))?;
    if meta.builtin {
        anyhow::bail!(
            "激活组为内置组（编成受保护）。请先：\n  1. exm group create \"<组名>\" --id <gid>\n  2. exm group switch <gid>\n或用 --group <gid> 指定自定义组"
        );
    }
    let was_empty = core.registry().group_agent_count(&gid) == 0;
    let def = AgentDefinition {
        name: name.to_string(),
        identifier: identifier.to_string(),
        domain: domain.to_string(),
        tier: if tier == "orchestrator" { Tier::Orchestrator } else { Tier::Unit },
        description: description.to_string(),
        capabilities: vec![],
        tools: vec![],
        when_to_call: description.to_string(),
        dependencies: vec![],
        composable_with: vec![],
        input_schema: Default::default(),
        output_schema: Default::default(),
        prompt_file: String::new(),
        model_hint: None,
    };
    let saved = core.registry().upsert_agent(&gid, def, prompt)?;
    if was_empty || tier == "orchestrator" {
        core.registry().set_primary(&gid, &saved.identifier)?;
        println!("{} 已设为组内主智能体（直接对接用户，拥有组内管理权限）", ok("完成"));
    }
    println!("{} 个体已创建：{}（{}，组 {gid}）", ok("完成"), saved.name, saved.identifier.cyan());
    if core.active_group() != gid {
        println!("{}", dim(format!("提示：当前激活组为 {}，需切换后才能对话：exm group switch {gid}", core.active_group())));
    }
    Ok(())
}

pub fn run_agent_remove(core: &Core, identifier: &str, group: Option<&str>) -> anyhow::Result<()> {
    let gid = group.map(String::from).unwrap_or_else(|| core.active_group());
    core.registry().remove_agent(&gid, identifier)?;
    println!("{} 个体已删除：{identifier}", ok("完成"));
    Ok(())
}

pub fn run_agent_set_primary(core: &Core, identifier: &str, group: Option<&str>) -> anyhow::Result<()> {
    let gid = group.map(String::from).unwrap_or_else(|| core.active_group());
    core.registry().set_primary(&gid, identifier)?;
    println!("{} 主智能体已设置为：{identifier}", ok("完成"));
    Ok(())
}
