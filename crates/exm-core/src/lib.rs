//! EXMACHINA 核心运行时（Rust）
//!
//! 组成：Store(SQLite) → MessageBus(全连结) → LocalRegistry(个体注册表)
//!      → LlmProvider(OpenAI 兼容 / Mock) → ToolGateway → AgentRuntime → Orchestrator
//!
//! 集群兼容：Bus / Registry / Scheduler 均为接口或可替换实现；
//! 单体实现先行，替换为分布式实现时上层业务零改动。

pub mod bus;
pub mod config;
pub mod cron;
pub mod fsdb;
pub mod image_stash;
pub mod mcp;
pub mod memory;
pub mod orchestrator;
pub mod parse;
pub mod provider;
pub mod registry;
pub mod remote;
pub mod runtime;
pub mod store;
pub mod task;
pub mod tools;
pub mod types;

use crate::bus::{InProcessBus, MessageBus};
use crate::config::ExmConfig;
use crate::cron::CronStore;
use crate::memory::{MemoryDraft, MemoryEntry, MemoryKind, MemoryStore, RecallHit};
use crate::orchestrator::{Orchestrator, ORCHESTRATOR_ID};
use crate::provider::{
    FailoverState, LlmProvider, MockLlmProvider, ModelPool, OpenAiCompatibleProvider,
    UnconfiguredProvider,
};
use crate::registry::LocalRegistry;
use crate::runtime::AgentRuntime;
use crate::store::Store;
use crate::tools::ToolGateway;
use crate::types::*;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 核心句柄：所有渠道（CLI / Gateway / 未来桌面端）共用同一实例
pub struct Core {
    pub bus: Arc<dyn MessageBus>,
    pub registry: Arc<LocalRegistry>,
    pub store: Arc<Store>,
    pub memory: Arc<MemoryStore>,
    pub events: broadcast::Sender<CoreEvent>,
    pub cron: Arc<CronStore>,
    /// 会话级运行串行：同一会话的并发输入排队（followup），不互踩任务图
    run_locks: parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// MCP 服务器池（与 Orchestrator 内工具网关共用同一实例）
    mcp: Arc<crate::mcp::McpRegistry>,
    /// 远程执行器（工作者池；网关在 serve 时注入）
    remote: parking_lot::RwLock<Option<Arc<dyn crate::remote::RemoteExecutor>>>,
    orchestrator: RwLock<Arc<Orchestrator>>,
    config: RwLock<Arc<ExmConfig>>,
}

impl Core {
    pub fn create(workspace_root: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        Self::with_config(ExmConfig::load(workspace_root))
    }

    pub fn with_config(cfg: ExmConfig) -> anyhow::Result<Self> {
        let store = Arc::new(Store::open(&cfg.data_dir)?);
        let memory = Arc::new(MemoryStore::open(&cfg.data_dir)?);
        let cron = Arc::new(CronStore::open(&cfg.data_dir)?);
        let registry = Arc::new(LocalRegistry::new(&cfg.agents_dir)?);
        let bus: Arc<dyn MessageBus> = Arc::new(InProcessBus::new());
        let (events, _rx) = broadcast::channel(8192);
        let mcp_handle = crate::mcp::McpRegistry::shared();
        mcp_handle.configure(&cfg.mcp_servers);
        let orchestrator = Arc::new(build_orchestrator(
            &cfg,
            &store,
            &registry,
            &events,
            &memory,
            Some(mcp_handle.clone()),
            None,
        )?);

        Ok(Core {
            bus,
            registry,
            store,
            memory,
            events,
            cron,
            run_locks: parking_lot::Mutex::new(std::collections::HashMap::new()),
            mcp: mcp_handle.clone(),
            remote: parking_lot::RwLock::new(None),
            orchestrator: RwLock::new(orchestrator),
            config: RwLock::new(Arc::new(cfg)),
        })
    }

    /// 配置热更新（设置页）：重建 Provider / Runtime / Orchestrator
    pub fn apply_config(&self, cfg: ExmConfig) -> anyhow::Result<()> {
        cfg.save()?;
        self.mcp.configure(&cfg.mcp_servers);
        let orch = build_orchestrator(&cfg, &self.store, &self.registry, &self.events, &self.memory, Some(self.mcp.clone()), self.remote())?;
        *self.orchestrator.write() = Arc::new(orch);
        *self.config.write() = Arc::new(cfg);
        Ok(())
    }

    // ---------------- 记忆系统便捷方法（组感知，docs/09 §6） ----------------

    pub fn memory_recall(&self, query: &str, limit: Option<usize>) -> anyhow::Result<Vec<RecallHit>> {
        let cfg = self.config();
        let gid = self.registry.active_group();
        self.memory
            .recall(query, limit.unwrap_or(cfg.memory_recall_limit), None, Some(&gid), None)
    }

    /// 全量检索（管理视角：群体 + 所有个体私有；范围 = 激活组 + 全局条目）
    pub fn memory_recall_all(&self, query: &str, limit: Option<usize>) -> anyhow::Result<Vec<RecallHit>> {
        let gid = self.registry.active_group();
        self.memory
            .recall_all(query, limit.unwrap_or(self.config().memory_recall_limit), Some(&gid), None)
    }

    /// 个体检索：该智能体私有 + 群体共享（范围 = 激活组 + 全局条目）
    pub fn memory_recall_for_agent(
        &self,
        agent_id: &str,
        query: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<RecallHit>> {
        let cfg = self.config();
        let gid = self.registry.active_group();
        self.memory
            .recall_for_agent(agent_id, query, limit.unwrap_or(cfg.memory_recall_limit), Some(&gid), None)
    }

    /// 写入记忆：自动归属激活组（组内可见，其他组不可见）
    pub fn memory_remember(&self, mut draft: MemoryDraft) -> anyhow::Result<MemoryEntry> {
        if draft.group_id.is_none() {
            draft.group_id = Some(self.registry.active_group());
        }
        let entry = self.memory.remember(&draft)?;
        self.render_memory_md()?;
        Ok(entry)
    }

    /// 写入全局记忆（跨组可见）
    pub fn memory_remember_global(&self, draft: MemoryDraft) -> anyhow::Result<MemoryEntry> {
        let mut draft = draft;
        draft.group_id = None;
        let entry = self.memory.remember(&draft)?;
        self.render_memory_md()?;
        Ok(entry)
    }

    pub fn memory_list(&self, kind: Option<MemoryKind>, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        self.memory.list(kind, limit)
    }

    /// 按归属过滤列表：Some(id) => 该个体私有 + 群体共享；组范围 = 激活组 + 全局条目
    pub fn memory_list_filtered(
        &self,
        kind: Option<MemoryKind>,
        agent: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let gid = self.registry.active_group();
        self.memory.list_filtered(kind, agent, limit, Some(&gid))
    }

    pub fn memory_stats(&self) -> anyhow::Result<serde_json::Value> {
        self.memory.stats()
    }

    pub fn memory_pin(&self, id: &str, pinned: bool) -> anyhow::Result<()> {
        self.memory.pin(id, pinned)?;
        self.render_memory_md()?;
        Ok(())
    }

    pub fn memory_forget(&self, id: &str) -> anyhow::Result<()> {
        self.memory.forget(id)?;
        self.render_memory_md()?;
        Ok(())
    }

    pub fn memory_reindex(&self) -> anyhow::Result<usize> {
        self.memory.reindex()
    }

    /// 记忆整理：时间衰减 + 重新渲染 memory.md
    pub fn memory_decay(&self, floor: f64) -> anyhow::Result<usize> {
        let cfg = self.config();
        let changed = self.memory.decay(cfg.memory_half_life_days, floor)?;
        self.render_memory_md()?;
        Ok(changed)
    }

    /// 重新生成基础记忆文件（memory.md）
    pub fn render_memory_md(&self) -> anyhow::Result<usize> {
        let cfg = self.config();
        self.memory.render_memory_md(&cfg.memory_md_path)
    }

    // ---------------- 人设（说话风格） ----------------

    pub fn persona(&self, identifier: &str) -> anyhow::Result<String> {
        self.registry.persona(identifier)
    }

    pub fn persona_is_custom(&self, identifier: &str) -> anyhow::Result<bool> {
        self.registry.persona_is_custom(identifier)
    }

    pub fn set_persona(&self, identifier: &str, text: &str) -> anyhow::Result<()> {
        self.registry.set_persona(identifier, text)
    }

    pub fn reset_persona(&self, identifier: &str) -> anyhow::Result<bool> {
        self.registry.reset_persona(identifier)
    }

    /// 统一对话入口：会话级串行（运行中的后续输入排队为 followup，收束后依次处理；
    /// 排队窗内到达的多条输入由 handle_user_message 的 collect 合并为一轮）
    /// 语音转写（全局生效档案，whisper 系模型；音频字节 → 文本）
    pub async fn transcribe(&self, audio: &[u8], filename: &str) -> anyhow::Result<String> {
        let orch = self.orchestrator();
        orch.transcribe_audio(audio, filename).await
    }

    /// 语音合成（全局生效档案，tts 系模型；文本 → mp3 字节）
    pub async fn speak(&self, text: &str) -> anyhow::Result<Vec<u8>> {
        let orch = self.orchestrator();
        orch.speak_audio(text).await
    }

    /// memory.md 超限自主压缩（LLM 简略 + 旧文归档）；返回 (压缩前, 压缩后) 字数
    pub async fn compact_memory_md(&self) -> anyhow::Result<(usize, usize)> {
        let max = self.config().memory_md_max_chars;
        let orch = self.orchestrator();
        orch.compact_memory_md(max).await
    }

    /// 会话 token 估算（全部消息陈述字符 / 4）
    pub fn session_tokens_estimate(&self, session_id: &str) -> u64 {
        self.store
            .list_messages(session_id, 500)
            .unwrap_or_default()
            .iter()
            .map(|m| m.statements.iter().map(|s| s.text.chars().count() as u64).sum::<u64>() / 4)
            .sum()
    }

    pub async fn chat(&self, session_id: &str, text: &str) -> anyhow::Result<()> {
        // 预算闸门：估算口径超限即拒绝新轮次（0 = 不限）
        let budget = self.config().max_session_tokens;
        if budget > 0 {
            let used = self.session_tokens_estimate(session_id);
            if used >= budget {
                anyhow::bail!("会话 token 预算已耗尽（估算 {used} / {budget}）；请新建会话或上调 maxSessionTokens");
            }
        }
        let lock = {
            let mut locks = self.run_locks.lock();
            locks
                .entry(session_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let orch = self.orchestrator();
        orch.handle_user_message(session_id, text).await
    }

    pub fn mcp(&self) -> Arc<crate::mcp::McpRegistry> {
        self.mcp.clone()
    }

    /// 注入远程执行器（工作者池）并热重建 Orchestrator 启用远程路由
    pub fn set_remote(&self, remote: Arc<dyn crate::remote::RemoteExecutor>) -> anyhow::Result<()> {
        *self.remote.write() = Some(remote);
        self.rebuild_orchestrator()
    }

    fn remote(&self) -> Option<Arc<dyn crate::remote::RemoteExecutor>> {
        self.remote.read().clone()
    }

    /// 断点续跑：网关重启后，把中断会话（图非终态）的剩余节点重新调度收束。
    /// 已完成节点的回流从持久层回填；dispatched/running 重置为 ready 重跑（幂等重执行）。
    pub async fn resume_interrupted(&self) -> usize {
        let sessions = match self.list_sessions() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        let mut resumed = 0usize;
        for s in sessions {
            let Some(mut graph) = self.store.latest_graph(&s.id).unwrap_or(None) else { continue };
            if graph.status != crate::types::GraphStatus::Executing {
                continue;
            }
            let has_pending = graph.nodes.iter().any(|n| !n.status.is_terminal());
            if !has_pending {
                continue;
            }
            match self.resume_session_graph(&s.id, &mut graph).await {
                Ok(()) => resumed += 1,
                Err(e) => eprintln!("[resume] 会话 {} 续跑失败：{e}", s.id),
            }
        }
        resumed
    }

    async fn resume_session_graph(&self, session_id: &str, graph: &mut crate::types::TaskGraph) -> anyhow::Result<()> {
        use crate::task::{NodeOutcome, Scheduler, TaskGraphModel};
        use futures_util::future::BoxFuture;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // 1) 非终态节点重置为可执行；已完成节点回流回填
        let mut reports: crate::task::Reports = Default::default();
        for n in graph.nodes.iter_mut() {
            if n.status.is_terminal() {
                if n.status == crate::types::TaskStatus::Done {
                    if let Ok(Some(r)) = self.store.sync_report_of_node(&n.id) {
                        reports.insert(n.id.clone(), r);
                    }
                }
            } else {
                n.status = crate::types::TaskStatus::Ready;
                n.retry_count = n.retry_count.saturating_sub(1);
            }
        }
        self.store.save_graph(graph)?;

        // 2) 重建 plan 骨架（收束与账本需要）
        let ledger = self.store.ledger_of(session_id)?;
        let plan = crate::types::OrchestratorPlan {
            route_level: crate::types::RouteLevel::L2,
            playbook: None,
            goal: ledger.task.goal.clone(),
            boundary: crate::types::DispatchBoundary {
                in_scope: ledger.task.constraints.clone(),
                forbidden: ledger.task.forbidden.clone(),
            },
            acceptance: ledger.task.acceptance.clone(),
            nodes: graph
                .nodes
                .iter()
                .map(|n| crate::types::PlanNode {
                    id: n.id.clone(),
                    title: n.title.clone(),
                    agent_identifier: n.agent_identifier.clone(),
                    objective: n.objective.clone(),
                    acceptance: n.acceptance.clone(),
                    depends_on: n.depends_on.clone(),
                    priority: n.priority.clone(),
                })
                .collect(),
            final_answer: None,
        };

        // 3) 重跑调度器（执行器 = Orchestrator 的节点执行路径）；已完成节点状态回放
        let orch = self.orchestrator();
        let mut model = TaskGraphModel::from_plan(&plan, session_id);
        for n in graph.nodes.iter() {
            if n.status == crate::types::TaskStatus::Done {
                model.set_status(&n.id, crate::types::TaskStatus::Done);
                model.set_report_id(&n.id, n.sync_report_id.clone());
            }
        }
        let reports_shared: Arc<Mutex<crate::task::Reports>> = Arc::new(Mutex::new(reports));
        let exec = crate::orchestrator::ExecCtx {
            orchestrator: orch.clone(),
            session_id: session_id.to_string(),
            plan: plan.clone(),
            reports: reports_shared.clone(),
        };
        let exec = Arc::new(exec);
        let scheduler = Scheduler::new(self.config().max_concurrency);
        let store = self.store.clone();
        let events = self.events.clone();
        let sid = session_id.to_string();
        {
            let exec = exec.clone();
            scheduler
                .run(
                    &mut model,
                    move |node| -> BoxFuture<'static, NodeOutcome> {
                        let exec = exec.clone();
                        Box::pin(async move { exec.run_node(node).await })
                    },
                    move |g| {
                        let _ = store.save_graph(&g.to_graph());
                        let evt = crate::types::CoreEvent {
                            kind: "graph.updated".into(),
                            session_id: sid.clone(),
                            payload: serde_json::to_value(g.to_graph()).unwrap_or(serde_json::Value::Null),
                        };
                        let _ = events.send(evt);
                    },
                )
                .await;
        }

        // 4) 收束（复用指挥体收束路径）
        let final_reports = reports_shared.lock().await.clone();
        let final_text = orch.converge(session_id, &plan, &final_reports, &model).await?;
        let statements = crate::orchestrator::text_to_statements(&final_text);
        self.store
            .add_message(
                session_id,
                crate::types::MessageRole::Orchestrator,
                Some(orch.orch_id().as_str()),
                statements.clone(),
            )?;
        let _ = self.events.send(crate::types::CoreEvent {
            kind: "run.finished".into(),
            session_id: session_id.to_string(),
            payload: serde_json::json!({
                "routeLevel": "L2",
                "agentId": orch.orch_id(),
                "graph": model.to_graph(),
                "statements": statements,
                "resumed": true,
            }),
        });
        Ok(())
    }

    fn rebuild_orchestrator(&self) -> anyhow::Result<()> {
        let cfg = self.config();
        let orch = build_orchestrator(&cfg, &self.store, &self.registry, &self.events, &self.memory, Some(self.mcp.clone()), self.remote())?;
        *self.orchestrator.write() = Arc::new(orch);
        Ok(())
    }

    /// 暂存图片附件（随该会话下一轮对话注入规划；每轮取走即清）
    pub fn stage_images(&self, session_id: &str, images: Vec<String>) {
        crate::image_stash::stage(session_id, images);
    }

    pub fn orchestrator(&self) -> Arc<Orchestrator> {
        self.orchestrator.read().clone()
    }

    pub fn config(&self) -> Arc<ExmConfig> {
        self.config.read().clone()
    }

    pub fn is_mock(&self) -> bool {
        self.config.read().use_mock
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.events.subscribe()
    }

    // ---------------- 会话与编成便捷方法 ----------------

    pub fn list_sessions_in_group(&self, gid: &str) -> anyhow::Result<Vec<Session>> {
        Ok(self.store.list_sessions_in_group(gid)?)
    }

    /// 当前交互范围（组或单体）的会话列表
    pub fn list_sessions_in_scope(&self) -> anyhow::Result<Vec<Session>> {
        Ok(self.store.list_sessions_in_group(&self.registry.active_scope())?)
    }

    pub fn delete_session(&self, id: &str) -> anyhow::Result<()> {
        Ok(self.store.delete_session(id)?)
    }

    pub fn create_session(&self, title: &str) -> anyhow::Result<Session> {
        let gid = self.registry.active_scope();
        self.store.create_session(title, &gid)
    }

    pub fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.store.list_sessions()
    }

    pub fn agents(&self) -> Vec<AgentDefinition> {
        self.registry.list()
    }

    pub fn agent(&self, identifier: &str) -> Option<AgentDefinition> {
        self.registry.get(identifier)
    }

    pub fn playbooks(&self) -> anyhow::Result<Vec<Playbook>> {
        self.registry.load_playbooks()
    }

    // ---------------- 智能体组（docs/09） ----------------

    pub fn list_groups(&self) -> Vec<GroupMeta> {
        self.registry.list_groups()
    }

    pub fn active_group(&self) -> String {
        self.registry.active_group()
    }

    pub fn active_group_meta(&self) -> Option<GroupMeta> {
        self.registry.active_group_meta()
    }

    pub fn switch_group(&self, gid: &str) -> anyhow::Result<()> {
        self.registry.set_active_group(gid)
    }

    pub fn create_group(
        &self,
        id: Option<String>,
        name: &str,
        description: &str,
    ) -> anyhow::Result<GroupMeta> {
        self.registry.create_group(id, name, description)
    }

    pub fn delete_group(&self, gid: &str) -> anyhow::Result<()> {
        self.registry.delete_group(gid)
    }

    pub fn group_meta(&self, gid: &str) -> Option<GroupMeta> {
        self.registry.group_meta(gid)
    }

    /// 在激活组内新增/替换个体（用户操作或主智能体经 agent_manage 工具）
    pub fn upsert_agent(
        &self,
        def: AgentDefinition,
        prompt: Option<String>,
    ) -> anyhow::Result<AgentDefinition> {
        let gid = self.registry.active_group();
        self.registry.upsert_agent(&gid, def, prompt)
    }

    pub fn remove_agent(&self, identifier: &str) -> anyhow::Result<()> {
        let gid = self.registry.active_group();
        self.registry.remove_agent(&gid, identifier)
    }

    pub fn set_primary(&self, identifier: &str) -> anyhow::Result<()> {
        let gid = self.registry.active_group();
        self.registry.set_primary(&gid, identifier)
    }

    pub fn registry(&self) -> Arc<LocalRegistry> {
        self.registry.clone()
    }

    // ---------------- 平台能力：技能包 / 定时任务 / 执行审批（docs/10） ----------------

    pub fn skills(&self) -> anyhow::Result<Vec<SkillDef>> {
        self.registry.load_skills()
    }

    pub fn add_skill(&self, skill: SkillDef) -> anyhow::Result<()> {
        self.registry.write_skill(&skill)
    }

    pub fn remove_skill(&self, id: &str) -> anyhow::Result<bool> {
        self.registry.remove_skill(id)
    }

    pub fn approval_list(&self, status: Option<&str>, limit: usize) -> anyhow::Result<Vec<ApprovalRequest>> {
        Ok(self.store.list_approvals(status, limit)?)
    }

    pub fn approval_get(&self, id: &str) -> anyhow::Result<Option<ApprovalRequest>> {
        Ok(self.store.get_approval(id)?)
    }

    /// 审批决定：批准即由系统代执行并记录输出；拒绝即关闭审批单
    pub async fn approval_decide(&self, id: &str, approve: bool) -> anyhow::Result<ApprovalRequest> {
        let mut req = self
            .store
            .get_approval(id)?
            .ok_or_else(|| anyhow::anyhow!("审批单不存在: {id}"))?;
        if req.status != "pending" {
            anyhow::bail!("审批单已处理: {}", req.status);
        }
        req.decided_at = Some(now_iso());
        if !approve {
            req.status = "denied".into();
            self.store.update_approval(&req)?;
            self.emit_approval_resolved(&req);
            return Ok(req);
        }
        let workspace = self.config().workspace_root.clone();
        let out = crate::tools::execute_shell_command_timed(&workspace, &req.command, 300).await;
        req.status = if out.ok { "executed".into() } else { "failed".into() };
        let text = if out.ok { out.output } else { out.error.unwrap_or_default() };
        req.result = Some(text.chars().take(4000).collect());
        self.store.update_approval(&req)?;
        self.emit_approval_resolved(&req);
        Ok(req)
    }

    fn emit_approval_resolved(&self, req: &ApprovalRequest) {
        let _ = self.events.send(CoreEvent {
            kind: "approval.resolved".into(),
            session_id: req.session_id.clone(),
            payload: serde_json::json!({ "approvalId": req.id, "status": req.status, "command": req.command }),
        });
    }

    /// 执行一条定时任务：复用同名会话（缺省 job-<id>），注入提示词并等待收束
    pub async fn run_cron_job(&self, job: &CronJob) -> CronRun {
        let started = now_iso();
        let title = job
            .session_title
            .clone()
            .unwrap_or_else(|| format!("job-{}", job.id));
        // 组切换先行：任务声明的组决定会话归属与执行上下文（执行完恢复）
        let prev_group = self.registry.active_group();
        let mut switched = false;
        if let Some(g) = &job.group {
            if *g != prev_group && self.group_meta(g).is_some() {
                switched = self.registry.set_active_group(g).is_ok();
            }
        }
        // 会话复用（限定本组）：按标题匹配已有会话，避免每次运行新建
        let gid = self.registry.active_group();
        let existing = self
            .store
            .list_sessions_in_group(&gid)
            .ok()
            .and_then(|list| list.into_iter().find(|s| s.title == title));
        let session = match existing {
            Some(s) => s,
            None => self
                .create_session(&title)
                .expect("定时任务会话创建失败"),
        };
        let result = self.chat(&session.id, &job.prompt).await;
        if switched {
            let _ = self.registry.set_active_group(&prev_group);
        }
        let status = if result.is_ok() { "done" } else { "failed" };
        let summary = match result {
            Ok(_) => "任务执行完成（详见会话与任务图）".to_string(),
            Err(e) => format!("执行失败：{e}"),
        };
        let run = CronRun {
            id: new_id()[..8].to_string(),
            job_id: job.id.clone(),
            job_name: job.name.clone(),
            session_id: session.id.clone(),
            started_at: started,
            finished_at: now_iso(),
            status: status.into(),
            summary,
        };
        let _ = self.cron.record_run(&run);
        let _ = self.events.send(CoreEvent {
            kind: "cron.finished".into(),
            session_id: session.id,
            payload: serde_json::json!({
                "jobId": run.job_id, "jobName": run.job_name,
                "status": run.status, "summary": run.summary,
            }),
        });
        run
    }

    /// 指挥体身份 = 激活组主智能体 identifier（自定义组随组切换）
    pub fn orchestrator_id(&self) -> String {
        self.registry
            .primary()
            .map(|p| p.identifier)
            .unwrap_or_else(|| ORCHESTRATOR_ID.to_string())
    }
}

pub fn build_orchestrator(
    cfg: &ExmConfig,
    store: &Arc<Store>,
    registry: &Arc<LocalRegistry>,
    events: &broadcast::Sender<CoreEvent>,
    memory: &Arc<MemoryStore>,
    mcp: Option<Arc<crate::mcp::McpRegistry>>,
    remote: Option<Arc<dyn crate::remote::RemoteExecutor>>,
) -> anyhow::Result<Orchestrator> {
    // 全局通道：有端点与密钥即真实推理；未配置时——测试替身模式给替身，产品模式给「未配置」指引通道
    let provider: Arc<dyn LlmProvider> = {
        let mut keys = cfg.llm.api_keys.clone();
        if keys.is_empty() && !cfg.llm.api_key.trim().is_empty() {
            keys.push(cfg.llm.api_key.clone());
        }
        if keys.is_empty() || cfg.llm.base_url.trim().is_empty() {
            if cfg.use_mock {
                Arc::new(MockLlmProvider)
            } else {
                Arc::new(UnconfiguredProvider::new("全局通道"))
            }
        } else {
            Arc::new(OpenAiCompatibleProvider::with_format(
                cfg.llm.base_url.clone(),
                keys,
                &cfg.llm.api_format,
            ))
        }
    };
    let mcp = mcp.unwrap_or_else(crate::mcp::McpRegistry::shared);
    mcp.configure(&cfg.mcp_servers);
    let tools = Arc::new(
        ToolGateway::new(
            &cfg.workspace_root,
            store.clone(),
            registry.clone(),
            events.clone(),
            cfg.security.clone(),
        )
        .with_mcp(mcp),
    );
    let unit_runtime = Arc::new(AgentRuntime::new(
        registry.clone(),
        provider.clone(),
        tools,
        cfg.llm.unit_model.clone(),
    ));
    // 模型档案运行池：每个档案独立 Provider。
    // 规则：**显式配置优先**——有端点与密钥即真实推理；未配置时，测试替身模式给替身，产品模式给「未配置」指引通道。
    let mut pool = ModelPool::new();
    for p in &cfg.llm_profiles {
        let mut keys = p.api_keys.clone();
        if keys.is_empty() && !p.api_key.trim().is_empty() {
            keys.push(p.api_key.clone());
        }
        let profile_provider: Arc<dyn LlmProvider> = if keys.is_empty() || p.base_url.trim().is_empty() {
            if cfg.use_mock {
                Arc::new(MockLlmProvider)
            } else {
                Arc::new(UnconfiguredProvider::new(format!("档案 {}", p.id)))
            }
        } else {
            Arc::new(OpenAiCompatibleProvider::with_format(
                p.base_url.clone(),
                keys,
                &p.api_format,
            ))
        };
        pool.insert(p.id.clone(), profile_provider, p.orch_model.clone(), p.unit_model.clone());
        if let Some(fb) = &p.fallback {
            pool.set_fallback(&p.id, fb);
        }
        if let Some(em) = &p.embed_model {
            pool.set_embed_model(&p.id, em);
        }
    }
    pool.set_active(cfg.active_profile.clone());
    Ok(Orchestrator {
        registry: registry.clone(),
        orch_provider: provider,
        model_pool: Arc::new(pool),
        failover: Arc::new(FailoverState::default()),
        remote_enabled: remote.is_some(),
        remote,
        unit_runtime,
        store: store.clone(),
        memory: memory.clone(),
        memory_enabled: cfg.memory_enabled,
        memory_recall_limit: cfg.memory_recall_limit,
        memory_md_path: cfg.memory_md_path.clone(),
        events: events.clone(),
        orch_model: cfg.llm.orch_model.clone(),
        max_concurrency: cfg.max_concurrency,
        auto_adapt: cfg.automation.auto_adapt,
    })
}
