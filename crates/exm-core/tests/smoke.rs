//! 冒烟测试
//! 覆盖：编成数据契约装载 / DAG 调度并发 / 端到端一次完整任务 / 自由建组（Mock 通道）
//! 编成（个体、链路）是数据：本文件不指向任何具体编制，只校验契约完整性。

use exm_core::config::ExmConfig;
use exm_core::memory::{MemoryDraft, MemoryKind};
use exm_core::types::*;
use exm_core::Core;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// 测试串行锁：Core 型测试共享仓库工作区（agents/active_group 等运行时文件），并行互踩
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn test_config() -> ExmConfig {
    let root = std::env::current_dir().unwrap().join("..").join("..");
    let mut cfg = ExmConfig::load(&root);
    // 编成隔离副本：复制 agents/（跳过运行时残留 groups/active_*），测试互不污染仓库工作区
    let agents_copy = std::env::temp_dir().join(format!("exm-agents-{}", uuid::Uuid::new_v4()));
    copy_agents_tree(&root.join("agents"), &agents_copy);
    cfg.agents_dir = agents_copy;
    cfg.data_dir = std::env::temp_dir().join(format!("exm-test-{}", uuid::Uuid::new_v4()));
    cfg.use_mock = true;
    cfg.max_concurrency = 4;
    cfg
}

/// 复制编成树到临时目录（跳过 groups/、active_group、active_single 等运行时文件）
fn copy_agents_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "groups" || name == "active_group" || name == "active_single" || name == "singles" && false {
            continue;
        }
        let target = dst.join(&name);
        if entry.path().is_dir() {
            copy_agents_tree(&entry.path(), &target);
        } else {
            let _ = std::fs::copy(entry.path(), &target);
        }
    }
}

#[test]
fn 装载编成_数据契约完整() {
    let _serial = serial_guard();
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
    let _serial = serial_guard();
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
    let _serial = serial_guard();
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let (events, mut rx) = tokio::sync::broadcast::channel(64);
    let sec_off = exm_core::config::SecurityConfig::default();
    let sec_always = exm_core::config::SecurityConfig {
        exec_approval: "always".into(),
        exec_allowlist: vec![],
        auth_key: String::new(),
        ..Default::default()
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
    let decided = core.approval_decide(&pending[0].id, true).await.unwrap();
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
            ..Default::default()
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
        ..Default::default()
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
    assert!(core.approval_decide(&pending[0].id, false).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_agent_target_switch_and_l0_direct() {
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
    let _serial = serial_guard();
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
    let _serial = serial_guard();
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let session = core.create_session("冒烟").expect("建会话失败");
    let mut rx = core.subscribe();

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

    let kinds = tokio::time::timeout(Duration::from_secs(10), collector)
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
        capabilities: None,
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
    pool.insert("p-a", std::sync::Arc::new(exm_core::provider::MockLlmProvider), "model-a");
    pool.insert("p-b", std::sync::Arc::new(exm_core::provider::MockLlmProvider), "model-b");
    // 档案ID：取该档案的默认模型
    let (prov, model) = pool.resolve("p-a").expect("应命中 p-a");
    assert_eq!(model, "model-a");
    assert!(std::sync::Arc::strong_count(&prov) >= 1);
    // 档案ID/模型名：显式模型覆盖档案默认
    let (_, model) = pool.resolve("p-b/自定义模型").expect("应命中 p-b");
    assert_eq!(model, "自定义模型");
    // 未知档案：回退全局（此处表现为 None）
    assert!(pool.resolve("p-不存在").is_none());

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

#[test]
fn 模型回退链_展开与冷却() {
    use exm_core::provider::{FailoverState, ModelPool};
    use std::sync::Arc;

    let mut pool = ModelPool::new();
    pool.insert("p-a", Arc::new(exm_core::provider::MockLlmProvider), "model-a");
    pool.insert("p-b", Arc::new(exm_core::provider::MockLlmProvider), "model-b");
    pool.insert("p-c", Arc::new(exm_core::provider::MockLlmProvider), "model-c");
    pool.set_active("p-a");
    pool.set_fallback("p-a", "p-b");
    pool.set_fallback("p-b", "p-c");
    // 成环：c → a 应被截断
    pool.set_fallback("p-c", "p-a");

    let chain = pool.chain("p-a");
    assert_eq!(chain, vec!["p-a".to_string(), "p-b".to_string(), "p-c".to_string()]);
    // 起点即全局档案时不追加重复兜底
    assert_eq!(pool.chain("p-a").len(), 3);
    // entry 取该档案的默认模型
    let (_, m) = pool.entry("p-b").expect("p-b 应存在");
    assert_eq!(m, "model-b");

    // 冷却：cool → cooling；clear → 解除
    let fo = FailoverState::default();
    assert!(!fo.cooling("p-b"));
    fo.cool("p-b", 60);
    assert!(fo.cooling("p-b"));
    fo.clear("p-b");
    assert!(!fo.cooling("p-b"));
    // 短冷却（0 秒）立即过期
    fo.cool("p-c", 0);
    assert!(!fo.cooling("p-c"));
}

#[tokio::test]
async fn 上下文压缩与collect_多轮会话语义() {
    let _serial = serial_guard();
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");

    // 单体会话：L0 直答路径
    core.registry().set_active_single(Some("machina")).expect("切单体失败");
    let s = core.create_session("压缩演练").expect("建会话失败");

    // 灌入 25 条消息（> TRIGGER=20）：12 条 user 旧消息 + 12 条 system 回复 + 1 条新 user
    use exm_core::types::{MessageRole, SpeechTag, Statement};
    for k in 0..12 {
        core.store
            .add_message(&s.id, MessageRole::User, None, vec![Statement::new(SpeechTag::要求, format!("旧问题{k}"))])
            .unwrap();
        core.store
            .add_message(&s.id, MessageRole::System, None, vec![Statement::report(format!("旧答复{k}"))])
            .unwrap();
    }

    // 触发一轮对话：plan 应完成压缩（rolling_summary 生成，summary_upto=24）并成功收束
    core.chat(&s.id, "用一句话介绍你自己").await.expect("对话失败");
    let sess = core.store.get_session(&s.id).unwrap().expect("会话存在");
    assert!(sess.rolling_summary.is_some(), "应生成滚动摘要");
    assert_eq!(sess.summary_upto, Some(13), "摘要应覆盖 13 条旧消息（25 总量 - 12 保留窗口）");

    // collect：上一轮收束后再连发两条（未跑），第三条触发时应合并三条为一轮目标
    core.store
        .add_message(&s.id, MessageRole::User, None, vec![Statement::new(SpeechTag::要求, "补充一".to_string())])
        .unwrap();
    core.store
        .add_message(&s.id, MessageRole::User, None, vec![Statement::new(SpeechTag::要求, "补充二".to_string())])
        .unwrap();
    core.chat(&s.id, "现在处理").await.expect("collect 轮失败");
    let ledger = core.store.ledger_of(&s.id).unwrap();
    assert!(ledger.task.goal.contains("补充一") && ledger.task.goal.contains("补充二"),
        "collect 应合并排队输入，实际 goal: {:?}", ledger.task.goal);
}

/// MCP 客户端（http 传输）：测试内起一个最小 JSON-RPC 服务器，走完 configure→refresh→snapshot→call 全链
#[tokio::test]
async fn mcp_客户端_http全链() {
    use exm_core::mcp::{McpRegistry, McpServerConfig};
    use std::collections::HashMap;

    // 最小 MCP 服务器（axum，随机端口）：initialize / tools/list / tools/call
    let app = axum::Router::new().route(
        "/mcp",
        axum::routing::post(async |body: String| {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let id = v.get("id").cloned().unwrap_or(serde_json::json!(0));
            let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => serde_json::json!({ "protocolVersion": "2025-03-26", "capabilities": { "tools": {} } }),
                "tools/list" => serde_json::json!({ "tools": [{
                    "name": "echo",
                    "description": "回声工具",
                    "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] },
                }]}),
                "tools/call" => {
                    let text = v.pointer("/params/arguments/text").and_then(|t| t.as_str()).unwrap_or("");
                    serde_json::json!({ "content": [{ "type": "text", "text": format!("回声:{text}") }], "isError": false })
                }
                _ => serde_json::Value::Null,
            };
            axum::Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut cfg = McpServerConfig {
        id: "t".into(),
        transport: "http".into(),
        command: None,
        args: vec![],
        env: HashMap::new(),
        url: Some(format!("http://{addr}/mcp")),
        allowed_agents: vec![],
        enabled: true,
    };
    let reg = McpRegistry::shared();
    reg.configure(&[cfg.clone()]);

    let results = reg.refresh_all().await;
    assert!(matches!(&results[..], [(id, Ok(n))] if id == "t" && *n == 1), "应列出 1 个工具: {results:?}");

    // 快照：个体可见（allowedAgents 空 = 全体）
    let snap = reg.tool_snapshot_for("anyone");
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].name, "mcp:t:echo");

    // 调用：全名 → 回声
    let r = reg.call("mcp:t:echo", "anyone", &serde_json::json!({ "text": "你好" })).await.expect("调用失败");
    assert!(r.ok);
    assert_eq!(r.text, "回声:你好");

    // 个体白名单：仅限 scout-agent
    cfg.allowed_agents = vec!["scout-agent".into()];
    reg.configure(&[cfg]);
    let denied = reg.call("mcp:t:echo", "other-agent", &serde_json::json!({ "text": "x" })).await;
    assert!(matches!(denied, Some(r) if !r.ok), "白名单外的个体应被拒绝");
    assert!(reg.tool_snapshot_for("other-agent").is_empty());
    assert_eq!(reg.tool_snapshot_for("scout-agent").len(), 1);
}

/// 混合记忆检索：词项无命中时语义兜底；词项命中时语义加成
#[tokio::test]
async fn 混合记忆_语义兜底与加成() {
    use exm_core::memory::{MemoryDraft, MemoryKind, MemoryStore};

    let dir = std::env::temp_dir().join(format!("exm-mem-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mem = MemoryStore::open(&dir).unwrap();

    // 两条群体记忆：词项完全不含查询词（检验语义兜底）
    let a = mem
        .remember(&MemoryDraft::new(MemoryKind::Fact, "部署要点".to_string(), "生产环境部署前必须跑通全量验收与回滚演练".to_string()))
        .unwrap();
    let b = mem
        .remember(&MemoryDraft::new(MemoryKind::Fact, "发布纪律".to_string(), "每次发版都要留好回滚路径与灰度开关".to_string()))
        .unwrap();

    // 定性向量：a 与查询同向，b 近正交
    let qv = vec![1.0f32, 0.0, 0.0];
    mem.set_embedding(&a.id, vec![1.0, 0.0, 0.0]).unwrap();
    mem.set_embedding(&b.id, vec![0.0, 1.0, 0.0]).unwrap();

    // 查询词与两条记忆零词项重叠：纯词项会退化为 recent；带查询向量应语义命中 a
    let hits = mem.recall("上线之前需要注意什么仪式感的东西", 5, None, None, Some(&qv)).unwrap();
    assert!(!hits.is_empty(), "语义兜底应命中");
    assert_eq!(hits[0].entry.id, a.id, "最相似应为 a（同向向量）: {:?}",
        hits.iter().map(|h| (&h.entry.title, h.score)).collect::<Vec<_>>());
    assert!(hits[0].reasons.iter().any(|r| r.contains("语义相似")), "理由应含语义相似");

    // 词项路径不受影响：查询含「部署」且带向量 → 语义加成仍排第一
    let hits2 = mem.recall("部署 要点", 5, None, None, Some(&qv)).unwrap();
    assert_eq!(hits2[0].entry.id, a.id);
    // 无向量（None）：纯词项也能命中（回归）
    let hits3 = mem.recall("部署 要点", 5, None, None, None).unwrap();
    assert!(!hits3.is_empty() && hits3[0].entry.id == a.id);
}

/// web_fetch 工具 + 图片附件暂存：本地 axum 服务器抓取正文；stash 存取语义
#[tokio::test]
async fn web_fetch工具与图片暂存() {
    use exm_core::types::ToolName;

    // 最小 HTTP 页面服务器（含应被剥离的脚本与样式）
    let app = axum::Router::new().route(
        "/page",
        axum::routing::get(|| async {
            "<html><head><style>body{color:red}</style><script>alert(1)</script></head><body><h1>标题</h1><p>正文内容 甲乙丙</p></body></html>"
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // 规格注册：白名单内含 web_fetch
    let specs = exm_core::tools::ToolGateway::tool_specs(&[ToolName::WebFetch], false, false);
    assert!(specs.iter().any(|s| s.name == "web_fetch"), "web_fetch 应在规格中");

    // 工具面（对标主流 agent）：edit / grep / glob 在白名单内即下发
    let full = exm_core::tools::ToolGateway::tool_specs(
        &[ToolName::Read, ToolName::Edit, ToolName::Grep, ToolName::Glob, ToolName::WebSearch],
        false,
        false,
    );
    for name in ["read", "edit", "grep", "glob"] {
        assert!(full.iter().any(|s| s.name == name), "{name} 应下发");
    }
    // 诚实性：搜索后端未配置时 web_search 不得出现在下发的 schema 里
    assert!(
        !full.iter().any(|s| s.name == "web_search"),
        "搜索未配置时不得下发 web_search（不承诺不存在的能力）"
    );
    let with_search = exm_core::tools::ToolGateway::tool_specs(&[ToolName::WebSearch], true, false);
    assert!(with_search.iter().any(|s| s.name == "web_search"), "搜索就绪时应下发 web_search");
    // 浏览器同理：探测不到浏览器可执行文件时不下发 browser
    let no_browser = exm_core::tools::ToolGateway::tool_specs(&[ToolName::Browser], false, false);
    assert!(!no_browser.iter().any(|s| s.name == "browser"), "无浏览器时不得下发 browser");
    let with_browser = exm_core::tools::ToolGateway::tool_specs(&[ToolName::Browser], false, true);
    assert!(with_browser.iter().any(|s| s.name == "browser"), "浏览器就绪时应下发 browser");

    let cfg = {
        let _serial = serial_guard();
        test_config()
    };
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let tools = exm_core::tools::ToolGateway::new(
        &core.config().workspace_root,
        core.store.clone(),
        core.registry.clone(),
        core.events.clone(),
        core.config().security.clone(),
    );
    let r = tools
        .execute(
            "machina",
            &[ToolName::WebFetch],
            ToolName::WebFetch,
            &serde_json::json!({ "url": format!("http://{addr}/page") }),
        )
        .await;
    assert!(r.ok, "web_fetch 应成功: {:?}", r.error);
    assert!(r.output.contains("正文内容"), "应含正文: {}", r.output);
    assert!(!r.output.contains("alert"), "脚本应被剥离");
    assert!(!r.output.contains("color:red"), "样式应被剥离");

    // 图片暂存：stage → take 取走即清
    exm_core::image_stash::stage("s-img", vec!["data:image/png;base64,AAAA".to_string()]);
    let taken = exm_core::image_stash::take("s-img");
    assert_eq!(taken.len(), 1);
    assert!(exm_core::image_stash::take("s-img").is_empty(), "取走即清");
}

/// memory.md 超限自主压缩：LLM 简略 + 原文归档（工作区文件 / 数据库）
#[tokio::test]
async fn 记忆压缩_超限归档两模式() {
    let _serial = serial_guard();
    let mut cfg = test_config();
    cfg.memory_md_max_chars = 100;
    cfg.memory_enabled = false; // 深层关：归档到工作区文件
    let core = Core::with_config(cfg.clone()).expect("创建 Core 失败");

    let long_md: String = std::iter::repeat("记忆条目内容测试。").take(60).collect(); // ~480 字
    std::fs::write(&core.config().memory_md_path, &long_md).unwrap();

    let (before, after) = core.compact_memory_md().await.expect("压缩失败");
    assert_eq!(before, long_md.chars().count());
    assert!(after < before, "压缩后应变短：{before} -> {after}");
    let compacted = std::fs::read_to_string(&core.config().memory_md_path).unwrap();
    assert!(compacted.contains("已压缩"), "Mock 压缩输出应落盘");
    // 归档文件应包含原文
    let archive = std::fs::read_to_string(
        core.config().memory_md_path.with_file_name("memory_archive.md"),
    )
    .unwrap_or_default();
    assert!(archive.contains("记忆条目内容测试"), "原文应归档");

    // 深层开：归档进数据库（digest 条目）
    let mut cfg2 = test_config();
    cfg2.memory_md_max_chars = 100;
    cfg2.memory_enabled = true;
    let core2 = Core::with_config(cfg2).expect("创建 Core 失败");
    let long2: String = std::iter::repeat("深层归档验证。").take(60).collect();
    std::fs::write(&core2.config().memory_md_path, &long2).unwrap();
    core2.compact_memory_md().await.expect("压缩失败");
    let hits = core2
        .memory
        .recall_all("memory.md 归档", 10, None, None)
        .unwrap_or_default();
    assert!(hits.iter().any(|h| h.entry.title.contains("memory.md 归档")), "归档应进深层记忆");
}

/// 分布式执行节点：工作者（独立 Core，Mock）接入 → 派发远程执行 → 回流收束
#[tokio::test]
async fn 分布式执行_工作者节点全链() {
    use exm_core::remote::{RemoteExecutor, WorkerFrame};
    use futures_util::{SinkExt, StreamExt};
    use std::sync::Arc;

    let _serial = serial_guard();

    // ---- 最小 hub：实现 RemoteExecutor（工作者出站通道 + 回流 oneshot）----
    struct MiniHub {
        out: tokio::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<axum::extract::ws::Message>>>,
        report_tx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<exm_core::types::SyncReport, String>>>>,
    }
    #[async_trait::async_trait]
    impl RemoteExecutor for MiniHub {
        fn accepts(&self, _def: &exm_core::types::AgentDefinition) -> bool {
            true
        }
        async fn execute(
            &self,
            _session_id: &str,
            def: &exm_core::types::AgentDefinition,
            order: &exm_core::types::DispatchOrder,
            _on_token: &(dyn Fn(String) + Send + Sync),
        ) -> anyhow::Result<exm_core::types::SyncReport> {
            let out = self.out.lock().await.clone().ok_or_else(|| anyhow::anyhow!("无工作者"))?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            *self.report_tx.lock().await = Some(tx);
            let frame = serde_json::to_string(&WorkerFrame::Dispatch {
                did: "d-test".into(),
                def: def.clone(),
                order: order.clone(),
            })?;
            out.send(axum::extract::ws::Message::Text(frame)).ok();
            match rx.await {
                Ok(Ok(r)) => Ok(r),
                Ok(Err(e)) => anyhow::bail!("{e}"),
                Err(_) => anyhow::bail!("工作者断开"),
            }
        }
    }

    let hub = Arc::new(MiniHub {
        out: tokio::sync::Mutex::new(None),
        report_tx: tokio::sync::Mutex::new(None),
    });
    let hub2 = hub.clone();

    // ---- WS 端点（axum）：Hello 注册出站通道；Report/Error 回流 oneshot ----
    let app = axum::Router::new().route(
        "/worker",
        axum::routing::get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hub = hub2.clone();
            async move {
                ws.on_upgrade(move |socket| async move {
                    let (sink, mut stream) = socket.split();
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<axum::extract::ws::Message>();
                    let mut sink = sink;
                    tokio::spawn(async move {
                        while let Some(m) = rx.recv().await {
                            if sink.send(m).await.is_err() {
                                break;
                            }
                        }
                    });
                    while let Some(Ok(msg)) = stream.next().await {
                        let axum::extract::ws::Message::Text(t) = msg else { continue };
                        let Ok(frame) = serde_json::from_str::<WorkerFrame>(&t) else { continue };
                        match frame {
                            WorkerFrame::Hello { .. } => {
                                *hub.out.lock().await = Some(tx.clone());
                            }
                            WorkerFrame::Report { report, .. } => {
                                if let Some(s) = hub.report_tx.lock().await.take() {
                                    let _ = s.send(Ok(report));
                                }
                            }
                            WorkerFrame::Error { message, .. } => {
                                if let Some(s) = hub.report_tx.lock().await.take() {
                                    let _ = s.send(Err(message));
                                }
                            }
                            _ => {}
                        }
                    }
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // ---- 工作者：独立 Core（Mock）+ WorkerSession 接入 ----
    let worker_cfg = test_config();
    let worker_core = Arc::new(Core::with_config(worker_cfg).expect("工作者 Core 失败"));
    let ws_task = tokio::spawn({
        let url = format!("ws://{addr}/worker");
        let core = worker_core.clone();
        async move { exm_core::remote::WorkerSession::run(&url, "", "w-test", core).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    // ---- 中心侧 Core 注入 hub → 全远程链路 ----
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("中心 Core 失败");
    core.set_remote(hub).expect("注入远程执行器失败");
    let s = core.create_session("分布式演练").unwrap();
    core.chat(&s.id, "评估当前项目的架构风险").await.expect("分布式任务失败");

    let graph = core.store.latest_graph(&s.id).unwrap().expect("任务图缺失");
    assert!(graph.nodes.iter().all(|n| n.status.is_terminal()), "节点应全部终态");
    let done = graph.nodes.iter().filter(|n| n.status == exm_core::types::TaskStatus::Done).count();
    assert!(done >= 3, "远程执行完成节点应 >= 3，实际 {done}");

    ws_task.abort();
}

/// 断点续跑：人工构造中断态图（节点全部 dispatched）→ resume_interrupted → 全终态收束
#[tokio::test]
async fn 断点续跑_中断会话自动收束() {
    let _serial = serial_guard();
    let cfg = test_config();
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let s = core.create_session("中断演练").unwrap();

    // 先跑一轮完整任务（Mock），拿到合法图
    core.chat(&s.id, "评估当前项目的架构风险").await.expect("首轮失败");
    let g = core.store.latest_graph(&s.id).unwrap().expect("图缺失");
    let done = g.nodes.iter().filter(|n| n.status == exm_core::types::TaskStatus::Done).count();
    assert!(done >= 3);

    // 构造中断态：图改回 Executing、全部节点置为 dispatched（模拟进程被杀）
    {
        use exm_core::types::{GraphStatus, TaskStatus};
        let mut g2 = g.clone();
        g2.status = GraphStatus::Executing;
        for n in g2.nodes.iter_mut() {
            if n.status == TaskStatus::Done {
                n.status = TaskStatus::Dispatched;
            }
        }
        core.store.save_graph(&g2).unwrap();
    }

    let resumed = core.resume_interrupted().await;
    assert_eq!(resumed, 1, "应恢复 1 个会话");
    let g3 = core.store.latest_graph(&s.id).unwrap().expect("续跑后图缺失");
    assert!(
        g3.nodes.iter().all(|n| n.status.is_terminal()),
        "续跑后应全部终态"
    );
    let done3 = g3.nodes.iter().filter(|n| n.status == exm_core::types::TaskStatus::Done).count();
    assert!(done3 >= 3, "续跑完成节点应 >= 3，实际 {done3}");
}

/// 终端超时强杀 + 会话 token 预算闸门
#[tokio::test]
async fn 终端超时与会话预算() {
    use exm_core::tools::execute_shell_command_timed;

    // 挂死命令（长跑）1 秒超时强杀
    let cmd = if cfg!(windows) { "ping -n 30 127.0.0.1" } else { "sleep 30" };
    let root = std::env::temp_dir();
    let started = std::time::Instant::now();
    let r = execute_shell_command_timed(&root, cmd, 1).await;
    assert!(!r.ok, "挂死命令应失败");
    assert!(r.error.unwrap_or_default().contains("超时"), "应为超时错误");
    assert!(started.elapsed().as_secs() < 10, "应快速返回而非等满 30s");

    // 预算闸门：预算 1 → 首轮过后即拒绝
    let _serial = serial_guard();
    let mut cfg = test_config();
    cfg.max_session_tokens = 1;
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let s = core.create_session("预算演练").unwrap();
    core.chat(&s.id, "用一句话介绍你自己").await.expect("首轮应放行（预算从 0 起）");
    assert!(core.session_tokens_estimate(&s.id) > 0, "估算应非零");
    let second = core.chat(&s.id, "再来一轮").await;
    assert!(second.is_err(), "超预算应拒绝");
    assert!(second.unwrap_err().to_string().contains("预算"), "错误应含预算提示");
}

/// 语音转写契约：Mock 通道明确不支持（真实路径走 openai/azure /audio/transcriptions）
#[tokio::test]
async fn 语音转写_通道契约() {
    use exm_core::provider::{LlmProvider, MockLlmProvider};
    let r = MockLlmProvider.transcribe("whisper-1", b"RIFF....", "a.webm").await;
    assert!(r.is_err(), "Mock 不支持转写");
    assert!(r.unwrap_err().to_string().contains("不支持"), "错误应明示不支持");
    // 合成契约同构：Mock 明确不支持
    let s = MockLlmProvider.speak("tts-1", "你好").await;
    assert!(s.is_err() && s.unwrap_err().to_string().contains("不支持"));
}

/// 工具面（对标主流 agent）：edit 精确编辑 / read 行号分页 / grep / glob / 检查点 / 排程，
/// 以及安全边界（路径越界、终端白名单细化与连接符拒绝）
#[tokio::test]
async fn 工具面_编辑检索排程与安全边界() {
    let _serial = serial_guard();
    let mut cfg = test_config();
    // 隔离工作区：工具会真实读写，不能污染仓库
    let tmp = std::env::temp_dir().join(format!("exm-tools-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    cfg.workspace_root = tmp.clone();
    cfg.data_dir = tmp.join("data");
    cfg.config_path = tmp.join("config.json");
    cfg.memory_md_path = tmp.join("memory.md");
    cfg.webui_dist = tmp.join("webui-dist");
    let core = Core::with_config(cfg).expect("创建 Core 失败");
    let tools = exm_core::tools::ToolGateway::new(
        &core.config().workspace_root,
        core.store.clone(),
        core.registry.clone(),
        core.events.clone(),
        core.config().security.clone(),
    )
    .with_cron(core.cron.clone());

    // --- filesystem write + read（行号 + 分页）---
    let w = tools
        .execute(
            "machina",
            &[ToolName::Filesystem],
            ToolName::Filesystem,
            &serde_json::json!({ "op": "write", "path": "docs/demo.txt", "content": "第一行\n第二行 标记\n第三行 标记\n" }),
        )
        .await;
    assert!(w.ok, "写入应成功: {:?}", w.error);

    let r = tools
        .execute("machina", &[ToolName::Read], ToolName::Read, &serde_json::json!({ "path": "docs/demo.txt" }))
        .await;
    assert!(r.ok, "读取应成功: {:?}", r.error);
    assert!(r.output.contains("    1→第一行"), "应带行号: {}", r.output);
    assert!(r.output.contains("共 3 行"), "应报总行数: {}", r.output);

    let page = tools
        .execute(
            "machina",
            &[ToolName::Read],
            ToolName::Read,
            &serde_json::json!({ "path": "docs/demo.txt", "offset": 2, "limit": 1 }),
        )
        .await;
    assert!(page.output.contains("    2→第二行 标记"), "分页应命中第 2 行: {}", page.output);
    assert!(page.output.contains("续读 offset=3"), "应提示续读: {}", page.output);

    // --- edit：唯一命中替换 / 多命中拒绝 / 未命中提示 ---
    let e1 = tools
        .execute(
            "machina",
            &[ToolName::Edit],
            ToolName::Edit,
            &serde_json::json!({ "path": "docs/demo.txt", "old_string": "第一行", "new_string": "首行（已改）" }),
        )
        .await;
    assert!(e1.ok, "唯一命中应替换成功: {:?}", e1.error);
    let after = std::fs::read_to_string(tmp.join("docs/demo.txt")).unwrap();
    assert!(after.contains("首行（已改）") && !after.contains("第一行"), "替换结果不符: {after}");

    let e2 = tools
        .execute(
            "machina",
            &[ToolName::Edit],
            ToolName::Edit,
            &serde_json::json!({ "path": "docs/demo.txt", "old_string": "标记", "new_string": "M" }),
        )
        .await;
    assert!(!e2.ok && e2.error.clone().unwrap_or_default().contains("命中"), "多命中应拒绝: {:?}", e2.error);

    let e3 = tools
        .execute(
            "machina",
            &[ToolName::Edit],
            ToolName::Edit,
            &serde_json::json!({ "path": "docs/demo.txt", "old_string": "不存在的文本", "new_string": "x" }),
        )
        .await;
    assert!(!e3.ok && e3.error.clone().unwrap_or_default().contains("未找到"), "未命中应明示: {:?}", e3.error);

    // --- grep / glob ---
    let g = tools
        .execute(
            "machina",
            &[ToolName::Grep],
            ToolName::Grep,
            &serde_json::json!({ "pattern": "首行", "path": "docs" }),
        )
        .await;
    assert!(g.ok && g.output.contains("docs/demo.txt:"), "grep 应返回 文件:行:内容: {}", g.output);

    let gl = tools
        .execute("machina", &[ToolName::Glob], ToolName::Glob, &serde_json::json!({ "pattern": "**/*.txt" }))
        .await;
    assert!(gl.ok && gl.output.contains("docs/demo.txt"), "glob 应列出文件: {}", gl.output);

    // --- 安全：路径越界（`..` 拼接不得逃逸）---
    let esc = tools
        .execute(
            "machina",
            &[ToolName::Filesystem],
            ToolName::Filesystem,
            &serde_json::json!({ "op": "write", "path": "../escaped.txt", "content": "x" }),
        )
        .await;
    assert!(!esc.ok && esc.error.clone().unwrap_or_default().contains("越界"), "越界应被拒: {:?}", esc.error);
    assert!(!tmp.parent().unwrap().join("escaped.txt").exists(), "越界文件不得被创建");

    // --- 安全：终端白名单细化（子命令）与连接符拒绝 ---
    let bad_sub = tools
        .execute(
            "machina",
            &[ToolName::Terminal],
            ToolName::Terminal,
            &serde_json::json!({ "command": "cargo install some-crate" }),
        )
        .await;
    assert!(!bad_sub.ok && bad_sub.error.clone().unwrap_or_default().contains("白名单"), "未列入的子命令应拒: {:?}", bad_sub.error);

    let chained = tools
        .execute(
            "machina",
            &[ToolName::Terminal],
            ToolName::Terminal,
            &serde_json::json!({ "command": "git status && rm -rf docs" }),
        )
        .await;
    assert!(!chained.ok, "连接符命令应被拒");
    assert!(tmp.join("docs").exists(), "被拒命令不得产生副作用");

    // --- 检查点：写前快照 → 回滚 ---
    let cps = core.list_checkpoints();
    assert!(
        cps.iter().any(|(_, rel, _)| rel.replace('\\', "/") == "docs/demo.txt"),
        "应有该文件的写前快照: {cps:?}"
    );
    core.restore_checkpoint(None, "docs/demo.txt").expect("回滚失败");
    let restored = std::fs::read_to_string(tmp.join("docs/demo.txt")).unwrap();
    assert!(restored.contains("第一行"), "回滚应恢复原始内容: {restored}");

    // --- 排程工具：AI 自建定时任务（创建 / 列表 / 删除）---
    let s1 = tools
        .execute(
            "machina",
            &[ToolName::Schedule],
            ToolName::Schedule,
            &serde_json::json!({ "op": "create", "name": "每日巡检", "prompt": "检查未决任务", "cron": "0 9 * * *" }),
        )
        .await;
    assert!(s1.ok, "创建排程应成功: {:?}", s1.error);
    let s2 = tools
        .execute("machina", &[ToolName::Schedule], ToolName::Schedule, &serde_json::json!({ "op": "list" }))
        .await;
    assert!(s2.ok && s2.output.contains("每日巡检"), "列表应含新任务: {}", s2.output);
    let job_id = core.cron.list().unwrap().first().map(|j| j.id.clone()).unwrap_or_default();
    let s3 = tools
        .execute(
            "machina",
            &[ToolName::Schedule],
            ToolName::Schedule,
            &serde_json::json!({ "op": "remove", "id": job_id }),
        )
        .await;
    assert!(s3.ok, "删除排程应成功: {:?}", s3.error);
    assert!(core.cron.list().unwrap().is_empty(), "删除后应为空");

    // --- 结果落盘：超阈值输出写文件并回填路径（用大结果集触发：grep 命中数百行）---
    let many: String = (0..600)
        .map(|i| format!("命中行 {i} 用于验证超限结果的落盘引用机制\n"))
        .collect();
    let b = tools
        .execute(
            "machina",
            &[ToolName::Filesystem],
            ToolName::Filesystem,
            &serde_json::json!({ "op": "write", "path": "docs/many.txt", "content": many }),
        )
        .await;
    assert!(b.ok, "写入大文件应成功: {:?}", b.error);
    let gb = tools
        .execute(
            "machina",
            &[ToolName::Grep],
            ToolName::Grep,
            &serde_json::json!({ "pattern": "命中行", "path": "docs", "maxResults": 500 }),
        )
        .await;
    assert!(gb.ok, "大结果 grep 应成功: {:?}", gb.error);
    assert!(
        gb.output.contains("已截断") && gb.output.contains(".exmachina/tool-output/"),
        "超限结果应落盘并回填路径: {}",
        gb.output.chars().take(300).collect::<String>()
    );
}

/// 沙箱执行：环境净化（敏感变量不外泄）+ strict 联网闸门；浏览器工具端到端（无浏览器则跳过）
#[tokio::test]
async fn 沙箱_环境净化与联网闸门_浏览器() {
    let _serial = serial_guard();
    let mut cfg = test_config();
    let tmp = std::env::temp_dir().join(format!("exm-sandbox-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    cfg.workspace_root = tmp.clone();
    cfg.data_dir = tmp.join("data");
    cfg.config_path = tmp.join("config.json");
    cfg.memory_md_path = tmp.join("memory.md");
    cfg.webui_dist = tmp.join("webui-dist");
    cfg.sandbox.mode = "workspace".into();
    cfg.security.exec_approval = "off".into();
    let core = Core::with_config(cfg.clone()).expect("创建 Core 失败");

    // 故意注入一个敏感变量：沙箱化子进程不得看到它
    std::env::set_var("EXM_TEST_SECRET", "leak-me-if-visible");
    let tools = exm_core::tools::ToolGateway::new(
        &core.config().workspace_root,
        core.store.clone(),
        core.registry.clone(),
        core.events.clone(),
        core.config().security.clone(),
    )
    .with_sandbox(core.config().sandbox.clone());

    #[cfg(target_os = "windows")]
    let probe = "echo %EXM_TEST_SECRET%";
    #[cfg(not(target_os = "windows"))]
    let probe = "echo $EXM_TEST_SECRET";
    let r = tools
        .execute("machina", &[ToolName::Terminal], ToolName::Terminal, &serde_json::json!({ "command": probe }))
        .await;
    assert!(r.ok, "白名单内命令应执行: {:?}", r.error);
    assert!(
        !r.output.contains("leak-me-if-visible"),
        "沙箱应净化敏感环境变量，实际输出: {}",
        r.output
    );

    // Windows 作业对象：句柄可建可挂（自检），且设了资源上限也不妨碍普通命令
    #[cfg(target_os = "windows")]
    {
        assert!(
            exm_core::tools::windows_job_available(),
            "应能创建作业对象并把子进程收进作业（沙箱加固自检）"
        );
        let mut lim_cfg = cfg.clone();
        lim_cfg.sandbox.memory_mb = 512;
        lim_cfg.sandbox.max_processes = 8;
        let core3 = Core::with_config(lim_cfg).expect("创建 Core 失败");
        let lim_tools = exm_core::tools::ToolGateway::new(
            &core3.config().workspace_root,
            core3.store.clone(),
            core3.registry.clone(),
            core3.events.clone(),
            core3.config().security.clone(),
        )
        .with_sandbox(core3.config().sandbox.clone());
        let r2 = lim_tools
            .execute("machina", &[ToolName::Terminal], ToolName::Terminal, &serde_json::json!({ "command": "echo job-ok" }))
            .await;
        assert!(
            r2.ok && r2.output.contains("job-ok"),
            "512MB 内存 + 8 进程上限不应妨碍普通命令: {:?}",
            r2.error
        );
    }
    #[cfg(not(target_os = "windows"))]
    eprintln!("[skip] 非 Windows 平台：作业对象用例跳过（Linux/macOS 由 ulimit + bwrap 负责）");

    // strict：联网类命令（git fetch）走审批闸门；本地命令不受影响
    let mut strict_cfg = cfg.clone();
    strict_cfg.sandbox.mode = "strict".into();
    strict_cfg.sandbox.allow_network = false;
    let core2 = Core::with_config(strict_cfg).expect("创建 Core 失败");
    let strict_tools = exm_core::tools::ToolGateway::new(
        &core2.config().workspace_root,
        core2.store.clone(),
        core2.registry.clone(),
        core2.events.clone(),
        core2.config().security.clone(),
    )
    .with_sandbox(core2.config().sandbox.clone());
    let net = strict_tools
        .execute("machina", &[ToolName::Terminal], ToolName::Terminal, &serde_json::json!({ "command": "git fetch" }))
        .await;
    assert!(!net.ok, "strict 下联网命令应被拦截");
    let net_msg = net.error.clone().unwrap_or_default();
    assert!(net_msg.contains("审批") || net_msg.contains("拦截"), "应给出审批提示: {net_msg}");
    let local = strict_tools
        .execute("machina", &[ToolName::Terminal], ToolName::Terminal, &serde_json::json!({ "command": "echo local-ok" }))
        .await;
    assert!(local.ok && local.output.contains("local-ok"), "strict 下本地命令应放行: {:?}", local.error);

    // 浏览器工具：探测不到浏览器则跳过（按诚实性原则，此时该工具也不会下发给模型）
    if !tools.browser_ready() {
        eprintln!("[skip] 未探测到 Chrome/Chromium，跳过浏览器用例");
        return;
    }
    let app = axum::Router::new().route(
        "/page",
        axum::routing::get(|| async {
            "<html><head><title>浏览器用例</title></head><body>\
             <h1>动态正文</h1><div id=\"box\">初始</div>\
             <script>document.getElementById('box').textContent = '脚本已执行';</script>\
             </body></html>"
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let open = tools
        .execute(
            "machina",
            &[ToolName::Browser],
            ToolName::Browser,
            &serde_json::json!({ "op": "open", "url": format!("http://{addr}/page") }),
        )
        .await;
    assert!(open.ok, "browser open 应成功: {:?}", open.error);
    assert!(open.output.contains("动态正文"), "应取到渲染后正文: {}", open.output);
    assert!(open.output.contains("脚本已执行"), "JS 渲染结果应可见（证明是真实浏览器）: {}", open.output);

    let shot = tools
        .execute(
            "machina",
            &[ToolName::Browser],
            ToolName::Browser,
            &serde_json::json!({ "op": "screenshot", "name": "smoke-page" }),
        )
        .await;
    assert!(shot.ok, "截图应成功: {:?}", shot.error);
    assert!(
        tmp.join(".exmachina").join("screenshots").join("smoke-page.png").is_file(),
        "截图应落盘: {}",
        shot.output
    );
    let closed = tools
        .execute("machina", &[ToolName::Browser], ToolName::Browser, &serde_json::json!({ "op": "close" }))
        .await;
    assert!(closed.ok, "close 应成功");
}

#[tokio::test]
async fn 自定义工具_HTTP与命令模板() {
    let _serial = serial_guard();
    let mut cfg = test_config();
    let tmp = std::env::temp_dir().join(format!("exm-custom-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    cfg.workspace_root = tmp.clone();
    cfg.data_dir = tmp.join("data");
    cfg.config_path = tmp.join("config.json");
    cfg.memory_md_path = tmp.join("memory.md");
    cfg.webui_dist = tmp.join("webui-dist");
    cfg.security.exec_approval = "off".into();
    cfg.sandbox.mode = "workspace".into();

    // 本地 HTTP 服务：GET 回显 query、POST 回显 body 字段
    let app = axum::Router::new()
        .route(
            "/now",
            axum::routing::get(|axum::extract::RawQuery(q): axum::extract::RawQuery| async move {
                format!("q={}", q.unwrap_or_default())
            }),
        )
        .route(
            "/echo",
            axum::routing::post(|axum::Json(b): axum::Json<serde_json::Value>| async move {
                format!("name={}", b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    cfg.tools.custom = vec![
        // GET + query 插值（对全部个体可见）
        exm_core::config::CustomTool {
            name: "weather".into(),
            description: "查天气".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }),
            kind: "http".into(),
            target: format!("http://{addr}/now?city={{city}}"),
            method: "GET".into(),
            headers: Default::default(),
            agents: vec![],
        },
        // POST + JSON body
        exm_core::config::CustomTool {
            name: "greet".into(),
            description: String::new(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            kind: "http".into(),
            target: format!("http://{addr}/echo"),
            method: "POST".into(),
            headers: Default::default(),
            agents: vec![],
        },
        // shell 模板（仅 machina 可见）
        exm_core::config::CustomTool {
            name: "shout".into(),
            description: "喊话".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "word": { "type": "string" } },
                "required": ["word"]
            }),
            kind: "shell".into(),
            target: "echo hi-{word}".into(),
            method: String::new(),
            headers: Default::default(),
            agents: vec!["machina".into()],
        },
        // 对 machina 不可见
        exm_core::config::CustomTool {
            name: "other-tool".into(),
            description: String::new(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            kind: "http".into(),
            target: "http://127.0.0.1:1/x".into(),
            method: "GET".into(),
            headers: Default::default(),
            agents: vec!["someone-else".into()],
        },
    ];
    let core = Core::with_config(cfg.clone()).expect("创建 Core 失败");
    let tools = exm_core::tools::ToolGateway::new(
        &core.config().workspace_root,
        core.store.clone(),
        core.registry.clone(),
        core.events.clone(),
        core.config().security.clone(),
    )
    .with_sandbox(core.config().sandbox.clone())
    .with_custom_tools(core.config().tools.custom.clone());

    // schema 下发按 agents 可见性过滤
    let specs = tools.custom_specs_for("machina");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"weather") && names.contains(&"shout"), "可见自定义工具应下发: {names:?}");
    assert!(!names.contains(&"other-tool"), "agents 未包含的个体不应看到该工具");
    assert!(
        specs.iter().all(|s| s.name != "weather" || s.parameters.get("required").is_some()),
        "参数 schema 应随工具下发"
    );

    // GET：query 插值 + URL 编码
    let r = tools
        .execute_named("machina", &[], "weather", &serde_json::json!({ "city": "shanghai" }))
        .await;
    assert!(r.ok, "http 工具应成功: {:?}", r.error);
    assert!(r.output.contains("city=shanghai"), "query 应带上实参: {}", r.output);

    // POST：实参作为 JSON body
    let r = tools
        .execute_named("machina", &[], "greet", &serde_json::json!({ "name": "exm" }))
        .await;
    assert!(r.ok && r.output.contains("name=exm"), "POST body 应带实参: {:?} {}", r.error, r.output);

    // shell：模板插值
    let r = tools
        .execute_named("machina", &[], "shout", &serde_json::json!({ "word": "X" }))
        .await;
    assert!(r.ok && r.output.contains("hi-X"), "shell 模板应插值: {:?} {}", r.error, r.output);

    // 可见性：agents 未包含 → 拒绝
    let r = tools.execute_named("machina", &[], "other-tool", &serde_json::json!({})).await;
    assert!(!r.ok && r.error.clone().unwrap_or_default().contains("未对本个体开放"), "不可见工具应拒绝: {:?}", r.error);

    // 注入面：shell 实参带元字符 → 拒绝
    let r = tools
        .execute_named("machina", &[], "shout", &serde_json::json!({ "word": "a&whoami" }))
        .await;
    assert!(!r.ok && r.error.clone().unwrap_or_default().contains("不允许的字符"), "危险实参应拒绝: {:?}", r.error);

    // 未知工具
    let r = tools.execute_named("machina", &[], "no-such-tool", &serde_json::json!({})).await;
    assert!(!r.ok && r.error.clone().unwrap_or_default().contains("未知工具"), "未知工具应报错: {:?}", r.error);
}
