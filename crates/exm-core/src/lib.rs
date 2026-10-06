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
pub mod browser;
pub mod computer;
pub mod fsdb;
pub mod image_stash;
pub mod mcp;
pub mod mcp_server;
pub mod memory;
pub mod orchestrator;
pub mod parse;
pub mod patch;
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
            Some(cron.clone()),
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
        let orch = build_orchestrator(&cfg, &self.store, &self.registry, &self.events, &self.memory, Some(self.mcp.clone()), self.remote(), Some(self.cron.clone()))?;
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

    /// 重新生成基础记忆文件（memory.md，全局浅层视图）；
    /// 同时刷新：每组一份 `groups/<gid>/MEMORY.md`（组浅层记忆）、每单体一份 `singles/<id>/MEMORY.md`。
    /// 深层记忆始终在 MemoryStore（数据库检索/衰减/压缩）——MEMORY.md 只是容量受限的人读快照。
    pub fn render_memory_md(&self) -> anyhow::Result<usize> {
        let cfg = self.config();
        let n = self.memory.render_memory_md(&cfg.memory_md_path)?;
        let gid = self.registry.active_group();
        for meta in self.registry.list_groups() {
            if let Some(dir) = self.registry.group_dir(&meta.id) {
                let _ = self.memory.render_group_memory_md(&meta.id, dir.join("MEMORY.md"));
            }
        }
        for s in self.registry.list_singles() {
            let dir = self.registry.singles_dir_pub().join(&s.identifier);
            let _ = self
                .memory
                .render_agent_memory_md(&s.identifier, Some(&gid), dir.join("MEMORY.md"));
        }
        Ok(n)
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

    /// 会话 token 用量：优先真实用量（provider 上报），无记录时退回字符估算（全部消息陈述字符 / 4）
    pub fn session_tokens_estimate(&self, session_id: &str) -> u64 {
        let (prompt, completion, calls) = self.store.usage_total(session_id);
        if calls > 0 {
            return prompt + completion;
        }
        self.store
            .list_messages(session_id, 500)
            .unwrap_or_default()
            .iter()
            .map(|m| m.statements.iter().map(|s| s.text.chars().count() as u64).sum::<u64>() / 4)
            .sum()
    }

    /// 会话用量明细：(prompt, completion, 调用次数)
    pub fn session_usage(&self, session_id: &str) -> (u64, u64, usize) {
        self.store.usage_total(session_id)
    }

    // ---------------- 文件检查点（写前快照的回看与回滚） ----------------

    fn checkpoints_dir(&self) -> std::path::PathBuf {
        self.config().workspace_root.join(".exmachina").join("checkpoints")
    }

    /// 检查点清单：(日期, 相对路径, 字节数)，按日期倒序
    pub fn list_checkpoints(&self) -> Vec<(String, String, u64)> {
        let root = self.checkpoints_dir();
        let mut out: Vec<(String, String, u64)> = Vec::new();
        let Ok(days) = std::fs::read_dir(&root) else { return out };
        for day in days.flatten() {
            if !day.path().is_dir() {
                continue;
            }
            let date = day.file_name().to_string_lossy().to_string();
            let mut stack = vec![day.path()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Ok(rel) = p.strip_prefix(&root).map(|r| r.to_path_buf()) {
                        // 去掉前缀的 <日期>/ 段
                        let rel_str = rel
                            .components()
                            .skip(1)
                            .map(|c| c.as_os_str().to_string_lossy().to_string())
                            .collect::<Vec<_>>()
                            .join("/");
                        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                        out.push((date.clone(), rel_str, size));
                    }
                }
            }
        }
        out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        out
    }

    /// 工作区文件保存（WebUI 编辑器写入口径）：路径防 `..` 兜底、写前检查点、工具审计。
    /// 与写类工具同规则（当日首次改动前的版本落 `.exmachina/checkpoints/`，供回滚）。
    /// 返回结果摘要；越界/非法路径报错。网关侧已做工作区规范化校验，此处兜底拒绝。
    pub fn save_workspace_file(&self, rel: &str, content: &str) -> anyhow::Result<String> {
        let norm = rel.replace('\\', "/").trim_matches('/').to_string();
        if norm.is_empty()
            || std::path::Path::new(rel.trim()).is_absolute()
            || norm.split('/').any(|seg| seg == "..")
        {
            anyhow::bail!("路径越界（仅允许工作区内的相对路径）: {rel}");
        }
        let root = self.config().workspace_root.clone();
        let target = root.join(&norm);
        if target.is_file() {
            let day: String = crate::types::now_iso().chars().take(10).collect();
            let dst = root.join(".exmachina").join("checkpoints").join(&day).join(&norm);
            if !dst.exists() {
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&target, &dst)?;
            }
        } else if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        // 审计：agent_id = webui，与智能体审计同账本、来源可区分
        let _ = self.store.audit_tool(
            "webui",
            "filesystem",
            &serde_json::json!({ "op": "write", "path": norm, "via": "editor" }),
            "编辑器保存",
            0,
        );
        Ok(format!("已保存 {norm}（{} 字节，写前检查点已记录）", content.len()))
    }

    // ---------------- 通道身份与配对码（gateway 身份闸门共用） ----------------

    pub fn identity_of(&self, channel: &str, external_id: &str) -> Option<UserIdentity> {
        self.store.get_identity(channel, external_id).ok().flatten()
    }

    pub fn identity_of_id(&self, id: &str) -> Option<UserIdentity> {
        self.store.get_identity_by_id(id).ok().flatten()
    }

    pub fn identities(&self) -> Vec<UserIdentity> {
        self.store.list_identities().unwrap_or_default()
    }

    pub fn save_identity(&self, identity: &UserIdentity) -> anyhow::Result<()> {
        self.store.put_identity(identity)
    }

    pub fn drop_identity(&self, id: &str) -> anyhow::Result<()> {
        self.store.delete_identity(id)
    }

    /// 签发一次性配对码（8 位短码，TTL 由调用方给）
    pub fn issue_pairing_code(&self, note: &str, platform: Option<&str>, ttl_secs: u64) -> anyhow::Result<PairingCode> {
        let code = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let c = PairingCode {
            code: code.clone(),
            note: note.to_string(),
            channel_platform: platform.map(|s| s.to_string()),
            created_by: "console".into(),
            created_at: now_iso(),
            expires_at_ms: now_ms() + ttl_secs * 1000,
        };
        self.store.put_pairing_code(&c)?;
        Ok(c)
    }

    pub fn pairing_codes(&self) -> Vec<PairingCode> {
        self.store.list_pairing_codes().unwrap_or_default()
    }

    /// 直接落一张配对码（迁移 / 测试 / 控制台定向写入）
    pub fn save_pairing_code(&self, code: &PairingCode) -> anyhow::Result<()> {
        self.store.put_pairing_code(code)
    }

    /// 兑换配对码 → 绑定身份（单次有效 + 过期校验；channel 为通道账号 id）
    pub fn redeem_pairing(
        &self,
        code: &str,
        channel: &str,
        external_id: &str,
        display_name: &str,
        default_role: &str,
    ) -> anyhow::Result<UserIdentity> {
        let c = self
            .store
            .consume_pairing_code(code.trim())
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .ok_or_else(|| anyhow::anyhow!("配对码无效或已被使用"))?;
        if now_ms() > c.expires_at_ms {
            anyhow::bail!("配对码已过期，请重新签发");
        }
        if let Some(p) = &c.channel_platform {
            if !channel.starts_with(p.as_str()) {
                anyhow::bail!("配对码限定平台 {p}，当前通道不可用");
            }
        }
        let identity = UserIdentity {
            id: format!("{channel}:{external_id}"),
            channel: channel.to_string(),
            external_id: external_id.to_string(),
            display_name: display_name.to_string(),
            role: default_role.to_string(),
            paired_at: now_iso(),
            note: c.note.clone(),
        };
        self.store.put_identity(&identity)?;
        Ok(identity)
    }

    /// 供网关在执行前拒绝时发运行错误事件（配额 / 治理类短路，走既有回帖通路）
    pub fn emit_run_error(&self, session_id: &str, message: &str) {
        let _ = self.events.send(crate::types::CoreEvent {
            kind: "run.error".into(),
            session_id: session_id.to_string(),
            payload: serde_json::json!({ "message": message }),
        });
    }

    /// 回滚检查点：`date` 为空取最近一份；返回被覆盖的文件相对路径
    pub fn restore_checkpoint(&self, date: Option<&str>, rel: &str) -> anyhow::Result<String> {
        let target = self.config().workspace_root.join(".exmachina").join("checkpoints");
        let day = match date {
            Some(d) if !d.trim().is_empty() => d.trim().to_string(),
            _ => self
                .list_checkpoints()
                .first()
                .map(|(d, _, _)| d.clone())
                .ok_or_else(|| anyhow::anyhow!("没有可用的检查点"))?,
        };
        let src = target.join(&day).join(rel);
        if !src.is_file() {
            anyhow::bail!("检查点不存在：{day}/{rel}");
        }
        let dst = self.config().workspace_root.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dst)?;
        Ok(format!("已回滚 {} → {rel}（来自检查点 {day}）", dst.display()))
    }

    // ---------------- 会话历史管控（undo / 编辑重发 / fork，组 9） ----------------

    /// 会话运行锁句柄（外部历史操作先 try_lock：处理中一律拒绝）
    fn run_lock(&self, session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.run_locks
            .lock()
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// 快照定位：turn 的消息游标（turn <= 0 = 空，即回退到会话起点）
    fn cursor_at_turn(&self, session_id: &str, turn: usize) -> anyhow::Result<usize> {
        if turn == 0 {
            return Ok(0);
        }
        let snaps = self.store.list_turn_snapshots(session_id)?;
        snaps
            .iter()
            .find(|s| s.turn == turn)
            .map(|s| s.message_count)
            .ok_or_else(|| anyhow::anyhow!("轮次快照不存在: turn {turn}（该轮未落快照或已被裁剪）"))
    }

    /// 消息撤销（9.2）：空闲强校验 → 游标回退（截断部分归档可查）→ 上下文重建
    /// → 可选检查点联动回滚 → 审计事件。返回 {archived, restored, cursor}。
    pub async fn undo_session(&self, session_id: &str, upto_turn: usize, restore_files: bool) -> anyhow::Result<serde_json::Value> {
        let lock = self.run_lock(session_id);
        // 空闲校验：处理中拒绝（spec 硬性要求）
        let _guard = lock.try_lock().map_err(|_| anyhow::anyhow!("会话正在处理中，请等待当前任务完成后再撤销"))?;
        self.store
            .get_session(session_id)?
            .ok_or_else(|| anyhow::anyhow!("会话不存在: {session_id}"))?;
        let cursor = self.cursor_at_turn(session_id, upto_turn)?;

        // 游标回退 + 归档（spec：被截断内容保留归档视图）
        let archived = self.store.truncate_messages(session_id, cursor)?;

        // 上下文重建：截断后滚动摘要失效（摘要覆盖越过游标即重置，按截断后历史重建）
        let session = self.store.get_session(session_id)?.unwrap();
        if session.summary_upto.map(|u| u > cursor).unwrap_or(false) {
            let _ = self.store.update_compaction(session_id, "", 0);
        }

        // 检查点联动回滚：该轮之后各快照登记的文件变更，按轮次倒序恢复
        let mut restored: Vec<String> = Vec::new();
        if restore_files {
            let mut later: Vec<TurnSnapshot> = self
                .store
                .list_turn_snapshots(session_id)?
                .into_iter()
                .filter(|s| s.turn > upto_turn && !s.checkpoint_ids.is_empty())
                .collect();
            later.sort_by(|a, b| b.turn.cmp(&a.turn)); // 后轮先回滚（后者覆盖前者的场景）
            for snap in later {
                for cp in snap.checkpoint_ids.iter().rev() {
                    let Some((date, rel)) = cp.split_once('/') else { continue };
                    match self.restore_checkpoint(Some(date), rel) {
                        Ok(_) => restored.push(cp.clone()),
                        Err(e) => eprintln!("[undo] 检查点恢复失败 {cp}: {e}"),
                    }
                }
            }
        }

        // 孤儿快照清理 + 审计 + 事件
        let _ = self.store.prune_turn_snapshots_after(session_id, upto_turn);
        let _ = self.store.audit_tool(
            "webui",
            "session_undo",
            &serde_json::json!({ "session": session_id, "uptoTurn": upto_turn, "restoreFiles": restore_files }),
            &format!("撤销至第 {upto_turn} 轮：归档 {archived} 条，联动回滚 {} 个文件", restored.len()),
            0,
        );
        let payload = serde_json::json!({
            "session": session_id, "uptoTurn": upto_turn, "cursor": cursor,
            "archived": archived, "restored": restored,
        });
        let _ = self.events.send(CoreEvent { kind: "session.undo".into(), session_id: session_id.to_string(), payload: payload.clone() });
        Ok(payload)
    }

    /// 编辑重发（9.3）：撤销至该轮之前（原内容归档可查）后以新内容触发新一轮处理。
    /// `turn` 为被编辑的用户消息所在轮次。
    pub async fn edit_resend(&self, session_id: &str, turn: usize, new_text: &str) -> anyhow::Result<serde_json::Value> {
        if new_text.trim().is_empty() {
            anyhow::bail!("新内容不能为空");
        }
        let upto = turn.saturating_sub(1);
        let info = self.undo_session(session_id, upto, true).await?;
        // 以新内容重新执行（原内容已在归档中可查）
        self.chat(session_id, new_text).await?;
        Ok(serde_json::json!({ "undone": info, "resent": true, "turn": turn }))
    }

    /// 会话分支派生（9.4）：新会话携带截至该轮的完整历史副本与派生来源标注；
    /// 组归属（= 权限与成员上下文）继承，原会话不变。返回新会话。
    pub fn fork_session(&self, session_id: &str, upto_turn: usize) -> anyhow::Result<Session> {
        let src = self
            .store
            .get_session(session_id)?
            .ok_or_else(|| anyhow::anyhow!("会话不存在: {session_id}"))?;
        let cursor = self.cursor_at_turn(session_id, upto_turn)?;
        let now = now_iso();
        let fork = Session {
            id: new_id(),
            title: format!("{}（分支·至第 {upto_turn} 轮）", src.title),
            status: "active".into(),
            group_id: src.group_id.clone(), // 权限与组继承
            rolling_summary: None,          // 摘要按副本历史重建
            summary_upto: None,
            parent_session: Some(src.id.clone()),
            parent_upto: Some(upto_turn),
            ledger: src.ledger.clone(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.store.insert_session(&fork)?;
        let copied = self.store.copy_messages_prefix(session_id, &fork.id, cursor)?;
        let _ = self.store.audit_tool(
            "webui",
            "session_fork",
            &serde_json::json!({ "source": session_id, "uptoTurn": upto_turn, "fork": fork.id }),
            &format!("派生分支会话 {}（复制 {copied} 条消息）", fork.id),
            0,
        );
        let _ = self.events.send(CoreEvent {
            kind: "session.fork".into(),
            session_id: fork.id.clone(),
            payload: serde_json::json!({ "source": session_id, "uptoTurn": upto_turn, "fork": fork.id, "copied": copied }),
        });
        Ok(fork)
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
        let orch = build_orchestrator(&cfg, &self.store, &self.registry, &self.events, &self.memory, Some(self.mcp.clone()), self.remote(), Some(self.cron.clone()))?;
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

    /// 登记会话的外部来源（闸门放行时由适配器调用；审批单创建时据此打升级标）
    pub fn stamp_session_origin(&self, session_id: &str, external_id: &str, role: &str) {
        let _ = self.store.put_session_origin(session_id, external_id, role);
    }

    pub fn session_origin(&self, session_id: &str) -> Option<(String, String)> {
        self.store.get_session_origin(session_id).ok().flatten()
    }

    /// 创建 WebUI 发起的 git 破坏性操作审批单：command 为精确 git 命令，
    /// 批准后由 approval_decide 代执行（与工具审批同一执行与审计路径）
    pub async fn request_git_approval(&self, command: &str) -> anyhow::Result<ApprovalRequest> {
        let req = ApprovalRequest {
            id: crate::types::new_id(),
            session_id: String::new(),
            node_id: None,
            agent_id: "webui".into(),
            command: command.to_string(),
            status: "pending".into(),
            result: None,
            created_at: now_iso(),
            decided_at: None,
            origin_user: None,
            origin_role: None,
        };
        self.store.add_approval(&req)?;
        let _ = self.events.send(CoreEvent {
            kind: "approval.required".into(),
            session_id: String::new(),
            payload: serde_json::json!({ "approvalId": req.id, "agentId": "webui", "command": command }),
        });
        Ok(req)
    }

    fn emit_approval_resolved(&self, req: &ApprovalRequest) {        let _ = self.events.send(CoreEvent {
            kind: "approval.resolved".into(),
            session_id: req.session_id.clone(),
            payload: serde_json::json!({ "approvalId": req.id, "status": req.status, "command": req.command }),
        });
    }

    /// 注入一条事件提示词到目标组会话（事件触发器共用通路，design D6：
    /// 组切换 + 会话复用 + prompt 注入；与 cron 同路但独立记账，不进 cron 运行日志）。
    /// 返回执行的会话 id。标题缺省 = `event-<group>`（同组事件复用同一会话）。
    pub async fn run_event_prompt(
        &self,
        title: &str,
        group: Option<&str>,
        prompt: &str,
    ) -> anyhow::Result<String> {
        let prev_group = self.registry.active_group();
        let mut switched = false;
        if let Some(g) = group {
            if *g != prev_group && self.group_meta(g).is_some() {
                switched = self.registry.set_active_group(g).is_ok();
            }
        }
        let gid = self.registry.active_group();
        let title = if title.trim().is_empty() { format!("event-{gid}") } else { title.to_string() };
        let result = async {
            let existing = self
                .store
                .list_sessions_in_group(&gid)
                .ok()
                .and_then(|list| list.into_iter().find(|s| s.title == title));
            let session = match existing {
                Some(s) => s,
                None => self.create_session(&title)?,
            };
            self.chat(&session.id, prompt).await?;
            Ok::<String, anyhow::Error>(session.id)
        }
        .await;
        if switched {
            let _ = self.registry.set_active_group(&prev_group);
        }
        result
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
    cron: Option<Arc<CronStore>>,
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
    let mut gateway = ToolGateway::new(
        &cfg.workspace_root,
        store.clone(),
        registry.clone(),
        events.clone(),
        cfg.security.clone(),
    )
    .with_search(cfg.search.clone())
    .with_hooks(cfg.hooks.clone())
    .with_sandbox(cfg.sandbox.clone())
    .with_browser(cfg.browser.clone())
    .with_computer(cfg.computer.clone())
    .with_custom_tools(cfg.tools.custom.clone())
    .with_mcp(mcp);
    if let Some(c) = &cron {
        gateway = gateway.with_cron(c.clone());
    }
    // 记忆工具（memory_read/write/link）的受控入口：组隔离在工具层强制
    gateway = gateway.with_memory(memory.clone());
    let tools = Arc::new(gateway);
    let unit_runtime = Arc::new(AgentRuntime::new(
        registry.clone(),
        provider.clone(),
        tools.clone(),
        cfg.llm.model.clone(),
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
        pool.insert(p.id.clone(), profile_provider, p.model.clone());
        pool.set_models(p.id.clone(), p.models.clone());
        if let Some(fb) = &p.fallback {
            pool.set_fallback(&p.id, fb);
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
        tools,
        store: store.clone(),
        memory: memory.clone(),
        memory_enabled: cfg.memory_enabled,
        memory_recall_limit: cfg.memory_recall_limit,
        memory_md_path: cfg.memory_md_path.clone(),
        events: events.clone(),
        orch_model: cfg.llm.model.clone(),
        memory_semantic_model: cfg.memory_semantic_model.clone(),
        speech_target: cfg.speech_model.clone(),
        stt_target: cfg.stt_model.clone(),
        vision_relay_target: cfg.vision_relay_model.clone(),
        max_concurrency: cfg.max_concurrency,
        max_unit_steps: cfg.automation.unit_max_steps,
        auto_adapt: cfg.automation.auto_adapt,
    })
}

// ---------------------------------------------------------------- 会话历史管控（组 9 单测）

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::types::{AgentDefinition, Tier};

    fn mock_core() -> Core {
        let dir = std::env::temp_dir().join(format!("exm-hist-{}", crate::types::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = ExmConfig::load(&dir);
        cfg.use_mock = true;
        cfg.memory_enabled = false; // 历史管控测试不关心记忆写入
        let core = Core::with_config(cfg).expect("创建 Core 失败");
        core.registry().create_group(Some("t".into()), "历史测试组", "").unwrap();
        let def = AgentDefinition {
            name: "问答体".into(),
            identifier: "hist-orch".into(),
            domain: "测试".into(),
            tier: Tier::Orchestrator,
            description: "测试主智能体".into(),
            capabilities: vec![],
            tools: vec![],
            when_to_call: String::new(),
            dependencies: vec![],
            composable_with: vec![],
            input_schema: Default::default(),
            output_schema: Default::default(),
            prompt_file: String::new(),
            model_hint: None,
        };
        core.registry().upsert_agent("t", def, None).unwrap();
        core.registry().set_primary("t", "hist-orch").unwrap();
        core.registry().set_active_group("t").unwrap();
        core
    }

    async fn three_rounds(core: &Core, sid: &str) {
        core.chat(sid, "第一轮问题").await.unwrap();
        core.chat(sid, "第二轮问题").await.unwrap();
        core.chat(sid, "第三轮问题").await.unwrap();
    }

    #[tokio::test]
    async fn 轮次快照_多轮对话后快照链完整() {
        let core = mock_core();
        let s = core.create_session("快照链").unwrap();
        three_rounds(&core, &s.id).await;
        let snaps = core.store.list_turn_snapshots(&s.id).unwrap();
        assert_eq!(snaps.len(), 3, "每轮一条快照");
        for (i, snap) in snaps.iter().enumerate() {
            assert_eq!(snap.turn, i + 1, "轮次从 1 递增");
            assert_eq!(snap.status, "done");
        }
        // 游标单调不减且等于当前消息总数
        let total = core.store.list_messages(&s.id, 0).unwrap().len();
        assert_eq!(snaps.last().unwrap().message_count, total);
        assert!(snaps.windows(2).all(|w| w[0].message_count <= w[1].message_count), "游标单调");
        let _ = std::fs::remove_dir_all(&core.config().workspace_root);
    }

    #[tokio::test]
    async fn undo_空闲回退_归档可查_快照裁剪() {
        let core = mock_core();
        let s = core.create_session("撤销").unwrap();
        three_rounds(&core, &s.id).await;
        let total = core.store.list_messages(&s.id, 0).unwrap().len();

        let info = core.undo_session(&s.id, 2, false).await.unwrap();
        assert_eq!(info["archived"].as_u64().unwrap() as usize, total - snaps_at(&core, &s.id, 2), "归档数 = 第 2 轮之后的消息数");
        // 活跃流截断到第 2 轮游标
        let cursor = core.store.list_turn_snapshots(&s.id).unwrap().last().unwrap().message_count;
        let now_len = core.store.list_messages(&s.id, 0).unwrap().len();
        assert_eq!(now_len, cursor, "截断后游标一致");
        assert!(now_len < total, "确实发生了截断");
        // 归档可查：被截断内容保留
        let archive = core.store.list_archived_messages(&s.id).unwrap();
        assert_eq!(archive.len(), total - now_len);
        assert!(archive.iter().any(|m| m.statements.iter().any(|st| st.text.contains("第三轮"))), "第三轮内容在归档中");
        // 孤儿快照已裁剪
        assert_eq!(snaps_len(&core, &s.id), 2);
        let _ = std::fs::remove_dir_all(&core.config().workspace_root);
    }

    fn snaps_len(core: &Core, sid: &str) -> usize {
        core.store.list_turn_snapshots(sid).unwrap().len()
    }

    fn snaps_at(core: &Core, sid: &str, turn: usize) -> usize {
        core.store
            .list_turn_snapshots(sid)
            .unwrap()
            .iter()
            .find(|s| s.turn == turn)
            .map(|s| s.message_count)
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn undo_处理中拒绝() {
        let core = mock_core();
        let s = core.create_session("忙碌").unwrap();
        core.chat(&s.id, "先来一轮").await.unwrap();
        // 占住运行锁（模拟处理中）
        let lock = core.run_lock(&s.id);
        let guard = lock.lock().await;
        let err = core.undo_session(&s.id, 1, false).await.unwrap_err();
        assert!(err.to_string().contains("处理中"), "应拒绝：{err}");
        drop(guard);
        // 空闲后可撤销
        assert!(core.undo_session(&s.id, 1, false).await.is_ok());
        let _ = std::fs::remove_dir_all(&core.config().workspace_root);
    }

    #[tokio::test]
    async fn undo_检查点联动回滚文件() {
        let core = mock_core();
        let s = core.create_session("文件联动").unwrap();
        // 构造两轮：第 1 轮登记检查点（文件 v1 → v2），第 2 轮普通
        let file = core.config().workspace_root.join("a.txt");
        std::fs::write(&file, "v1").unwrap();
        let day: String = now_iso().chars().take(10).collect();
        // 检查点文件（写类工具写前快照的同款落点）+ 轮次链登记
        let cp = core.config().workspace_root.join(".exmachina").join("checkpoints").join(&day).join("a.txt");
        std::fs::create_dir_all(cp.parent().unwrap()).unwrap();
        std::fs::write(&cp, "v1").unwrap();
        core.store.record_checkpoint_ref(&s.id, &day, "a.txt").unwrap();
        std::fs::write(&file, "v2").unwrap();
        core.store
            .put_turn_snapshot(&TurnSnapshot {
                id: format!("{}:1", s.id),
                session_id: s.id.clone(),
                turn: 1,
                message_count: 2,
                checkpoint_ids: vec![format!("{day}/a.txt")],
                prompt_tokens: 0,
                completion_tokens: 0,
                status: "done".into(),
                created_at: now_iso(),
            })
            .unwrap();
        core.store
            .add_message(&s.id, MessageRole::User, None, vec![Statement::new(SpeechTag::要求, "改文件")])
            .unwrap();
        core.store
            .put_turn_snapshot(&TurnSnapshot {
                id: format!("{}:2", s.id),
                session_id: s.id.clone(),
                turn: 2,
                message_count: 3,
                checkpoint_ids: vec![],
                prompt_tokens: 0,
                completion_tokens: 0,
                status: "done".into(),
                created_at: now_iso(),
            })
            .unwrap();

        // 撤销到第 1 轮（保留）→ 第 2 轮无检查点；撤销到第 0 轮 → 第 1 轮的检查点回滚文件
        let info = core.undo_session(&s.id, 0, true).await.unwrap();
        let restored: Vec<String> = serde_json::from_value(info["restored"].clone()).unwrap();
        assert_eq!(restored, vec![format!("{day}/a.txt")], "检查点引用被联动回滚");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1", "文件内容回到检查点版本");
        assert_eq!(core.store.list_messages(&s.id, 0).unwrap().len(), 0, "游标回退到 0");
        let _ = std::fs::remove_dir_all(&core.config().workspace_root);
    }

    #[tokio::test]
    async fn 编辑重发_原内容归档_新内容重执行() {
        let core = mock_core();
        let s = core.create_session("改写").unwrap();
        three_rounds(&core, &s.id).await;
        let before = core.store.list_messages(&s.id, 0).unwrap().len();

        core.edit_resend(&s.id, 2, "第二轮问题（已修改）").await.unwrap();
        // 原内容归档可查
        let archive_text: String = core
            .store
            .list_archived_messages(&s.id)
            .unwrap()
            .iter()
            .flat_map(|m| m.statements.iter().map(|st| st.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(archive_text.contains("第二轮问题"), "原第 2 轮内容在归档中：{archive_text}");
        assert!(archive_text.contains("第三轮问题"), "第 3 轮一并被回退归档");
        // 新内容重执行：活跃流 = 第 1 轮 + 新一轮，最后一条用户消息为新文本
        let msgs = core.store.list_messages(&s.id, 0).unwrap();
        assert!(msgs.len() < before + 2, "重发后活跃流短于原全程");
        let last_user = msgs.iter().rev().find(|m| matches!(m.role, MessageRole::User)).unwrap();
        assert!(last_user.statements[0].text.contains("已修改"), "以编辑后的内容重执行");
        // 新一轮也落了快照
        let snaps = core.store.list_turn_snapshots(&s.id).unwrap();
        assert_eq!(snaps.len(), 2, "第 1 轮保留 + 重发轮新快照");
        assert_eq!(snaps.last().unwrap().turn, 2);
        let _ = std::fs::remove_dir_all(&core.config().workspace_root);
    }

    #[tokio::test]
    async fn fork_派生独立演进_原会话不变() {
        let core = mock_core();
        let s = core.create_session("分叉源").unwrap();
        three_rounds(&core, &s.id).await;
        let src_msgs = core.store.list_messages(&s.id, 0).unwrap().len();
        let src_snaps = snaps_len(&core, &s.id);

        let fork = core.fork_session(&s.id, 2).unwrap();
        assert_ne!(fork.id, s.id);
        assert_eq!(fork.parent_session.as_deref(), Some(s.id.as_str()), "派生来源标注");
        assert_eq!(fork.parent_upto, Some(2));
        assert_eq!(fork.group_id, "t", "组归属继承");
        // 历史副本 = 截至第 2 轮
        let copied = core.store.list_messages(&fork.id, 0).unwrap();
        let cursor = core.store.list_turn_snapshots(&s.id).unwrap().iter().find(|x| x.turn == 2).unwrap().message_count;
        assert_eq!(copied.len(), cursor, "副本 = 前缀游标");
        assert!(copied.len() < src_msgs);
        // 原会话不变
        assert_eq!(core.store.list_messages(&s.id, 0).unwrap().len(), src_msgs);
        assert_eq!(snaps_len(&core, &s.id), src_snaps);
        // 分支独立演进：新消息只进分支
        core.chat(&fork.id, "分支上的新问题").await.unwrap();
        assert!(core.store.list_messages(&fork.id, 0).unwrap().len() > copied.len());
        assert_eq!(core.store.list_messages(&s.id, 0).unwrap().len(), src_msgs, "原会话不受分支影响");
        let _ = std::fs::remove_dir_all(&core.config().workspace_root);
    }

    #[tokio::test]
    async fn undo_不存在的轮次与会话报错() {
        let core = mock_core();
        let s = core.create_session("边界").unwrap();
        assert!(core.undo_session(&s.id, 5, false).await.is_err(), "无快照轮次拒绝");
        assert!(core.undo_session("no-such-session", 1, false).await.is_err(), "不存在会话拒绝");
        assert!(core.fork_session("no-such-session", 1).is_err());
        let _ = std::fs::remove_dir_all(&core.config().workspace_root);
    }
}
