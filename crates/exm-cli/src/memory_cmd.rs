//! `exm memory` —— 深层记忆的查看、检索、固化与维护（docs/08 §6）

use crate::render::*;
use exm_core::memory::{MemoryDraft, MemoryKind};
use exm_core::Core;
use owo_colors::OwoColorize;

fn parse_kind(s: &str) -> anyhow::Result<MemoryKind> {
    MemoryKind::parse(s).ok_or_else(|| {
        anyhow::anyhow!("未知记忆类型：{s}（可选 fact|decision|preference|evidence|digest|lesson|stat）")
    })
}

pub fn run_memory_list(core: &Core, kind: Option<String>, agent: Option<&str>, limit: usize) -> anyhow::Result<()> {
    let kind = match kind {
        Some(k) => Some(parse_kind(&k)?),
        None => None,
    };
    let entries = core.memory_list_filtered(kind, agent, limit)?;
    if let Some(id) = agent {
        println!("{}", format!("个体 {} 可见的记忆（私有 + 群体共享）", id).bold().to_string());
    }
    if entries.is_empty() {
        println!("{}", dim("（记忆库为空；执行任务或 `exm memory add` 后写入）"));
        return Ok(());
    }
    for e in entries {
        let pin = if e.pinned { "📌 ".to_string() } else { String::new() };
        let layer = if e.agent_id.is_some() {
            "个体".cyan().to_string()
        } else if let Some(g) = &e.group_id {
            format!("组:{g}").purple().to_string()
        } else {
            "全局".blue().to_string()
        };
        println!(
            "{}{} {} {}",
            pin,
            layer,
            e.kind.key().magenta(),
            e.title.bold()
        );
        if let Some(a) = &e.agent_id {
            println!("   {}", dim(format!("归属: {a}")));
        }
        println!(
            "   {}",
            dim(format!("重要性 {:.2}｜置信度 {:.2}｜访问 {}", e.importance, e.confidence, e.access_count))
        );
        println!("   {}", e.body.replace('\n', " ").chars().take(160).collect::<String>());
        println!("   {}", dim(format!("id={}  tags={}  {}", e.id, e.tags.join(","), e.updated_at)));
    }
    Ok(())
}

pub fn run_memory_search(core: &Core, query: &str, agent: Option<&str>, limit: usize) -> anyhow::Result<()> {
    let hits = match agent {
        Some(id) => core.memory_recall_for_agent(id, query, Some(limit))?,
        None => core.memory_recall_all(query, Some(limit))?,
    };
    if hits.is_empty() {
        println!("{}", dim("（无命中）"));
        return Ok(());
    }
    let gid = core.active_group();
    let scope = agent
        .map(|a| format!("个体 {a} 可见范围（私有 + 共享，激活组 {gid}）"))
        .unwrap_or_else(|| format!("全部记忆（激活组 {gid} + 全局条目）"));
    println!("{}", format!("检索「{query}」命中 {} 条（{}）", hits.len(), scope).bold().to_string());
    for h in hits {
        println!(
            "  {} {} {}",
            format!("{:.2}", h.score).green(),
            h.entry.kind.key().cyan(),
            h.entry.title.bold()
        );
        println!("     {}", h.entry.body.replace('\n', " ").chars().take(200).collect::<String>());
        println!("     {}", dim(format!("理由：{}", h.reasons.join("；"))));
    }
    Ok(())
}

pub fn run_memory_add(
    core: &Core,
    kind: &str,
    title: &str,
    body: &str,
    pin: bool,
    agent: Option<&str>,
    tags: Vec<String>,
    importance: f64,
    global: bool,
) -> anyhow::Result<()> {
    let mut draft = MemoryDraft::new(parse_kind(kind)?, title, body).importance(importance);
    draft.tags = tags;
    if let Some(a) = agent {
        draft.agent_id = Some(a.to_string());
    }
    let entry = if global { core.memory_remember_global(draft)? } else { core.memory_remember(draft)? };
    if pin {
        core.memory_pin(&entry.id, true)?;
    }
    let layer = if entry.agent_id.is_some() {
        "个体记忆".to_string()
    } else if let Some(g) = &entry.group_id {
        format!("组记忆（{g}）")
    } else {
        "全局记忆".to_string()
    };
    println!("{} {}（{layer}，id={}）", ok("已记忆"), entry.title, entry.id);
    Ok(())
}

pub fn run_memory_pin(core: &Core, id: &str, pinned: bool) -> anyhow::Result<()> {
    core.memory_pin(id, pinned)?;
    println!("{} {} pinned={pinned}", ok("已更新"), id);
    Ok(())
}

pub fn run_memory_forget(core: &Core, id: &str) -> anyhow::Result<()> {
    core.memory_forget(id)?;
    println!("{} {}", ok("已遗忘"), id);
    Ok(())
}

pub fn run_memory_reindex(core: &Core) -> anyhow::Result<()> {
    let n = core.memory_reindex()?;
    println!("{} 重建倒排索引：{n} 条记忆", ok("完成"));
    Ok(())
}

pub fn run_memory_decay(core: &Core, floor: f64) -> anyhow::Result<()> {
    let n = core.memory_decay(floor)?;
    println!("{} 衰减更新 {n} 条记忆（并重渲染 memory.md）", ok("完成"));
    Ok(())
}

pub fn run_memory_stats(core: &Core) -> anyhow::Result<()> {
    let s = core.memory_stats()?;
    println!("{}", "记忆库统计".bold().to_string());
    println!("  {} {}", dim("库文件"), s["dbPath"]);
    println!("  {} {}", dim("条目总数"), s["total"]);
    println!("  {} {}", dim("组级记忆（含组归属）"), s["groupScoped"]);
    println!("  {} {}", dim("群体记忆"), s["shared"]);
    println!("  {} {}", dim("个体记忆"), s["individual"]);
    println!("  {} {}", dim("固定记忆"), s["pinned"]);
    println!("  {} {}", dim("倒排词项"), s["terms"]);
    if let Some(map) = s["byKind"].as_object() {
        for (k, v) in map {
            println!("  {} {}", dim(format!("类型 {k}")), v);
        }
    }
    println!();
    println!("{}", "个体可靠性统计（自我进化反馈）".bold().to_string());
    let stats = core.memory.agent_stats(20)?;
    if stats.is_empty() {
        println!("  {}", dim("（暂无；执行任务后累积）"));
    }
    for s in stats {
        println!(
            "  {} 执行 {}｜完成 {}｜受阻 {}｜失败 {}｜平均置信度 {:.2}",
            s.agent_id.cyan(),
            s.runs,
            s.done,
            s.blocked,
            s.failed,
            s.avg_confidence
        );
    }
    Ok(())
}

pub fn run_memory_render(core: &Core) -> anyhow::Result<()> {
    let n = core.render_memory_md()?;
    println!("{} memory.md 已重渲染（固定记忆 {n} 条）", ok("完成"));
    Ok(())
}
