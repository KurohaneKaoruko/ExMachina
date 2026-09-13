//! 冒烟测试
//! 覆盖：编成数据契约装载 / DAG 调度并发 / 端到端一次完整任务 / 自由建组（Mock 通道）
//! 编成（个体、链路）是数据：本文件不指向任何具体编制，只校验契约完整性。

use exm_core::config::ExmConfig;
use exm_core::memory::{MemoryDraft, MemoryKind};
use exm_core::types::*;
use exm_core::Core;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

fn test_config() -> ExmConfig {
    let root = std::env::current_dir().unwrap().join("..").join("..");
    let mut cfg = ExmConfig::load(&root);
    cfg.agents_dir = root.join("agents");
    cfg.data_dir = std::env::temp_dir().join(format!("exm-test-{}", uuid::Uuid::new_v4()));
    cfg.use_mock = true;
    cfg.max_concurrency = 4;
    cfg
}

#[test]
fn 装载编成_数据契约完整() {
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let agents = core.agents();
    assert!(agents.len() >= 10, "编成装载异常：个体数 {}", agents.len());
    let orchestrators = agents.iter().filter(|a| matches!(a.tier, Tier::Orchestrator)).count();
    assert_eq!(orchestrators, 1, "指挥体应恰好 1 个");

    // 个体契约：职责描述非空、能力标签非空、提示词文件存在
    for a in &agents {
        assert!(!a.description.trim().is_empty(), "{} 缺少职责描述", a.identifier);
        assert!(!a.capabilities.is_empty(), "{} 缺少能力标签", a.identifier);
        assert!(
            core.registry().prompt_exists(&a.prompt_file),
            "{} 缺少提示词文件 {}",
            a.identifier,
            a.prompt_file
        );
    }

    // 链路契约：每条 playbook 步骤指向存在的个体，且终点个体存在
    let playbooks = core.playbooks().expect("playbook 装载失败");
    assert!(!playbooks.is_empty(), "至少应装载 1 条链路模板");
    for p in &playbooks {
        assert!(!p.steps.is_empty(), "playbook {} 无步骤", p.id);
        for s in &p.steps {
            assert!(
                core.agent(&s.agent_identifier).is_some(),
                "playbook {} 步骤 {} 指向不存在的个体 {}",
                p.id,
                s.key,
                s.agent_identifier
            );
        }
        assert!(
            core.agent(&p.terminal).is_some(),
            "playbook {} 终点个体 {} 不存在",
            p.id,
            p.terminal
        );
    }

    // 裁决选路：按数据声明可解析出裁决个体（无命中时回退主智能体，永不为空）
    core.registry().arbiter().expect("裁决个体应可解析");

    // 契约校验：SyncReport 缺字段必须被拒
    assert!(exm_core::parse::parse_sync_report("没有 JSON").is_err());
}

#[test]
fn 自由建组_自定义集群热切换() {
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");

    // 建组演练不依赖默认组编成：残留先清理，保证可重跑
    let root = std::env::current_dir().unwrap().join("..").join("..");
    let _ = std::fs::remove_dir_all(root.join("agents").join("groups").join("smoke-style"));

    let g = core
        .create_group(Some("smoke-style".into()), "风格实验组", "验证自由建组与热切换")
        .expect("建组失败");
    assert_eq!(g.id, "smoke-style");
    assert_eq!(g.builtin, false);

    core.switch_group("smoke-style").expect("切组失败");
    assert!(core.agents().is_empty(), "新组初始应无个体");

    // 定义一个与默认组完全不同风格的主智能体（数据驱动，非代码内置）
    let def: AgentDefinition = serde_json::from_value(serde_json::json!({
        "name": "主笔体",
        "identifier": "chief-writer",
        "domain": "内容域",
        "tier": "unit",
        "description": "内容集群主智能体：统筹选题、写作与校对个体并直接交付",
        "capabilities": ["选题统筹", "内容调度"],
        "tools": ["read", "terminal"],
        "whenToCall": "内容生产类任务"
    }))
    .expect("定义构造失败");
    core.upsert_agent(def, None).expect("创建个体失败");
    core.set_primary("chief-writer").expect("设立主智能体失败");

    // 组感知：指挥体身份随激活组主智能体解析
    assert_eq!(core.orchestrator_id(), "chief-writer");
    assert!(core.agent("chief-writer").is_some());
    assert_eq!(core.active_group_meta().unwrap().primary.as_deref(), Some("chief-writer"));

    // 技能全局共用：切到自定义组后仍装载同一份技能（不做组隔离）
    let skills_in_group = core.skills().expect("技能装载失败");
    assert!(!skills_in_group.is_empty(), "自定义组应能看到全局技能包");

    // 组级记忆：写入归属激活组；组内可见，切回默认组后不可见
    core.memory_remember(MemoryDraft::new(MemoryKind::Fact, "风格组私有结论", "仅 smoke-style 组可见"))
        .expect("组内写入记忆失败");
    let in_group = core.memory_recall("风格组私有结论", Some(5)).unwrap();
    assert!(
        in_group.iter().any(|h| h.entry.title == "风格组私有结论"),
        "组内应检索到本组条目"
    );

    // 切回默认组后身份与记忆范围随之切换
    core.switch_group("default").expect("切回失败");
    assert_ne!(core.orchestrator_id(), "chief-writer");
    let after_back = core.memory_recall("风格组私有结论", Some(5)).unwrap();
    assert!(
        after_back.iter().all(|h| h.entry.title != "风格组私有结论"),
        "切回默认组后不应检索到他组条目"
    );

    core.delete_group("smoke-style").expect("删除组失败");
    assert!(core.list_groups().iter().all(|m| m.id != "smoke-style"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 执行审批_高危命令拦截与批准放行() {
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let (events, mut rx) = tokio::sync::broadcast::channel(64);
    let sec_off = exm_core::config::SecurityConfig::default();
    let sec_always = exm_core::config::SecurityConfig {
        exec_approval: "always".into(),
        exec_allowlist: vec![],
        auth_key: String::new(),
    };
    // always 模式（无豁免白名单）：所有终端命令都需审批
    let gw = exm_core::tools::ToolGateway::new(
        core.config().workspace_root.clone(),
        core.store.clone(),
        core.registry().clone(),
        events.clone(),
        sec_always,
    );

    // 命令拦截 → 审批单挂起 + 事件通知
    let blocked = gw
        .execute(
            "coding-agent",
            &[ToolName::Terminal],
            ToolName::Terminal,
            &serde_json::json!({ "command": "echo blocked-test" }),
        )
        .await;
    assert!(!blocked.ok, "未放行命令应被拦截");
    assert!(blocked.error.unwrap_or_default().contains("待人工审批"), "拦截信息应含审批提示");
    let evt = rx.try_recv().expect("应发出审批事件");
    assert_eq!(evt.kind, "approval.required");

    let pending = core.approval_list(Some("pending"), 10).unwrap();
    assert_eq!(pending.len(), 1, "应产生一张待审批单");
    assert_eq!(pending[0].command, "echo blocked-test");

    // 批准 → 系统代执行并记录结果
    let decided = core.approval_decide(&pending[0].id, true).unwrap();
    assert!(
        matches!(decided.status.as_str(), "executed" | "failed"),
        "批准后应代执行（平台无该命令时记 failed），实际 {}",
        decided.status
    );
    assert!(decided.result.is_some());

    // 白名单前缀放行：命中豁免前缀的命令跳过闸门，不产生新审批单
    let gw_exempt = exm_core::tools::ToolGateway::new(
        core.config().workspace_root.clone(),
        core.store.clone(),
        core.registry().clone(),
        events.clone(),
        exm_core::config::SecurityConfig {
            exec_approval: "always".into(),
            exec_allowlist: vec!["echo".into()],
            auth_key: String::new(),
        },
    );
    let allowed = gw_exempt
        .execute(
            "coding-agent",
            &[ToolName::Terminal],
            ToolName::Terminal,
            &serde_json::json!({ "command": "echo ok" }),
        )
        .await;
    assert!(allowed.ok, "命中豁免白名单的命令应直接放行");
    assert_eq!(core.approval_list(Some("pending"), 10).unwrap().len(), 0, "放行不应产生审批单");

    // off 模式（默认）：不拦截
    let gw_off = exm_core::tools::ToolGateway::new(
        core.config().workspace_root.clone(),
        core.store.clone(),
        core.registry().clone(),
        events.clone(),
        sec_off,
    );
    let free = gw_off
        .execute(
            "coding-agent",
            &[ToolName::Terminal],
            ToolName::Terminal,
            &serde_json::json!({ "command": "echo free" }),
        )
        .await;
    assert!(free.ok, "off 模式不应拦截");

    // risky 模式：echo 非高危 → 直接放行不拦截（闸门语义按风险分级）
    let sec_risky = exm_core::config::SecurityConfig {
        exec_approval: "risky".into(),
        exec_allowlist: vec![],
        auth_key: String::new(),
    };
    let gw_risky = exm_core::tools::ToolGateway::new(
        core.config().workspace_root.clone(),
        core.store.clone(),
        core.registry().clone(),
        events.clone(),
        sec_risky,
    );
    let safe = gw_risky
        .execute(
            "coding-agent",
            &[ToolName::Terminal],
            ToolName::Terminal,
            &serde_json::json!({ "command": "echo safe" }),
        )
        .await;
    assert!(safe.ok, "risky 模式下非高危命令应放行");

    // 已处理审批单不可重复决定
    assert!(core.approval_decide(&pending[0].id, false).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 单体智能体_目标切换与L0直答() {
    let cfg = test_config();
    // 残留清理（只清演练个体与目标状态；不得动 agents/singles 全目录——默认智能体种子在此）
    let agents_root = std::env::current_dir().unwrap().join("..").join("..").join("agents");
    let _ = std::fs::remove_file(agents_root.join("active_single"));
    let _ = std::fs::remove_file(agents_root.join("singles").join("lone-writer.json"));
    let _ = std::fs::remove_file(agents_root.join("singles").join("prompts").join("lone-writer.md"));
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    assert!(!core.registry().single_mode(), "默认为组模式");
    let group_agents = core.registry().list().len();

    // 创建单体（不属于任何组）并切换为目标
    let def: AgentDefinition = serde_json::from_value(serde_json::json!({
        "name": "独行者", "identifier": "lone-writer", "domain": "单体",
        "tier": "orchestrator", "description": "独立写作助理", "capabilities": ["独立写作"],
        "tools": ["read", "terminal"]
    }))
    .expect("定义构造失败");
    core.registry().upsert_single(def, None).expect("创建单体失败");
    core.registry().set_active_single(Some("lone-writer")).expect("切换目标失败");

    assert!(core.registry().single_mode());
    assert_eq!(core.orchestrator_id(), "lone-writer", "指挥体身份 = 当前单体");
    assert!(core.registry().units().is_empty(), "单体模式无派发个体");
    let active_list = core.registry().list_active();
    assert!(active_list.iter().any(|a| a.identifier == "lone-writer"), "当前范围应含目标单体");
    assert!(active_list.iter().all(|a| a.identifier != "exmachina-orchestrator"), "单体模式不应混入组编成");
    let scope = core.registry().active_scope();
    assert_eq!(scope, "single:lone-writer");

    // 单体模式对话：模拟通道按单体契约走 L0 直答（无派发节点）
    let session = core.create_session("单体冒烟").expect("建会话失败");
    assert!(session.group_id.starts_with("single:"), "会话归属单体范围");
    let orch = core.orchestrator();
    orch.handle_user_message(&session.id, "用一句话介绍你自己").await.expect("单体对话失败");
    // L0 直答不产生任务图；消息归属单体（本机）
    assert!(core.store.latest_graph(&session.id).unwrap().is_none(), "L0 不应有派发任务图");
    let messages = core.store.list_messages(&session.id, 50).unwrap();
    assert!(
        messages.iter().any(|m| m.role == MessageRole::Orchestrator && m.agent_id.as_deref() == Some("lone-writer")),
        "收束消息应归属单体智能体"
    );

    // 切回组模式：编成恢复，单体删除
    core.registry().set_active_single(None).expect("切回失败");
    assert!(!core.registry().single_mode());
    assert_eq!(core.registry().list().len(), group_agents);
    assert!(core.registry().remove_single("lone-writer").unwrap());
    assert!(core.registry().single("lone-writer").is_none());

    // 默认智能体 Machina（生成器预置）：存在、名为 Machina、提示词含「本机」自称
    let machina = core.registry().single("machina").expect("默认智能体 Machina 应预置");
    assert_eq!(machina.name, "Machina");
    let prompt = core.registry().load_single_prompt("machina.md").unwrap();
    assert!(prompt.contains("本机"), "Machina 提示词应含「本机」自称");
    // 指挥体不叫 Machina（组指挥体 ≠ 默认智能体）
    let orch = core.agent("exmachina-orchestrator").expect("指挥体应存在");
    assert_ne!(orch.name, "Machina");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 经验优化_合成_版本与重置() {
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    // 模拟通道确定性合成：手动触发不要求历史教训
    let a1 = core
        .orchestrator()
        .optimize_agent("scout-agent")
        .await
        .expect("经验要点合成失败");
    assert_eq!(a1.revision, 1);
    assert!(a1.content.starts_with("- "), "要点应为列表形式");
    assert!(a1.lessons_seen == 0, "无教训时计入 0 条");
    // 二次合成：修订号递增，旧版本入历史
    let a2 = core.orchestrator().optimize_agent("scout-agent").await.unwrap();
    assert_eq!(a2.revision, 2);
    assert_eq!(a2.previous.len(), 1, "旧版本应入历史");
    assert_eq!(a2.previous[0].revision, 1);
    assert!(core.registry().load_adaptation("scout-agent").unwrap().is_some());
    // 不存在的个体报错（不会创建任何东西）
    assert!(core.orchestrator().optimize_agent("no-such-agent").await.is_err());
    // 重置
    assert!(core.registry().reset_adaptation("scout-agent").unwrap());
    assert!(core.registry().load_adaptation("scout-agent").unwrap().is_none());
    assert!(!core.registry().reset_adaptation("scout-agent").unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 端到端_对话到收束全链路() {
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let session = core.create_session("冒烟").expect("建会话失败");
    let mut rx = core.subscribe();

    let mut kinds: Vec<String> = Vec::new();
    let collector = tokio::spawn(async move {
        let mut seen = Vec::new();
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    seen.push(evt.kind.clone());
                    if evt.kind == "run.finished" || evt.kind == "run.error" {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
        seen
    });

    let orch = core.orchestrator();
    orch.handle_user_message(&session.id, "评估 EXMACHINA 项目的架构风险")
        .await
        .expect("任务执行失败");

    kinds = tokio::time::timeout(Duration::from_secs(10), collector)
        .await
        .expect("事件收集超时")
        .expect("收集任务 panic");

    // 事件流契约
    for expect in ["orchestrator.token", "graph.updated", "dispatch.sent", "sync.received", "run.finished"] {
        assert!(kinds.iter().any(|k| k == expect), "缺少事件 {expect}，实际: {kinds:?}");
    }

    // 图与回流
    let graph = core.store.latest_graph(&session.id).unwrap().expect("任务图缺失");
    assert!(graph.nodes.len() >= 4, "节点数应 >= 4，实际 {}", graph.nodes.len());
    assert!(
        graph.nodes.iter().all(|n| n.status.is_terminal()),
        "存在未终态节点: {:?}",
        graph.nodes.iter().map(|n| (n.id.clone(), n.status)).collect::<Vec<_>>()
    );
    let done = graph.nodes.iter().filter(|n| n.status == TaskStatus::Done).count();
    assert!(done >= 3, "完成节点应 >= 3，实际 {done}");
    let parallel_roots = graph.nodes.iter().filter(|n| n.depends_on.is_empty()).count();
    assert!(parallel_roots >= 2, "应存在并行根节点");

    // 证据账与三账
    let evidence = core.store.list_evidence(&session.id).unwrap();
    assert!(!evidence.is_empty(), "证据账不应为空");
    let ledger = core.store.ledger_of(&session.id).unwrap();
    assert!(!ledger.task.goal.is_empty(), "任务账应有目标");
    assert!(!ledger.evidence.confirmed.is_empty(), "证据账应有确认项");

    // 收束消息：user / unit / orchestrator 齐备
    let messages = core.store.list_messages(&session.id, 200).unwrap();
    for role in [MessageRole::User, MessageRole::Unit, MessageRole::Orchestrator] {
        assert!(messages.iter().any(|m| m.role == role), "缺少 {role:?} 消息");
    }
}

/// 独立临时注册表（自带 default 组），不触碰仓库 agents/ 数据
fn temp_registry(tag: &str) -> exm_core::registry::LocalRegistry {
    let dir = std::env::temp_dir().join(format!("exm-reg-test-{}-{tag}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("definitions")).unwrap();
    let meta = GroupMeta {
        id: "default".into(),
        name: "测试组".into(),
        description: String::new(),
        primary: None,
        workspace: None,
        model: None,
        builtin: true,
        created_at: now_iso(),
    };
    std::fs::write(
        dir.join("group.json"),
        serde_json::to_string_pretty(&meta).unwrap(),
    )
    .unwrap();
    exm_core::registry::LocalRegistry::new(&dir).expect("临时注册表创建失败")
}

fn sample_def(identifier: &str) -> AgentDefinition {
    serde_json::from_value(serde_json::json!({
        "name": identifier,
        "identifier": identifier,
        "domain": "公共",
        "tier": "unit",
        "description": "测试个体",
        "capabilities": ["测试"]
    }))
    .expect("定义构造失败")
}

#[test]
fn 默认模型_组与个体解析与持久化() {
    // ---- ModelPool 解析语义 ----
    let mut pool = exm_core::provider::ModelPool::new();
    pool.insert("p-a", std::sync::Arc::new(exm_core::provider::MockLlmProvider), "orch-a", "unit-a");
    pool.insert("p-b", std::sync::Arc::new(exm_core::provider::MockLlmProvider), "orch-b", "unit-b");
    // 档案ID：按角色取指挥体/子个体模型
    let (prov, model) = pool.resolve("p-a", true).expect("应命中 p-a");
    assert_eq!(model, "unit-a");
    let (_, model) = pool.resolve("p-a", false).expect("应命中 p-a");
    assert_eq!(model, "orch-a");
    assert!(std::sync::Arc::strong_count(&prov) >= 1);
    // 档案ID/模型名：显式模型优先于角色
    let (_, model) = pool.resolve("p-b/自定义模型", false).expect("应命中 p-b");
    assert_eq!(model, "自定义模型");
    // 未知档案：回退全局（此处表现为 None）
    assert!(pool.resolve("p-不存在", true).is_none());

    // ---- 组默认模型：持久化 + 清除 ----
    let reg = temp_registry("group-model");
    let g = reg
        .create_group(Some("model-grp".into()), "模型组", "")
        .expect("建组失败");
    assert!(g.model.is_none());
    reg.set_group_model("model-grp", "p-b/unit-b").expect("设置组模型失败");
    assert_eq!(reg.group_meta("model-grp").unwrap().model.as_deref(), Some("p-b/unit-b"));
    reg.set_group_model("model-grp", "").expect("清除组模型失败");
    assert!(reg.group_meta("model-grp").unwrap().model.is_none());

    // ---- 个体默认模型：单体与组内个体均可设置并持久化 ----
    let def = sample_def("solo-one");
    reg.upsert_single(def, None).expect("创建单体失败");
    reg.set_agent_model("solo-one", "p-a").expect("设置单体模型失败");
    assert_eq!(reg.single("solo-one").unwrap().model_hint.as_deref(), Some("p-a"));

    reg.set_active_group("model-grp").expect("切组失败");
    let member = sample_def("member-one");
    reg.upsert_agent("model-grp", member, None).expect("创建组内个体失败");
    reg.set_agent_model("member-one", "p-b/orch-b").expect("设置个体模型失败");
    assert_eq!(
        reg.agents_in_group("model-grp")
            .iter()
            .find(|a| a.identifier == "member-one")
            .unwrap()
            .model_hint
            .as_deref(),
        Some("p-b/orch-b")
    );
    // 清除 = 跟随组/全局
    reg.set_agent_model("member-one", "").expect("清除个体模型失败");
    assert!(reg
        .agents_in_group("model-grp")
        .iter()
        .find(|a| a.identifier == "member-one")
        .unwrap()
        .model_hint
        .is_none());
    // 不存在的个体必须报错
    assert!(reg.set_agent_model("ghost", "p-a").is_err());
}
