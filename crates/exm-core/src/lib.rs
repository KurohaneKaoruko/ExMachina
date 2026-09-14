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
use crate::provider::{FailoverState, LlmProvider, MockLlmProvider, ModelPool, OpenAiCompatibleProvider};
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
            orchestrator: RwLock::new(orchestrator),
            config: RwLock::new(Arc::new(cfg)),
        })
    }

    /// 配置热更新（设置页）：重建 Provider / Runtime / Orchestrator
    pub fn apply_config(&self, cfg: ExmConfig) -> anyhow::Result<()> {
        cfg.save()?;
        self.mcp.configure(&cfg.mcp_servers);
        let orch = build_orchestrator(&cfg, &self.store, &self.registry, &self.events, &self.memory, Some(self.mcp.clone()))?;
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
    pub async fn chat(&self, session_id: &str, text: &str) -> anyhow::Result<()> {
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
    pub fn approval_decide(&self, id: &str, approve: bool) -> anyhow::Result<ApprovalRequest> {
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
        let out = crate::tools::execute_shell_command(&workspace, &req.command);
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
) -> anyhow::Result<Orchestrator> {
    let provider: Arc<dyn LlmProvider> = if cfg.use_mock {
        Arc::new(MockLlmProvider)
    } else {
        let mut keys = cfg.llm.api_keys.clone();
        if keys.is_empty() && !cfg.llm.api_key.trim().is_empty() {
            keys.push(cfg.llm.api_key.clone());
        }
        Arc::new(OpenAiCompatibleProvider::with_format(
            cfg.llm.base_url.clone(),
            keys,
            &cfg.llm.api_format,
        ))
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
    // 模型档案运行池：每个档案独立 Provider（密钥空缺的档案退化为 Mock，行为与全局一致）
    let mut pool = ModelPool::new();
    for p in &cfg.llm_profiles {
        let mut keys = p.api_keys.clone();
        if keys.is_empty() && !p.api_key.trim().is_empty() {
            keys.push(p.api_key.clone());
        }
        let profile_provider: Arc<dyn LlmProvider> = if keys.is_empty() {
            Arc::new(MockLlmProvider)
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
