//! `exm skill` / `exm cron` / `exm approval` —— 平台能力命令（docs/10）

use crate::render::*;
use exm_core::types::{ApprovalRequest, CronJob, SkillDef};
use exm_core::Core;
use owo_colors::OwoColorize;

// ---------------------------------------------------------------- 技能包

pub fn run_skill_list(core: &Core) -> anyhow::Result<()> {
    let skills = core.skills()?;
    if skills.is_empty() {
        println!(
            "{}",
            dim("（当前组无技能包；exm skill add <id> --name <名称> --instructions <指令> 创建）")
        );
        return Ok(());
    }
    println!("{}", format!("技能包（全体组共用，共 {} 个）", skills.len()).bold().to_string());
    for s in skills {
        let agents = if s.agents.is_empty() { "全体".to_string() } else { s.agents.join("、") };
        println!(
            "  {} {} {}",
            s.id.cyan(),
            s.name.bold(),
            dim(format!("适用 [{}]", agents))
        );
        if !s.description.is_empty() {
            println!("    {}", dim(&s.description));
        }
        println!(
            "    {} {}",
            dim("触发:"),
            if s.triggers.is_empty() { "—".to_string() } else { s.triggers.join("、") }
        );
        println!("    {}", s.instructions.chars().take(120).collect::<String>());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_skill_add(
    core: &Core,
    id: &str,
    name: &str,
    description: &str,
    instructions: &str,
    triggers: Vec<String>,
    agents: Vec<String>,
) -> anyhow::Result<()> {
    let skill = SkillDef {
        id: id.trim().to_string(),
        name: name.trim().to_string(),
        description: description.trim().to_string(),
        triggers,
        instructions: instructions.trim().to_string(),
        agents,
        created_at: exm_core::types::now_iso(),
    };
    core.add_skill(skill)?;
    println!("{} 技能包已创建：{}（全体组共用）", ok("完成"), id.cyan());
    println!("{}", dim("派发时任务目标命中触发词即自动携带该指令；热装载，无需重启。"));
    Ok(())
}

pub fn run_skill_remove(core: &Core, id: &str) -> anyhow::Result<()> {
    if core.remove_skill(id)? {
        println!("{} 技能包已删除：{id}", ok("完成"));
    } else {
        anyhow::bail!("技能包不存在: {id}");
    }
    Ok(())
}

// ---------------------------------------------------------------- 经验优化（子个体自适应）

pub async fn run_agent_optimize(core: &Core, identifier: &str) -> anyhow::Result<()> {
    println!("{}", dim(format!("基于历史教训合成经验改进要点（{}）…", identifier)));
    let a = core.orchestrator().optimize_agent(identifier).await?;
    println!(
        "{} {} 已更新至 rev{}（教训计入 {} 条）",
        ok("完成"),
        identifier.cyan(),
        a.revision,
        a.lessons_seen
    );
    println!("{}", a.content);
    println!("{}", dim("要点将常驻注入该个体的后续派发；agent reset-adaptation 可重置。"));
    Ok(())
}

pub fn run_agent_adaptation(core: &Core, identifier: &str) -> anyhow::Result<()> {
    match core.registry().load_adaptation(identifier)? {
        Some(a) => {
            println!(
                "{} 经验改进要点 rev{}（更新于 {}，教训计入 {} 条）",
                identifier.cyan(),
                a.revision,
                dim(&a.updated_at),
                a.lessons_seen
            );
            if let Some(b) = &a.basis {
                println!("  {} {}", dim("依据"), b);
            }
            println!("{}", a.content);
            if !a.previous.is_empty() {
                println!("  {} {}", dim("历史版本"), format!("{:?}", a.previous.iter().map(|v| v.revision).collect::<Vec<_>>()));
            }
        }
        None => println!("{}", dim("该个体尚无经验要点；agent optimize <id> 可手动合成。")),
    }
    Ok(())
}

pub fn run_agent_reset_adaptation(core: &Core, identifier: &str) -> anyhow::Result<()> {
    if core.registry().reset_adaptation(identifier)? {
        println!("{} 经验要点已重置：{identifier}", ok("完成"));
    } else {
        anyhow::bail!("该个体尚无经验要点");
    }
    Ok(())
}

// ---------------------------------------------------------------- 模型档案（多厂商）

fn masked(key: &str) -> String {
    if key.trim().is_empty() { "（空 = Mock 通道）".to_string() } else { "已配置".to_string() }
}

pub fn run_model_list(core: &Core) -> anyhow::Result<()> {
    let cfg = core.config();
    println!(
        "{}",
        format!("模型档案（生效：{}）", cfg.active_profile.cyan()).bold().to_string()
    );
    for p in &cfg.llm_profiles {
        let mark = if p.id == cfg.active_profile { "●".green().to_string() } else { " ".into() };
        println!(
            "{} {} {} {}",
            mark,
            p.id.cyan(),
            p.name.bold(),
            dim(format!("{}｜{} / {}", p.base_url, p.orch_model, p.unit_model))
        );
        let key_count = p.api_keys.len().max(if p.api_key.is_empty() { 0 } else { 1 });
        println!("    {}", dim(format!("apiKey: {}｜Key 池 {} 把", masked(&p.api_key), key_count)));
    }
    println!("{}", dim(format!("通道：{}", if cfg.use_mock { "Mock 模拟（apiKey 为空）" } else { "真实推理" })));
    Ok(())
}

pub fn run_model_add(
    core: &Core,
    id: &str,
    name: &str,
    base_url: &str,
    api_key: &str,
    orch_model: &str,
    unit_model: &str,
) -> anyhow::Result<()> {
    use exm_core::config::LlmProfile;
    let mut cfg = (*core.config()).clone();
    let profile = LlmProfile {
        fallback: None,
        embed_model: None,
        id: id.trim().to_string(),
        name: name.trim().to_string(),
        base_url: base_url.trim().to_string(),
        api_key: api_key.trim().to_string(),
        api_keys: Vec::new(),
        api_format: String::new(),
        orch_model: orch_model.trim().to_string(),
        unit_model: unit_model.trim().to_string(),
    };
    if let Some(slot) = cfg.llm_profiles.iter_mut().find(|p| p.id == profile.id) {
        *slot = profile.clone();
    } else {
        cfg.llm_profiles.push(profile.clone());
    }
    if cfg.active_profile == profile.id {
        cfg.llm = exm_core::config::LlmConfig {
            base_url: profile.base_url,
            api_key: profile.api_key,
            api_keys: Vec::new(),
            api_format: profile.api_format.clone(),
            orch_model: profile.orch_model,
            unit_model: profile.unit_model,
        };
    }
    cfg.use_mock = cfg.llm.api_key.trim().is_empty() && cfg.llm.api_keys.is_empty();
    core.apply_config(cfg)?;
    println!("{} 模型档案已保存：{}", ok("完成"), id.cyan());
    Ok(())
}

pub fn run_model_use(core: &Core, id: &str) -> anyhow::Result<()> {
    let mut cfg = (*core.config()).clone();
    let Some(p) = cfg.llm_profiles.iter().find(|p| p.id == id).cloned() else {
        anyhow::bail!("档案不存在: {id}");
    };
    cfg.active_profile = p.id.clone();
    let mut keys = p.api_keys.clone();
    if keys.is_empty() && !p.api_key.trim().is_empty() {
        keys.push(p.api_key.clone());
    }
    cfg.llm = exm_core::config::LlmConfig {
        base_url: p.base_url,
        api_key: p.api_key,
        api_keys: keys,
        api_format: p.api_format.clone(),
        orch_model: p.orch_model,
        unit_model: p.unit_model,
    };
    cfg.use_mock = cfg.llm.api_key.trim().is_empty() && cfg.llm.api_keys.is_empty();
    let base = cfg.llm.base_url.clone();
    core.apply_config(cfg)?;
    println!("{} 已切换到档案 {}（{}）", ok("完成"), id.cyan(), dim(&base));
    Ok(())
}

pub fn run_model_remove(core: &Core, id: &str) -> anyhow::Result<()> {
    let mut cfg = (*core.config()).clone();
    if cfg.llm_profiles.len() <= 1 {
        anyhow::bail!("至少保留一个模型档案");
    }
    let Some(pos) = cfg.llm_profiles.iter().position(|p| p.id == id) else {
        anyhow::bail!("档案不存在: {id}");
    };
    cfg.llm_profiles.remove(pos);
    if cfg.active_profile == id {
        let first = cfg.llm_profiles[0].clone();
        cfg.active_profile = first.id.clone();
        let mut keys = first.api_keys.clone();
        if keys.is_empty() && !first.api_key.trim().is_empty() {
            keys.push(first.api_key.clone());
        }
        cfg.llm = exm_core::config::LlmConfig {
            base_url: first.base_url.clone(),
            api_key: first.api_key.clone(),
            api_keys: keys,
            api_format: first.api_format.clone(),
            orch_model: first.orch_model.clone(),
            unit_model: first.unit_model.clone(),
        };
    }
    cfg.use_mock = cfg.llm.api_key.trim().is_empty() && cfg.llm.api_keys.is_empty();
    core.apply_config(cfg)?;
    println!("{} 模型档案已删除：{id}", ok("完成"));
    Ok(())
}

pub async fn run_model_test(core: &Core, id: Option<&str>) -> anyhow::Result<()> {
    let cfg = core.config();
    let profile = match id {
        Some(x) => cfg.llm_profiles.iter().find(|p| p.id == x).cloned(),
        None => cfg.llm_profiles.iter().find(|p| p.id == cfg.active_profile).cloned(),
    };
    let Some(p) = profile else {
        anyhow::bail!("档案不存在");
    };
    println!("{}", dim(format!("测试 {}（{}）…", p.id, p.base_url)));
    use exm_core::provider::LlmProvider;
    let mut keys = p.api_keys.clone();
    if keys.is_empty() && !p.api_key.trim().is_empty() {
        keys.push(p.api_key.clone());
    }
    let provider: std::sync::Arc<dyn LlmProvider> = std::sync::Arc::new(
        exm_core::provider::OpenAiCompatibleProvider::new(p.base_url.clone(), keys),
    );
    let req = exm_core::provider::ChatRequest::new(
        p.orch_model.clone(),
        vec![exm_core::provider::ChatMessage::user("ping".to_string())],
    );
    match tokio::time::timeout(std::time::Duration::from_secs(15), provider.chat(req)).await {
        Ok(Ok(resp)) => println!(
            "{} 连通正常：模型 {} 返回 {} 字符",
            ok("完成"),
            p.orch_model,
            resp.content.chars().count()
        ),
        Ok(Err(e)) => println!("{} 连通失败：{e}", err("失败")),
        Err(_) => println!("{} 连接超时（15s）", err("失败")),
    }
    Ok(())
}

// ---------------------------------------------------------------- 定时任务

pub fn run_cron_list(core: &Core) -> anyhow::Result<()> {
    let jobs = core.cron.list()?;
    if jobs.is_empty() {
        println!("{}", dim("（无定时任务；exm cron add <名称> --prompt <提示词> --cron \"分 时 日 月 周\" 创建）"));
        return Ok(());
    }
    println!("{}", "定时任务（网关 serve 常驻时自动调度）".bold().to_string());
    for j in jobs {
        let state = if j.enabled { "启用".green().to_string() } else { "停用".dimmed().to_string() };
        let schedule = match (&j.cron, &j.at) {
            (Some(c), _) => format!("cron[{c}]"),
            (None, Some(a)) => format!("一次性[{a}]"),
            _ => "—".into(),
        };
        println!(
            "  {} {} {} {} {}",
            j.id.cyan(),
            j.name.bold(),
            state,
            schedule,
            dim(format!(
                "上次: {}",
                j.last_run_at.clone().unwrap_or_else(|| "未运行".into())
            ))
        );
        println!("    {}", dim(format!("组: {}｜会话: {}", j.group.as_deref().unwrap_or("(激活组)"), j.session_title.as_deref().unwrap_or("job-<id>"))));
        println!("    {}", j.prompt.chars().take(100).collect::<String>());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_cron_add(
    core: &Core,
    name: &str,
    prompt: &str,
    cron: Option<String>,
    at: Option<String>,
    group: Option<String>,
    session_title: Option<String>,
) -> anyhow::Result<()> {
    if cron.is_none() && at.is_none() {
        anyhow::bail!("需要 --cron \"分 时 日 月 周\" 或 --at <ISO8601 时间>");
    }
    let job = core.cron.upsert(CronJob {
        id: String::new(),
        name: name.trim().to_string(),
        prompt: prompt.trim().to_string(),
        cron,
        at,
        group,
        session_title,
        enabled: true,
        last_run_at: None,
        last_status: None,
        last_run_minute: None,
        created_at: String::new(),
    })?;
    println!("{} 定时任务已创建：{}（{}）", ok("完成"), job.name, job.id.cyan());
    println!("{}", dim("exm serve 常驻时由网关调度器自动触发；exm cron run <id> 可手动执行。"));
    Ok(())
}

pub fn run_cron_enable(core: &Core, id: &str, enabled: bool) -> anyhow::Result<()> {
    let mut job = core
        .cron
        .get(id)?
        .ok_or_else(|| anyhow::anyhow!("任务不存在: {id}"))?;
    job.enabled = enabled;
    core.cron.upsert(job)?;
    println!("{} 任务 {id} 已{}", ok("完成"), if enabled { "启用" } else { "停用" });
    Ok(())
}

pub fn run_cron_remove(core: &Core, id: &str) -> anyhow::Result<()> {
    if core.cron.remove(id)? {
        println!("{} 定时任务已删除：{id}", ok("完成"));
    } else {
        anyhow::bail!("任务不存在: {id}");
    }
    Ok(())
}

pub async fn run_cron_now(core: &Core, id: &str) -> anyhow::Result<()> {
    let job = core
        .cron
        .get(id)?
        .ok_or_else(|| anyhow::anyhow!("任务不存在: {id}"))?;
    println!("{}", dim(format!("执行任务 {}（{}）…", job.name, job.id)));
    let run = core.run_cron_job(&job).await;
    println!(
        "{} 状态 {}｜会话 {}｜{}",
        ok("完成"),
        run.status,
        run.session_id.cyan(),
        dim(&run.summary)
    );
    Ok(())
}

pub fn run_cron_runs(core: &Core, job: Option<&str>, limit: usize) -> anyhow::Result<()> {
    let runs = core.cron.runs(job, limit)?;
    if runs.is_empty() {
        println!("{}", dim("（无运行记录）"));
        return Ok(());
    }
    for r in runs {
        println!(
            "  {} {} {} {}",
            r.started_at.dimmed(),
            r.job_name.bold(),
            if r.status == "done" { "done".green().to_string() } else { "failed".red().to_string() },
            dim(&r.summary)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- 执行审批

pub fn run_approval_list(core: &Core, status: Option<&str>, limit: usize) -> anyhow::Result<()> {
    let items = core.approval_list(status, limit)?;
    if items.is_empty() {
        println!("{}", dim("（无审批单）"));
        return Ok(());
    }
    for r in items {
        print_approval(&r);
    }
    Ok(())
}

fn print_approval(r: &ApprovalRequest) {
    let status = match r.status.as_str() {
        "pending" => "待审批".yellow().to_string(),
        "approved" => "已批准".green().to_string(),
        "denied" => "已拒绝".red().to_string(),
        "executed" => "已执行".green().to_string(),
        "failed" => "执行失败".red().to_string(),
        other => other.to_string(),
    };
    println!("  {} {} {} {}", r.id.cyan(), status, dim(format!("个体 {}", r.agent_id)), dim(&r.created_at));
    println!("    $ {}", r.command);
    if let Some(out) = &r.result {
        println!("    {}", dim(out.replace('\n', " ").chars().take(200).collect::<String>()));
    }
}

pub async fn run_approval_decide(core: &Core, id: &str, approve: bool) -> anyhow::Result<()> {
    let r = core.approval_decide(id, approve).await?;
    match r.status.as_str() {
        "denied" => println!("{} 审批单 {id} 已拒绝", ok("完成")),
        "executed" => {
            println!("{} 审批单 {id} 已批准并代执行", ok("完成"));
            if let Some(out) = &r.result {
                println!("{}", dim(format!("输出：{}", out.chars().take(600).collect::<String>())));
            }
        }
        other => println!("{} 审批单 {id} 状态：{other}", ok("完成")),
    }
    Ok(())
}
