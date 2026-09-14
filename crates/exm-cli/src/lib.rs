//! EXMACHINA CLI —— `exm` 与 `exmachina` 双命令入口（同一实现）
//!
//! 命令规范见 docs/07 §2：两个名字互为别名，参数、行为、退出码完全一致。

pub mod chat;
pub mod config_cmd;
pub mod doctor;
pub mod group_cmd;
pub mod install;
pub mod memory_cmd;
pub mod persona_cmd;
pub mod platform_cmd;
pub mod render;

use clap::{Parser, Subcommand};
use exm_core::Core;
use owo_colors::OwoColorize;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "exm",
    bin_name = "exm | exmachina",
    version,
    about = "EXMACHINA 多智能体系统 CLI（`exm` 与 `exmachina` 为同一命令的两个入口名）",
    long_about = "EXMACHINA CLI\n\n用法场景：\n  · `exm`        日常简洁调用（脚本、Pipeline、快速对话）\n  · `exmachina`  同一实现的完整名称（文档、CI、教学场景更自解释）\n\n两者行为、参数、退出码完全一致；差异仅体现在帮助信息的命令名回显。"
)]
struct Cli {
    /// 工作区根目录（默认当前目录）
    #[arg(long, global = true, default_value = ".")]
    workspace: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 首启安装向导：环境检查 → 模型接入 → 落盘 → 自检
    Install {
        /// 快速模式：仅确认关键项（模型接入仍会询问）
        #[arg(long)]
        quick: bool,
    },
    /// 体检：配置 / 编成 / 记忆 / 通道 / WebUI
    Doctor,
    /// 与指挥体对话（进程内直挂 core）
    Chat {
        /// 任务输入；缺省进入交互模式
        text: Option<String>,
        /// 继续既有会话
        #[arg(short, long)]
        session: Option<String>,
        /// 仅输出最终结果
        #[arg(short, long)]
        quiet: bool,
    },
    /// 列出指挥体与全部子个体
    Agents {
        /// 按能力检索
        #[arg(long)]
        capability: Option<String>,
    },
    /// 查看会话最新任务图
    Tasks {
        session_id: String,
    },
    /// 起网关（HTTP REST + WebSocket），供 WebUI / 第三方渠道接入
    Serve {
        #[arg(short, long, default_value_t = 4173)]
        port: u16,
    },
    /// 分布式执行节点：接入网关承接子个体派发（旧设备算力入池）
    Worker {
        /// 网关工作者端点，如 ws://192.168.1.10:4173/worker
        #[arg(short, long)]
        url: String,
        /// 工作者密钥（= 网关 authKey；未配置鉴权可省略）
        #[arg(short, long, default_value = "")]
        token: String,
        /// 本节点标识（缺省 = 主机名）
        #[arg(short, long, default_value = "")]
        id: String,
    },
    /// 智能体组：列出 / 创建 / 切换 / 删除（组是隔离与切换的基本单位）
    Group {
        #[command(subcommand)]
        action: GroupAction,
    },
    /// 组内个体管理：创建 / 删除（默认组定义受保护）
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// 人设（说话风格）：查看 / 修改 / 重置
    Persona {
        #[command(subcommand)]
        action: PersonaAction,
    },
    /// 配置管理（get/set/list/schema）
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// 记忆系统：查看 / 检索 / 固化 / 维护
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// 技能包：派发时按触发词自动携带的指令包（数据化技能）
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// 定时任务：cron / 一次性任务，由网关调度器自动执行
    Cron {
        #[command(subcommand)]
        action: CronAction,
    },
    /// 执行审批：高危命令的拦截与批准/拒绝
    Approval {
        #[command(subcommand)]
        action: ApprovalAction,
    },
    /// 模型档案：多厂商端点管理与切换（OpenAI 兼容）
    Model {
        #[command(subcommand)]
        action: ModelAction,
    },
}

#[derive(Subcommand)]
enum ModelAction {
    /// 列出模型档案与当前生效者
    List,
    /// 新增/更新档案
    Add {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        base_url: String,
        #[arg(long, default_value = "")]
        api_key: String,
        #[arg(long)]
        orch_model: String,
        #[arg(long)]
        unit_model: String,
    },
    /// 切换生效档案（热生效）
    Use { id: String },
    /// 删除档案
    Remove { id: String },
    /// 连通测试（缺省测当前生效档案）
    Test { id: Option<String> },
}

#[derive(Subcommand)]
enum SkillAction {
    /// 列出激活组技能包
    List,
    /// 创建技能包（任务目标命中触发词即随派发携带）
    Add {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        instructions: String,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long, value_delimiter = ',')]
        trigger: Vec<String>,
        /// 限定适用的个体 identifier；缺省全体适用
        #[arg(long, value_delimiter = ',')]
        agent: Vec<String>,
    },
    /// 删除技能包
    Remove { id: String },
}

#[derive(Subcommand)]
enum CronAction {
    /// 列出定时任务
    List,
    /// 创建定时任务（--cron 五段表达式 或 --at 一次性时间）
    Add {
        name: String,
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        cron: Option<String>,
        #[arg(long)]
        at: Option<String>,
        /// 执行组；缺省为运行时激活组
        #[arg(long)]
        group: Option<String>,
        /// 会话标题（复用同名会话）
        #[arg(long)]
        session_title: Option<String>,
    },
    /// 手动执行一次
    Run { id: String },
    /// 启用 / 停用
    Enable { id: String },
    /// 启用 / 停用
    Disable { id: String },
    /// 删除
    Remove { id: String },
    /// 运行记录（可按任务过滤）
    Runs {
        #[arg(long)]
        job: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum ApprovalAction {
    /// 列出审批单（--status pending 筛选）
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// 批准并代执行
    Approve { id: String },
    /// 拒绝
    Deny { id: String },
}

#[derive(Subcommand)]
enum GroupAction {
    /// 列出全部智能体组（标注激活组）
    List,
    /// 创建新组（可只建组；首个个体创建时自动成为主智能体）
    Create {
        name: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// 切换激活组
    Switch { id: String },
    /// 组详情
    Info { #[arg(long)] id: Option<String> },
    /// 删除自定义组
    Delete { id: String },
    /// 导出组为可分享的 bundle（组元信息 + 个体定义 + 提示词）
    Export {
        id: String,
        #[arg(long, default_value = "")]
        out: String,
    },
    /// 从 bundle 导入为新组
    Import {
        #[arg(long)]
        file: String,
        #[arg(long)]
        id: Option<String>,
    },
}

#[derive(Subcommand)]
enum AgentAction {
    /// 在组内创建个体（首个个体自动成为主智能体；默认写入激活组，可用 --group 指定）
    Create {
        name: String,
        #[arg(long)]
        identifier: String,
        #[arg(long)]
        description: String,
        #[arg(long, default_value = "自定义")]
        domain: String,
        /// orchestrator（主智能体）或 unit
        #[arg(long, default_value = "unit")]
        tier: String,
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
    },
    /// 删除组内个体（默认激活组，可用 --group 指定）
    Remove {
        identifier: String,
        #[arg(long)]
        group: Option<String>,
    },
    /// 设置组内主智能体
    SetPrimary {
        identifier: String,
        #[arg(long)]
        group: Option<String>,
    },
    /// 基于历史教训优化个体：合成经验改进要点（只调优，不创建）
    Optimize { identifier: String },
    /// 查看个体经验改进要点
    Adaptation { identifier: String },
    /// 重置个体的经验改进要点
    ResetAdaptation { identifier: String },
}

#[derive(Subcommand)]
enum PersonaAction {
    /// 查看个体人设（标注是否自定义）
    Get { identifier: String },
    /// 修改个体人设（--text 直接给内容，或 --file 从文件读取）
    Set {
        identifier: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// 重置为默认智械体风格
    Reset { identifier: String },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// 列出当前配置
    List,
    /// 读取单个配置键
    Get { key: String },
    /// 写入单个配置键
    Set { key: String, value: String },
    /// 输出配置 Schema（设置页/向导驱动源）
    Schema,
}

#[derive(Subcommand)]
enum MemoryAction {
    /// 列出记忆条目（--agent 查看该个体可见的：私有 + 群体）
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 检索记忆（综合得分排序；--agent 限定个体可见范围）
    Search {
        query: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// 写入一条记忆（--agent 归属个体则为私有记忆；--global 为跨组全局记忆；缺省归属激活组）
    Add {
        #[arg(long, default_value = "fact")]
        kind: String,
        title: String,
        body: String,
        #[arg(long)]
        pin: bool,
        #[arg(long, default_value_t = 0.6)]
        importance: f64,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long)]
        global: bool,
    },
    /// 固定/取消固定（固定项进入基础记忆 memory.md）
    Pin {
        id: String,
        #[arg(long)]
        off: bool,
    },
    /// 遗忘指定记忆
    Forget { id: String },
    /// 重建倒排索引
    Reindex,
    /// 记忆衰减整理
    Decay {
        #[arg(long, default_value_t = 0.05)]
        floor: f64,
    },
    /// 重渲染基础记忆 memory.md
    Render,
    /// 记忆库统计与个体可靠性
    Stats,
}

fn build_core(workspace: &PathBuf) -> anyhow::Result<Arc<Core>> {
    let root = workspace.canonicalize().unwrap_or_else(|_| workspace.clone());
    Ok(Arc::new(Core::create(&root)?))
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async { dispatch(cli).await })
}

async fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let workspace = cli.workspace.clone();
    match cli.command {
        Commands::Install { quick } => {
            let root = workspace.canonicalize().unwrap_or(workspace);
            install::run_install(&root, quick)
        }
        Commands::Doctor => {
            let root = workspace.canonicalize().unwrap_or(workspace);
            doctor::run_doctor(&root)
        }
        Commands::Chat { text, session, quiet } => {
            let core = build_core(&workspace)?;
            match text {
                Some(t) => {
                    chat::run_chat(core, &t, &chat::ChatOptions { session, quiet }).await
                }
                None => chat::run_interactive(core, quiet).await,
            }
        }
        Commands::Agents { capability } => {
            let core = build_core(&workspace)?;
            let defs = match capability {
                Some(cap) => core.registry.find_by_capability(&cap).into_iter().collect::<Vec<_>>(),
                None => core.agents(),
            };
            let mut last_domain = String::new();
            for d in defs {
                let label = d.domain.clone();
                if label != last_domain {
                    last_domain = label.clone();
                    println!("\n{}", label.bold().underline().to_string());
                }
                let tier = if matches!(d.tier, exm_core::types::Tier::Orchestrator) {
                    format!(" {} ", "指挥体".on_blue().white())
                } else {
                    "  ".to_string()
                };
                println!(
                    "{} {} {} — {}",
                    tier,
                    d.identifier.cyan(),
                    d.name.bold(),
                    render::dim(d.description.chars().take(40).collect::<String>())
                );
            }
            println!(
                "\n{}",
                render::dim(format!("共 {}（指挥体 1 + 子个体 {}）", core.registry.count(), core.registry.units().len()))
            );
            Ok(())
        }
        Commands::Tasks { session_id } => {
            let core = build_core(&workspace)?;
            match core.store.latest_graph(&session_id)? {
                Some(g) => {
                    println!("{}", format!("图 {}｜状态 {:?}", g.id, g.status).bold().to_string());
                    for n in &g.nodes {
                        println!("{}", render::node_line(n));
                    }
                }
                None => println!("{}", render::dim("该会话暂无任务图")),
            }
            Ok(())
        }
        Commands::Serve { port } => {
            let core = build_core(&workspace)?;
            exm_gateway::serve(core, port).await
        }
        Commands::Worker { url, token, id } => {
            let core = build_core(&workspace)?;
            let worker_id = if id.is_empty() {
                std::env::var("COMPUTERNAME")
                    .or_else(|_| std::env::var("HOSTNAME"))
                    .unwrap_or_else(|_| "worker".into())
            } else {
                id
            };
            eprintln!("[worker] 本地就绪（模型：{}），接入 {url}", if core.is_mock() { "Mock" } else { "已配置" });
            exm_core::remote::WorkerSession::run(&url, &token, &worker_id, core).await
        }
        Commands::Config { action } => {
            let root = workspace.canonicalize().unwrap_or(workspace);
            match action {
                ConfigAction::List => config_cmd::run_config_list(&root),
                ConfigAction::Get { key } => config_cmd::run_config_get(&root, &key),
                ConfigAction::Set { key, value } => config_cmd::run_config_set(&root, &key, &value),
                ConfigAction::Schema => config_cmd::run_config_schema(),
            }
        }
        Commands::Memory { action } => {
            let core = build_core(&workspace)?;
            match action {
                MemoryAction::List { kind, agent, limit } => {
                    memory_cmd::run_memory_list(&core, kind, agent.as_deref(), limit)
                }
                MemoryAction::Search { query, agent, limit } => {
                    memory_cmd::run_memory_search(&core, &query, agent.as_deref(), limit)
                }
                MemoryAction::Add { kind, title, body, pin, importance, agent, tags, global } => {
                    memory_cmd::run_memory_add(&core, &kind, &title, &body, pin, agent.as_deref(), tags, importance, global)
                }
                MemoryAction::Pin { id, off } => memory_cmd::run_memory_pin(&core, &id, !off),
                MemoryAction::Forget { id } => memory_cmd::run_memory_forget(&core, &id),
                MemoryAction::Reindex => memory_cmd::run_memory_reindex(&core),
                MemoryAction::Decay { floor } => memory_cmd::run_memory_decay(&core, floor),
                MemoryAction::Render => memory_cmd::run_memory_render(&core),
                MemoryAction::Stats => memory_cmd::run_memory_stats(&core),
            }
        }
        Commands::Skill { action } => {
            let core = build_core(&workspace)?;
            match action {
                SkillAction::List => platform_cmd::run_skill_list(&core),
                SkillAction::Add { id, name, instructions, description, trigger, agent } => {
                    platform_cmd::run_skill_add(&core, &id, &name, &instructions, &description, trigger, agent)
                }
                SkillAction::Remove { id } => platform_cmd::run_skill_remove(&core, &id),
            }
        }
        Commands::Cron { action } => {
            let core = build_core(&workspace)?;
            match action {
                CronAction::List => platform_cmd::run_cron_list(&core),
                CronAction::Add { name, prompt, cron, at, group, session_title } => {
                    platform_cmd::run_cron_add(&core, &name, &prompt, cron, at, group, session_title)
                }
                CronAction::Run { id } => platform_cmd::run_cron_now(&core, &id).await,
                CronAction::Enable { id } => platform_cmd::run_cron_enable(&core, &id, true),
                CronAction::Disable { id } => platform_cmd::run_cron_enable(&core, &id, false),
                CronAction::Remove { id } => platform_cmd::run_cron_remove(&core, &id),
                CronAction::Runs { job, limit } => platform_cmd::run_cron_runs(&core, job.as_deref(), limit),
            }
        }
        Commands::Approval { action } => {
            let core = build_core(&workspace)?;
            match action {
                ApprovalAction::List { status, limit } => {
                    platform_cmd::run_approval_list(&core, status.as_deref(), limit)
                }
                ApprovalAction::Approve { id } => platform_cmd::run_approval_decide(&core, &id, true).await,
                ApprovalAction::Deny { id } => platform_cmd::run_approval_decide(&core, &id, false).await,
            }
        }
        Commands::Model { action } => {
            let core = build_core(&workspace)?;
            match action {
                ModelAction::List => platform_cmd::run_model_list(&core),
                ModelAction::Add { id, name, base_url, api_key, orch_model, unit_model } => {
                    platform_cmd::run_model_add(&core, &id, &name, &base_url, &api_key, &orch_model, &unit_model)
                }
                ModelAction::Use { id } => platform_cmd::run_model_use(&core, &id),
                ModelAction::Remove { id } => platform_cmd::run_model_remove(&core, &id),
                ModelAction::Test { id } => platform_cmd::run_model_test(&core, id.as_deref()).await,
            }
        }
        Commands::Group { action } => {
            let core = build_core(&workspace)?;
            match action {
                GroupAction::List => group_cmd::run_list(&core),
                GroupAction::Create { name, id, description } => {
                    group_cmd::run_create(&core, &name, id, &description)
                }
                GroupAction::Switch { id } => group_cmd::run_switch(&core, &id),
                GroupAction::Info { id } => group_cmd::run_info(&core, id),
                GroupAction::Delete { id } => group_cmd::run_delete(&core, &id),
                GroupAction::Export { id, out } => group_cmd::run_export(&core, &id, &out),
                GroupAction::Import { file, id } => group_cmd::run_import(&core, &file, id.as_deref()),
            }
        }
        Commands::Agent { action } => {
            let core = build_core(&workspace)?;
            match action {
                AgentAction::Create { name, identifier, description, domain, tier, group, prompt } => {
                    group_cmd::run_agent_create(
                        &core, &name, &identifier, &description, &domain, &tier, group.as_deref(), prompt,
                    )
                }
                AgentAction::Remove { identifier, group } => {
                    group_cmd::run_agent_remove(&core, &identifier, group.as_deref())
                }
                AgentAction::Optimize { identifier } => {
                    platform_cmd::run_agent_optimize(&core, &identifier).await
                }
                AgentAction::Adaptation { identifier } => {
                    platform_cmd::run_agent_adaptation(&core, &identifier)
                }
                AgentAction::ResetAdaptation { identifier } => {
                    platform_cmd::run_agent_reset_adaptation(&core, &identifier)
                }
                AgentAction::SetPrimary { identifier, group } => {
                    group_cmd::run_agent_set_primary(&core, &identifier, group.as_deref())
                }
            }
        }
        Commands::Persona { action } => {
            let core = build_core(&workspace)?;
            match action {
                PersonaAction::Get { identifier } => persona_cmd::run_get(&core, &identifier),
                PersonaAction::Set { identifier, text, file } => {
                    persona_cmd::run_set(&core, &identifier, text, file)
                }
                PersonaAction::Reset { identifier } => persona_cmd::run_reset(&core, &identifier),
            }
        }
    }
}
