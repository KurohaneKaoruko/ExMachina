//! 指挥体宿主 —— docs/01 §2.1
//! 收拢 → 分解 → 选路 → 派发 → 监控 → 裁决 → 收束

use crate::memory::{MemoryDraft, MemoryKind, MemoryStore};
use crate::parse;
use crate::provider::{ChatMessage, ChatRequest, FailoverState, LlmProvider, ModelChain, ModelPool};
use crate::registry::LocalRegistry;
use crate::runtime::AgentRuntime;
use crate::store::Store;
use crate::task::{NewNode, NodeOutcome, Reports, Scheduler, TaskGraphModel};
use crate::types::*;
use futures_util::future::BoxFuture;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc::unbounded_channel, Mutex};

pub const ORCHESTRATOR_ID: &str = "exmachina-orchestrator";

/// 自定义组的规划契约：主智能体提示词 + 该契约 = 指挥体能力
pub const PLANNING_CONTRACT: &str = "

---

## 指挥体规划契约（系统追加，必须遵守）

你是本组的主智能体（指挥体）：收拢用户输入 → 锁定任务边界 → 分解为可并行的子任务图 → 派发组内子个体 → 汇总裁决收束。

## 输出契约（OrchestratorPlan，系统强制校验）

只输出一个 JSON 代码块（```json ... ```），结构：
{
  \"routeLevel\": \"L0|L1|L2|L3\",
  \"goal\": \"<用户目标>\",
  \"boundary\": { \"inScope\": [\"…\"], \"forbidden\": [\"…\"] },
  \"acceptance\": [\"可验证的验收口径\"],
  \"nodes\": [{
    \"id\": \"T1\", \"title\": \"…\", \"agentIdentifier\": \"<组内子个体 identifier>\",
    \"objective\": \"<该节点要完成什么>\", \"acceptance\": [\"…\"],
    \"dependsOn\": [], \"priority\": \"P0|P1|P2|P3\"
  }],
  \"finalAnswer\": [{ \"tag\": \"报告|肯定|…\", \"text\": \"…\" }]
}
- L0 = 本机直接回答（此时 nodes 为空、finalAnswer 必填）；L1 单链；L2 并行；L3 含裁决。
- agentIdentifier 只能取自下方可调度子个体清单。
- 创建/修改组内个体必须由用户明确要求；用户未要求时禁止规划任何个体管理类节点。";

/// 单体模式契约：单体智能体直接完成任务，不派发
pub const SINGLE_CONTRACT: &str = r#"""

---

## 单体契约（系统追加，必须遵守）

你是单体智能体：独立直接服务于用户，本范围内没有任何可调度的子个体。
禁止规划任何派发节点。

## 输出契约（OrchestratorPlan，运行时强制校验）

只输出一个 JSON 代码块（```json ... ```）：
{
  "routeLevel": "L0",
  "goal": "<用户目标>",
  "boundary": { "inScope": ["…"], "forbidden": ["…"] },
  "acceptance": ["…"],
  "nodes": [],
  "finalAnswer": [{ "tag": "报告|肯定|…", "text": "…" }]
}
- routeLevel 必须为 L0；nodes 必须为空；finalAnswer 必填（完整回答放这里）。"#;

pub struct Orchestrator {
    pub registry: Arc<LocalRegistry>,
    pub orch_provider: Arc<dyn LlmProvider>,
    /// 模型档案运行池：按个体/组默认模型解析生效 (Provider, 模型名)
    pub model_pool: Arc<ModelPool>,
    /// 失败冷却状态（档案失败后一段时间内被回退链跳过）
    pub failover: Arc<FailoverState>,
    /// 远程执行器（工作者节点池；None = 全本地执行）
    pub remote: Option<Arc<dyn crate::remote::RemoteExecutor>>,
    /// 远程派发开关（工作者在线即路由远程，失败回落本地）
    pub remote_enabled: bool,
    pub unit_runtime: Arc<AgentRuntime>,
    pub store: Arc<Store>,
    pub memory: Arc<MemoryStore>,
    pub memory_enabled: bool,
    pub memory_recall_limit: usize,
    pub memory_md_path: PathBuf,
    pub events: broadcast::Sender<CoreEvent>,
    pub orch_model: String,
    /// 语义检索目标："档案ID" 或 "档案ID/模型名"；空 = 仅词项召回（由记忆设置指定）
    pub memory_semantic_model: String,
    /// 能力模型槽位（"档案ID" 或 "档案ID/模型名"）：语音合成 / 语音转述 / 视觉转述
    pub speech_target: String,
    pub stt_target: String,
    pub vision_relay_target: String,
    pub max_concurrency: usize,
    /// 新教训达阈值时自动提炼经验改进要点（docs/10 §5）
    pub auto_adapt: bool,
}

impl Orchestrator {
    /// 能力槽位目标（组覆盖优先）：激活组的组级覆盖 → 全局槽位（模型设置页）。
    /// 返回 None = 该槽位全局与组都未配置（调用方回落出厂默认）。
    fn capability_target(&self, slot: &str) -> Option<String> {
        let from_group = self
            .registry
            .active_group_meta()
            .and_then(|m| m.capabilities)
            .and_then(|c| match slot {
                "speech" => c.speech,
                "transcribe" => c.transcribe,
                "vision_relay" => c.vision_relay,
                "embedding" => c.embedding,
                _ => None,
            });
        let target = from_group.or_else(|| match slot {
            "speech" => Some(self.speech_target.clone()),
            "transcribe" => Some(self.stt_target.clone()),
            "vision_relay" => Some(self.vision_relay_target.clone()),
            "embedding" => Some(self.memory_semantic_model.clone()),
            _ => None,
        });
        target.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// 指挥体身份 = 激活组主智能体 identifier（组感知；内置组即 exmachina-orchestrator）。
    /// 语音转写：组覆盖 → 全局槽位指定的档案/模型；未配置回落全局生效档案的 whisper-1
    pub async fn transcribe_audio(&self, audio: &[u8], filename: &str) -> anyhow::Result<String> {
        match self.capability_target("transcribe").and_then(|t| self.resolve_capability(&t)) {
            Some((provider, model)) => provider.transcribe(&model, audio, filename).await,
            None => self.orch_provider.transcribe("whisper-1", audio, filename).await,
        }
    }

    /// 语音合成：组覆盖 → 全局槽位指定的档案/模型；未配置回落全局生效档案的 tts-1
    pub async fn speak_audio(&self, text: &str) -> anyhow::Result<Vec<u8>> {
        match self.capability_target("speech").and_then(|t| self.resolve_capability(&t)) {
            Some((provider, model)) => provider.speak(&model, text).await,
            None => self.orch_provider.speak("tts-1", text).await,
        }
    }

    /// 视觉转述：生效模型未标记支持视觉（清单里明确 vision=false）且配置了转述模型时，
    /// 把图片逐张交给转述模型转成文字描述。返回 None = 无需/无法转述（保持直通或忽略）。
    async fn relay_images(&self, images: &[String]) -> Option<String> {
        if images.is_empty() {
            return None;
        }
        // 目标模型视觉能力：链首候选（真正会接这批图片的模型）
        let vision = self
            .orch_candidates()
            .first()
            .and_then(|(pid, _, model)| self.model_pool.capability(pid, model, true));
        match vision {
            Some(true) | None => return None, // 支持视觉 / 能力未知（保持直通）
            Some(false) => {}
        }
        let (provider, model) = self
            .capability_target("vision_relay")
            .and_then(|t| self.resolve_capability(&t))?;
        let mut out = String::new();
        for (i, img) in images.iter().enumerate() {
            let mut msg = ChatMessage::user(
                "请用中文详细描述这张图片的内容：主体与场景、可见文字、数据与关键细节。只输出描述正文。",
            );
            msg.images = vec![img.clone()];
            let req = ChatRequest::new(model.clone(), vec![msg]);
            match provider.chat(req).await {
                Ok(resp) if !resp.content.trim().is_empty() => {
                    out.push_str(&format!("【图片{}】{}\n", i + 1, resp.content.trim()));
                }
                Ok(_) => {}
                Err(e) => eprintln!("[vision-relay] 图片转述失败：{e}"),
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// memory.md 超限自主压缩：LLM 简略化，旧全文归档（深层开 = 存数据库；关 = 存工作区归档文件）
    /// 返回 (压缩前字数, 压缩后字数)；未超限返回 Ok((len, len))
    pub async fn compact_memory_md(&self, max_chars: usize) -> anyhow::Result<(usize, usize)> {
        let md = std::fs::read_to_string(&self.memory_md_path).unwrap_or_default();
        let len = md.chars().count();
        if len <= max_chars {
            return Ok((len, len));
        }
        let system = "你是记忆压缩器【记忆压缩】。把 memory.md 压简为要点清单：保留目标、偏好、事实与教训的关键信息，删除重复与冗余；只输出压缩后的 Markdown 正文，不要任何前后缀。";
        let user = format!(
            "以下 memory.md 超出上限（{len} 字符 > {max_chars}），请压缩到约 {} 字符以内：\n\n{md}",
            max_chars * 4 / 5
        );
        let req = ChatRequest::new(
            self.orch_model.clone(),
            vec![ChatMessage::system(system), ChatMessage::user(user)],
        );
        let resp = self.orch_provider.chat(req).await?;
        let compressed = resp.content.trim().to_string();
        anyhow::ensure!(!compressed.is_empty(), "压缩结果为空");

        // 归档：深层开 → 存数据库（digest 条目，全局可见）；关 → 追加工作区归档文件
        if self.memory_enabled {
            let draft = MemoryDraft::new(MemoryKind::Digest, format!("memory.md 归档 {}", now_iso()), md.clone())
                .importance(0.4);
            let _ = self.memory.remember(&draft);
        } else {
            let archive = self.memory_md_path.with_file_name("memory_archive.md");
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&archive) {
                use std::io::Write as _;
                let _ = writeln!(f, "\n## 归档 {}\n\n{md}\n", now_iso());
            }
        }
        std::fs::write(&self.memory_md_path, &compressed)?;
        Ok((len, compressed.chars().count()))
    }

    /// 工作者端执行入口：远程派发在本地执行（供 WorkerSession 调用）
    pub async fn execute_unit<F>(
        &self,
        def: &AgentDefinition,
        order: &DispatchOrder,
        mut on_token: F,
    ) -> anyhow::Result<SyncReport>
    where
        F: FnMut(&str),
    {
        let chain = self.unit_chain_for(def);
        self.unit_runtime.execute(def, order, chain, |d| on_token(d)).await
    }

    pub fn orch_id(&self) -> String {
        self.registry
            .primary()
            .map(|p| p.identifier)
            .unwrap_or_else(|| ORCHESTRATOR_ID.to_string())
    }

    // ---------------------------------------------------------- 模型解析

    /// 当前交互目标的模型提示：单体模式 = 该单体 model_hint；组模式 = 组默认模型
    fn target_model_hint(&self) -> Option<String> {
        if let Some(id) = self.registry.active_single() {
            return self.registry.single(&id).and_then(|d| d.model_hint);
        }
        self.registry.active_group_meta().and_then(|m| m.model)
    }

    /// 指挥体生效模型：目标提示 → 全局生效档案
    fn resolve_orch_model(&self) -> (Arc<dyn LlmProvider>, String) {
        self.orch_candidates()
            .into_iter()
            .next()
            .map(|(_, p, m)| (p, m))
            .unwrap_or_else(|| (self.orch_provider.clone(), self.orch_model.clone()))
    }

    /// 回退链展开：起始档案 → fallback 链 → 全局生效档案兜底；冷却中的档案跳过（保底留一个）
    fn expand_chain(&self, start: &str) -> Vec<(String, Arc<dyn LlmProvider>, String)> {
        let mut ids = self.model_pool.chain(start);
        let active = self.model_pool.active_id();
        if !active.is_empty() && !ids.contains(&active) {
            ids.push(active);
        }
        let total = ids.len();
        let mut out: Vec<(String, Arc<dyn LlmProvider>, String)> = Vec::new();
        for (n, id) in ids.into_iter().enumerate() {
            let Some((provider, model)) = self.model_pool.entry(&id) else { continue };
            if !out.is_empty() && n + 1 < total && self.failover.cooling(&id) {
                continue; // 冷却中且后面还有候选：跳过
            }
            out.push((id, provider, model));
        }
        out
    }

    /// 写入路径向量回填：本轮新条目批量嵌入（标题+正文），失败静默（词项召回仍可用）
    async fn backfill_embeddings(&self, ids: &[String]) {
        let Some(cfg) = self.semantic_recall() else { return };
        let (provider, model) = cfg;
        let mut pending: Vec<(String, String)> = Vec::new();
        for id in ids {
            if let Ok(Some(e)) = self.memory.get_entry_public(id) {
                if e.embedding.is_none() {
                    pending.push((id.clone(), format!("{}\n{}", e.title, e.body)));
                }
            }
        }
        if pending.is_empty() {
            return;
        }
        let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();
        match provider.embed(&model, &texts).await {
            Ok(vectors) if vectors.len() == pending.len() => {
                for ((id, _), v) in pending.iter().zip(vectors) {
                    let _ = self.memory.set_embedding(id, v);
                }
            }
            Ok(_) => eprintln!("[memory] 嵌入返回数量不符，跳过本轮回填"),
            Err(e) => eprintln!("[memory] 嵌入回填失败（词项召回不受影响）: {e}"),
        }
    }

    /// 查询嵌入（混合记忆检索）：语义检索指向的档案 + 模型，否则 None（纯词项）
    async fn embed_query(&self, text: &str) -> Option<Vec<f32>> {
        let (provider, model) = self.semantic_recall()?;
        provider.embed(&model, &[text.to_string()]).await.ok()?.into_iter().next()
    }

    /// 解析语义检索目标（组覆盖 → 全局记忆设置指定的档案与模型）；未配置返回 None
    fn semantic_recall(&self) -> Option<(Arc<dyn LlmProvider>, String)> {
        let target = self.capability_target("embedding")?;
        self.resolve_capability(&target).filter(|(_, m)| !m.is_empty())
    }

    /// 解析能力槽位目标（"档案ID" 或 "档案ID/模型名"）→ (Provider, 生效模型名)。
    /// 目标为空或档案不存在返回 None（调用方回落默认行为）。
    fn resolve_capability(&self, target: &str) -> Option<(Arc<dyn LlmProvider>, String)> {
        let id = target.trim().to_string();
        if id.is_empty() {
            return None;
        }
        let (pid, model) = match id.split_once('/') {
            Some((p, m)) if !m.trim().is_empty() => (p.trim().to_string(), m.trim().to_string()),
            _ => (id, String::new()),
        };
        let (provider, profile_model) = self.model_pool.entry(&pid)?;
        let model = if model.is_empty() { profile_model } else { model };
        Some((provider, model))
    }

    /// 指挥体候选链（目标提示 → 全局 → 回退链）
    fn orch_candidates(&self) -> Vec<(String, Arc<dyn LlmProvider>, String)> {
        let start = self
            .target_model_hint()
            .and_then(|h| self.model_pool.resolve_full(&h).map(|(id, _, _)| id))
            .unwrap_or_else(|| self.model_pool.active_id());
        if start.is_empty() {
            return Vec::new();
        }
        self.expand_chain(&start)
    }

    /// 子个体候选链（个体 → 组 → 全局 → 回退链）
    fn unit_candidates(&self, def: &AgentDefinition) -> Vec<(String, Arc<dyn LlmProvider>, String)> {
        let start = def
            .model_hint
            .clone()
            .or_else(|| self.registry.active_group_meta().and_then(|m| m.model))
            .and_then(|h| self.model_pool.resolve_full(&h).map(|(id, _, _)| id))
            .unwrap_or_else(|| self.model_pool.active_id());
        if start.is_empty() {
            return Vec::new();
        }
        self.expand_chain(&start)
    }

    /// 子个体回退链（run_node → AgentRuntime）
    fn unit_chain_for(&self, def: &AgentDefinition) -> ModelChain {
        ModelChain { candidates: self.unit_candidates(def), failover: self.failover.clone() }
    }

    // ---------------------------------------------------------- 事件

    pub fn emit(&self, session_id: &str, kind: &str, payload: serde_json::Value) {
        let evt = CoreEvent {
            kind: kind.to_string(),
            session_id: session_id.to_string(),
            payload,
        };
        let _ = self.events.send(evt.clone());
        // 事件溯源（可回放，docs/04 §8）
        let env = Envelope::new(
            format!("session.{session_id}.events"),
            self.orch_id(),
            "broadcast",
            "event",
            session_id,
            serde_json::to_value(&evt).unwrap_or(serde_json::Value::Null),
        );
        let _ = self.store.append_event(&env);
    }

    // ---------------------------------------------------------- 主入口

    /// collect：上一条非 user（orchestrator/unit/system）消息之后的所有 user 输入合并
    fn collect_pending_inputs(&self, session_id: &str) -> Option<String> {
        // list_messages 为时间正序（read_lines 语义）：从尾部反向收集连续 user 消息
        let msgs: Vec<crate::types::ChatMessage> = self.store.list_messages(session_id, 200).ok()?;
        let mut pending: Vec<String> = Vec::new();
        for m in msgs.iter().rev() {
            if m.role != MessageRole::User {
                break;
            }
            let mut stmts: Vec<String> = m.statements.iter().map(|s| s.text.clone()).collect();
            stmts.reverse();
            pending.extend(stmts);
        }
        if pending.is_empty() {
            return None;
        }
        pending.reverse(); // 时间正序
        Some(pending.join("\n"))
    }

    /// 上下文压缩（docs/架构与设计.md）：历史超过阈值时，旧消息压成滚动摘要，保留最近窗口原文
    /// 返回 (滚动摘要, 最近窗口消息原文文本)
    async fn ensure_compacted(&self, session_id: &str) -> (Option<String>, String) {
        type StoredMessage = crate::types::ChatMessage;
        const TRIGGER: usize = 20; // 超过才压缩
        const KEEP: usize = 12; // 保留最近 N 条原文
        let msgs: Vec<StoredMessage> = self.store.list_messages(session_id, 500).unwrap_or_default();
        if msgs.len() <= TRIGGER {
            return (None, render_recent(&msgs));
        }
        // list_messages 为时间正序：older = 前段（压入摘要）；recent = 尾部 KEEP 条原文窗口
        let split = msgs.len().saturating_sub(KEEP);
        let older: Vec<&StoredMessage> = msgs.iter().take(split).collect();
        let recent: Vec<&StoredMessage> = msgs.iter().skip(split).collect();
        let session = self.store.get_session(session_id).ok().flatten();
        let upto = older.len();
        let prev_upto = session.as_ref().and_then(|s| s.summary_upto).unwrap_or(0);
        let prev_summary = session.as_ref().and_then(|s| s.rolling_summary.clone()).unwrap_or_default();
        if prev_upto == upto {
            return (Some(prev_summary), render_recent_hlp(&recent));
        }
        // 增量压缩：已有摘要 + 新纳入的旧消息 → 新摘要（LLM 非流式，指挥体模型）
        let transcript: String = older
            .iter()
            .map(|m| {
                let who = match m.role {
                    MessageRole::User => "用户",
                    MessageRole::Orchestrator => m.agent_id.as_deref().unwrap_or("指挥体"),
                    MessageRole::Unit => m.agent_id.as_deref().unwrap_or("子个体"),
                    MessageRole::System => "系统",
                };
                let body = m.statements.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("；");
                format!("[{who}] {}", body.chars().take(400).collect::<String>())
            })
            .collect::<Vec<_>>()
            .join("\n");
        let system = "你是会话压缩器【会话压缩】。把历史对话压成不超过 400 字的连续上下文摘要：保留目标、关键结论、证据与未决事项；只输出摘要正文，不要任何前后缀。";
        let user = format!("已有摘要：\n{prev_summary}\n\n新增对话：\n{transcript}\n\n输出合并后的新摘要。");
        let summary = match self.resolve_orch_model() {
            (provider, model) => {
                let req = ChatRequest::new(model, vec![ChatMessage::system(system), ChatMessage::user(user)]);
                provider.chat(req).await.map(|r| r.content).unwrap_or_else(|e| {
                    eprintln!("[orchestrator] 会话压缩失败（沿用旧摘要）：{e}");
            			if prev_summary.is_empty() { String::new() } else { prev_summary.clone() }
                })
            }
        };
        if !summary.trim().is_empty() {
            let _ = self.store.update_compaction(session_id, &summary, upto);
            (Some(summary), render_recent_hlp(&recent))
        } else {
            (None, render_recent_hlp(&recent))
        }
    }

    pub async fn handle_user_message(
        self: &Arc<Self>,
        session_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let session = self.store.get_session(session_id)?;
        if session.is_none() {
            anyhow::bail!("会话不存在: {session_id}");
        }

        self.store
            .add_message(session_id, MessageRole::User, None, vec![Statement::new(SpeechTag::要求, text)])?;

        // collect 语义：把「上一条非 user 消息之后到达的全部用户输入」合并为本轮目标
        // （串行锁保证的排队窗口内的多条消息 = 一轮处理，避免逐条各跑一遍）
        let goal = self.collect_pending_inputs(session_id).unwrap_or_else(|| text.to_string());

        let ledger = self.store.mutate_ledger(session_id, |l| {
            l.task.goal = goal.clone();
        })?;
        self.emit(session_id, "ledger.updated", serde_json::to_value(&ledger)?);

        let result = self.run_round(session_id, &goal).await;
        if let Err(err) = &result {
            self.emit(session_id, "run.error", serde_json::json!({ "message": err.to_string() }));
            let _ = self.store.add_message(
                session_id,
                MessageRole::System,
                None,
                vec![Statement::warn(format!("本轮运行失败：{err}"))],
            );
        }
        result
    }

    async fn run_round(self: &Arc<Self>, session_id: &str, text: &str) -> anyhow::Result<()> {
        let plan = self.plan(session_id, text).await?;

        let ledger = self.store.mutate_ledger(session_id, |l| {
            l.task.acceptance = plan.acceptance.clone();
            l.task.constraints = plan.boundary.in_scope.clone();
            l.task.forbidden = plan.boundary.forbidden.clone();
        })?;
        self.emit(session_id, "ledger.updated", serde_json::to_value(&ledger)?);

        // L0 直达：不派发
        if matches!(plan.route_level, RouteLevel::L0) || plan.nodes.is_empty() {
            let statements = plan
                .final_answer
                .clone()
                .unwrap_or_else(|| vec![Statement::report("本机直接回答（L0）。")]);
            self.store.add_message(
                session_id,
                MessageRole::Orchestrator,
                Some(self.orch_id().as_str()),
                statements.clone(),
            )?;
            self.emit(
                session_id,
                "run.finished",
                serde_json::json!({ "routeLevel": "L0", "agentId": self.orch_id(), "statements": statements }),
            );
            return Ok(());
        }

        let mut graph = TaskGraphModel::from_plan(&plan, session_id);
        self.store.save_graph(&graph.to_graph())?;
        self.emit(session_id, "graph.updated", serde_json::to_value(graph.to_graph())?);

        let reports: Arc<Mutex<Reports>> = Arc::new(Mutex::new(Reports::new()));
        let exec = ExecCtx {
            orchestrator: self.clone(),
            session_id: session_id.to_string(),
            plan: plan.clone(),
            reports: reports.clone(),
        };
        let exec = Arc::new(exec);

        let scheduler = Scheduler::new(self.max_concurrency);
        let store = self.store.clone();
        let events = self.events.clone();
        let session_for_cb = session_id.to_string();
        let executor = {
            let exec = exec.clone();
            move |node: TaskNode| -> BoxFuture<'static, NodeOutcome> {
                let exec = exec.clone();
                Box::pin(async move { exec.run_node(node).await })
            }
        };

        scheduler
            .run(
                &mut graph,
                executor,
                move |g| {
                    let _ = store.save_graph(&g.to_graph());
                    let evt = CoreEvent {
                        kind: "graph.updated".into(),
                        session_id: session_for_cb.clone(),
                        payload: serde_json::to_value(g.to_graph()).unwrap_or(serde_json::Value::Null),
                    };
                    let _ = events.send(evt);
                },
            )
            .await;

        let final_reports = reports.lock().await.clone();
        let final_text = self.converge(session_id, &plan, &final_reports, &graph).await?;
        let statements = text_to_statements(&final_text);
        self.store.add_message(
            session_id,
            MessageRole::Orchestrator,
            Some(self.orch_id().as_str()),
            statements.clone(),
        )?;

        // 只更新阻断项，保留并发节点写入的证据/风险（原子读改写）
        let blockers: Vec<String> = final_reports
            .values()
            .flat_map(|r| r.blockers.iter().map(|b| b.reason.clone()))
            .collect();
        let ledger = self.store.mutate_ledger(session_id, |l| {
            l.risk.blockers = blockers;
        })?;
        self.emit(session_id, "ledger.updated", serde_json::to_value(&ledger)?);

        // ---- 自我进化：写入记忆（摘要/决策/证据/教训/个体统计） ----
        if self.memory_enabled {
            let ids = self.write_memories(session_id, text, &plan, &graph, &final_reports, &statements);
            // 语义向量后台回填（混合记忆检索；无嵌入模型时静默跳过）
            if !ids.is_empty() {
                let o = self.clone();
                tokio::spawn(async move { o.backfill_embeddings(&ids).await; });
            }
            // 经验优化自动触发：新教训达阈值时后台合成（docs/10 §5，只调优既有个体）
            if self.auto_adapt {
                let mut agent_ids: Vec<String> =
                    graph.list().iter().map(|n| n.agent_identifier.clone()).collect();
                agent_ids.sort();
                agent_ids.dedup();
                let o = self.clone();
                tokio::spawn(async move { o.auto_adapt(&agent_ids).await });
            }
        }

        self.emit(
            session_id,
            "run.finished",
            serde_json::json!({
                "routeLevel": format!("{:?}", plan.route_level),
                "agentId": self.orch_id(),
                "graph": graph.to_graph(),
                "statements": statements
            }),
        );
        Ok(())
    }

    /// 记忆写入策略（docs/08 §4）：摘要必有、决策必存、A/B 级证据必存、失败必记教训
    fn write_memories(
        &self,
        session_id: &str,
        user_text: &str,
        plan: &OrchestratorPlan,
        graph: &TaskGraphModel,
        reports: &Reports,
        statements: &[Statement],
    ) -> Vec<String> {
        let mut written: Vec<serde_json::Value> = Vec::new();
        let entry_ids: Vec<String> = Vec::new();
        // 组级记忆隔离：任务产物归属激活组（docs/09 §6）
        let gid = self.registry.active_group();

        // 1) 会话摘要（薄记）
        let digest = format!(
            "目标：{}\n路由：{:?}{}\n节点：{}\n收束要点：{}",
            plan.goal,
            plan.route_level,
            plan.playbook.as_ref().map(|p| format!("（链路 {p}）")).unwrap_or_default(),
            graph
                .list()
                .iter()
                .map(|n| format!("{}({}):{}", n.id, n.agent_identifier, n.status.label()))
                .collect::<Vec<_>>()
                .join("、"),
            statements
                .iter()
                .filter(|s| matches!(s.tag, SpeechTag::肯定 | SpeechTag::报告))
                .map(|s| s.text.clone())
                .take(3)
                .collect::<Vec<_>>()
                .join("；")
        );
        if let Ok(e) = self.memory.remember(
            &MemoryDraft::new(MemoryKind::Digest, format!("会话摘要：{}", plan.goal.chars().take(40).collect::<String>()), digest)
                .importance(0.45)
                .session(session_id)
                .tags(&["session", "digest"])
                .group(&gid),
        ) {
            written.push(serde_json::to_value(&e).unwrap_or(serde_json::Value::Null));
        }

        // 2) 决策（边界与禁区 = 本次任务的裁决口径）
        if let Ok(e) = self.memory.remember(
            &MemoryDraft::new(
                MemoryKind::Decision,
                format!("任务边界裁决：{}", plan.goal.chars().take(40).collect::<String>()),
                format!(
                    "范围内：{}\n禁区：{}\n验收：{}",
                    plan.boundary.in_scope.join("；"),
                    plan.boundary.forbidden.join("；"),
                    plan.acceptance.join("；")
                ),
            )
            .importance(0.7)
            .session(session_id)
            .tags(&["decision", "boundary"])
            .group(&gid),
        ) {
            written.push(serde_json::to_value(&e).unwrap_or(serde_json::Value::Null));
        }

        // 3) 证据（仅 A/B 级）与教训（失败/受阻），并按个体落统计
        for node in graph.list() {
            let Some(report) = reports.get(&node.id) else { continue };
            let status = match report.status {
                SyncStatus::Done => "done",
                SyncStatus::Blocked => "blocked",
                SyncStatus::NeedArbitration => "blocked",
                SyncStatus::InProgress => "blocked",
            };
            let _ = self.memory.record_agent_outcome(&node.agent_identifier, status, report.confidence);

            let strong: Vec<String> = report
                .evidence
                .iter()
                .filter(|e| matches!(e.level, EvidenceLevel::A | EvidenceLevel::B))
                .map(|e| format!("[{}|{}] {} — {}", e.level.label(), e.kind.label(), e.reference, e.note))
                .collect();
            if !strong.is_empty() {
                if let Ok(e) = self.memory.remember(
                    &MemoryDraft::new(
                        MemoryKind::Evidence,
                        format!("证据：{} / {}", node.agent_identifier, node.title),
                        strong.join("\n"),
                    )
                    .importance(0.6)
                    .session(session_id)
                    .agent(&node.agent_identifier)
                    .tags(&["evidence", &node.agent_identifier])
                    .group(&gid),
                ) {
                    written.push(serde_json::to_value(&e).unwrap_or(serde_json::Value::Null));
                }
            }

            if !matches!(report.status, SyncStatus::Done) || !report.blockers.is_empty() {
                let mut body = report.summary.clone();
                for b in &report.blockers {
                    body.push_str(&format!("\n阻塞：{}（解除条件：{}）", b.reason, b.unblock_condition));
                }
                if let Ok(e) = self.memory.remember(
                    &MemoryDraft::new(
                        MemoryKind::Lesson,
                        format!("教训：{} 在「{}」受阻", node.agent_identifier, node.title),
                        body,
                    )
                    .importance(0.75)
                    .session(session_id)
                    .agent(&node.agent_identifier)
                    .tags(&["lesson", "failure", &node.agent_identifier])
                    .group(&gid),
                ) {
                    written.push(serde_json::to_value(&e).unwrap_or(serde_json::Value::Null));
                }
            }
        }

        // 4) 用户偏好：输入含"记住/以后/每次"等指令词时固化
        let lower = user_text.to_lowercase();
        if ["记住", "以后", "每次", "默认", "偏好"].iter().any(|k| lower.contains(k)) {
            if let Ok(e) = self.memory.remember(
                &MemoryDraft::new(MemoryKind::Preference, "用户偏好指令", user_text.chars().take(400).collect::<String>())
                    .importance(0.9)
                    .session(session_id)
                    .tags(&["preference", "user"])
                    .group(&gid),
            ) {
                written.push(serde_json::to_value(&e).unwrap_or(serde_json::Value::Null));
            }
        }

        // 5) 重渲染基础记忆文件（memory.md）
        let pinned = self.memory.render_memory_md(&self.memory_md_path).unwrap_or(0);
        self.emit(
            session_id,
            "memory.written",
            serde_json::json!({ "entries": written, "pinned": pinned, "path": self.memory_md_path.display().to_string() }),
        );
        entry_ids
    }

    // ------------------------------------------------- 经验优化（docs/10 §5）

    /// 合成个体的经验改进要点：教训 + 可靠性统计 → LLM 提炼 → 净化 → 版本化落盘
    pub async fn optimize_agent(&self, identifier: &str) -> anyhow::Result<AgentAdaptation> {
        let Some(def) = self.registry.get(identifier) else {
            anyhow::bail!("个体不存在（激活组内）: {identifier}");
        };
        let gid = self.registry.active_group();
        let stats = self.memory.agent_stats(1000)?.into_iter().find(|s| s.agent_id == identifier);
        let lessons = self.memory.list_filtered(Some(MemoryKind::Lesson), Some(identifier), 20, Some(&gid))?;

        let stat_line = stats
            .as_ref()
            .map(|s| {
                format!(
                    "执行 {} 次，完成 {}，受阻 {}，失败 {}，平均置信度 {:.2}",
                    s.runs, s.done, s.blocked, s.failed, s.avg_confidence
                )
            })
            .unwrap_or_else(|| "暂无执行统计".into());
        let lessons_block = if lessons.is_empty() {
            "（暂无历史教训）".to_string()
        } else {
            lessons
                .iter()
                .map(|e| {
                    format!(
                        "- {}：{}",
                        e.title,
                        e.body.replace('\n', " ").chars().take(120).collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join("
")
        };
        let basis = stats.as_ref().map(|s| {
            serde_json::json!({
                "runs": s.runs, "done": s.done, "blocked": s.blocked, "failed": s.failed,
                "avgConfidence": format!("{:.2}", s.avg_confidence),
            })
        });

        let system = "你是 EXMACHINA 的个体优化器：基于子个体的历史教训与执行统计，提炼可复用的行为改进要点。
要求：最多 6 条；每条以 \"- \" 开头、不超过 100 字；只输出规则列表，不要输出其他内容；
规则必须是可执行的行为约束（提炼共性规律，不复述个案细节）；缺乏可靠依据时仅输出：- 暂无改进要点。【经验改进要点合成】".to_string();
        let user = format!(
            "个体：{}（{}）
职责：{}
统计：{}

历史教训：
{}",
            def.name, def.identifier, def.description, stat_line, lessons_block
        );
        let (provider, model) = self.resolve_orch_model();
        let req = ChatRequest::new(model, vec![ChatMessage::system(system), ChatMessage::user(user)]);
        let resp = provider.chat(req).await?;
        let content = sanitize_adaptation(&resp.content);
        anyhow::ensure!(!content.is_empty(), "未能提炼出有效的改进要点");

        let mut adapt = self
            .registry
            .load_adaptation(identifier)?
            .unwrap_or_else(|| AgentAdaptation {
                identifier: identifier.to_string(),
                content: String::new(),
                revision: 0,
                lessons_seen: 0,
                updated_at: String::new(),
                basis: None,
                previous: vec![],
            });
        adapt.content = content;
        adapt.revision += 1;
        adapt.lessons_seen = lessons.len() as u32;
        adapt.basis = basis.clone();
        let saved = self.registry.save_adaptation(adapt)?;
        self.emit(
            "",
            "agent.adapted",
            serde_json::json!({
                "agentId": identifier, "revision": saved.revision,
                "basis": basis, "lessonsSeen": saved.lessons_seen,
            }),
        );
        Ok(saved)
    }

    /// 自动触发：某个体新增教训达 3 条时后台合成
    async fn auto_adapt(self: &Arc<Self>, agent_ids: &[String]) {
        for id in agent_ids {
            let lessons = self
                .memory
                .list_filtered(Some(MemoryKind::Lesson), Some(id), 50, Some(&self.registry.active_group()))
                .unwrap_or_default()
                .len() as u32;
            let seen = self
                .registry
                .load_adaptation(id)
                .ok()
                .flatten()
                .map(|a| a.lessons_seen)
                .unwrap_or(0);
            if lessons.saturating_sub(seen) < 3 {
                continue;
            }
            if let Err(e) = self.optimize_agent(id).await {
                eprintln!("[orchestrator] 自动经验优化失败（{id}）：{e}");
            }
        }
    }

    // ---------------------------------------------------------- 分解

    async fn plan(self: &Arc<Self>, session_id: &str, text: &str) -> anyhow::Result<OrchestratorPlan> {
        // 组感知：规划提示统一取组内主智能体的职责提示词；内置组即全连结指挥体
        let primary = self.registry.primary().ok_or_else(|| {
            anyhow::anyhow!("当前组未设置主智能体（exm group info 查看；用 agent_manage 或 exm agent create 创建）")
        })?;
        let mut system_prompt = self.registry.load_prompt(&primary.prompt_file).unwrap_or_else(|_| {
            format!("# {}\n\n你是本组的主智能体，直接对接用户并调度组内个体。", primary.name)
        });
        if self.registry.single_mode() {
            // 单体模式：单体智能体直接完成，禁止派发
            system_prompt.push_str(SINGLE_CONTRACT);
        } else if !self.registry.active_group_meta().map(|m| m.builtin).unwrap_or(true) {
            // 自定义组：追加系统级规划契约（内置组提示词已内含）
            system_prompt.push_str(PLANNING_CONTRACT);
        }
        let registry_brief = self
            .registry
            .units()
            .iter()
            .map(|d| format!("- {}（{}）：{}", d.identifier, d.name, d.description))
            .collect::<Vec<_>>()
            .join("\n");

        // 技能包清单（数据驱动）：命中触发词的技能会随派发自动携带
        let skill_brief = match self.registry.load_skills() {
            Ok(sk) if !sk.is_empty() => {
                let lines = sk
                    .iter()
                    .map(|s| {
                        let agents = if s.agents.is_empty() { "全体".to_string() } else { s.agents.join("、") };
                        format!(
                            "- {}（{}）：触发 [{}]；适用 {}",
                            s.id,
                            s.name,
                            if s.triggers.is_empty() { "—".into() } else { s.triggers.join("、") },
                            agents
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("
");
                format!("## 可用技能包
{lines}
")
            }
            _ => String::new(),
        };

        // 链路模板清单（数据驱动）：与注册表一样注入规划提示，编成变化无需改代码
        let playbook_brief = match self.registry.load_playbooks() {
            Ok(pbs) if !pbs.is_empty() => {
                let lines = pbs
                    .iter()
                    .map(|p| {
                        format!(
                            "- {} {}：{}（触发：{}）",
                            p.id,
                            p.name,
                            p.description,
                            if p.trigger_signals.is_empty() { "—".to_string() } else { p.trigger_signals.join("、") }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("## 可用链路模板\n{lines}\n")
            }
            _ => String::new(),
        };

        // ---- 记忆召回（docs/08 §5）：注入历史记忆与个体可靠性统计 ----
        let mut memory_block = String::new();
        if self.memory_enabled {
            if let Ok(hits) = self.memory.recall(text, self.memory_recall_limit, None, Some(&self.registry.active_group()), self.embed_query(text).await.as_deref()) {
                memory_block = MemoryStore::render_prompt_block(&hits);
                self.emit(
                    session_id,
                    "memory.recall",
                    serde_json::json!({
                        "query": text,
                        "hits": hits.iter().map(|h| serde_json::json!({
                            "id": h.entry.id,
                            "kind": h.entry.kind.key(),
                            "title": h.entry.title,
                            "score": h.score,
                            "reasons": h.reasons,
                        })).collect::<Vec<_>>()
                    }),
                );
            }
        } else {
            // 深层记忆关闭：回落 memory.md 文件记忆（OpenClaw/Hermes 模式）
            if let Ok(md) = std::fs::read_to_string(&self.memory_md_path) {
                let trimmed: String = md.chars().take(8000).collect();
                if !trimmed.trim().is_empty() {
                    memory_block = format!("## Memory (memory.md)\n{trimmed}\n\n");
                }
            }
        }
        let stats_block = match self.memory.agent_stats(12) {
            Ok(stats) if !stats.is_empty() => {
                let lines = stats
                    .iter()
                    .map(|s| {
                        format!(
                            "- {}：执行 {} 次，完成 {}，受阻 {}，失败 {}，平均置信度 {:.2}",
                            s.agent_id, s.runs, s.done, s.blocked, s.failed, s.avg_confidence
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("## 个体可靠性统计（历史反馈，用于选路参考）\n{lines}\n")
            }
            _ => String::new(),
        };

        // 会话上下文（多轮记忆）：滚动摘要（压缩后的更早历史）+ 最近窗口原文
        let (summary, recent) = self.ensure_compacted(session_id).await;
        let context_block = match (&summary, recent.is_empty()) {
            (Some(s), false) if !s.trim().is_empty() => format!("## 会话上下文（更早对话摘要）\n{s}\n\n## 最近对话\n{recent}\n\n"),
            (Some(s), _) if !s.trim().is_empty() => format!("## 会话上下文（更早对话摘要）\n{s}\n\n"),
            (_, false) => format!("## 最近对话\n{recent}\n\n"),
            _ => String::new(),
        };

        let mut user_msg = ChatMessage::user(format!(
            "{context_block}用户任务输入：{text}\n\n【要求】按 OrchestratorPlan 契约输出 JSON。nodes 中的 agentIdentifier 必须来自上方可调度清单。"
        ));
        // 多模态输入：本轮附带的图片（data URL）。生效模型未标记视觉能力时，
        // 走「视觉转述」模型把图片转成文字描述注入；未配置转述且明确无视觉 → 丢弃并说明。
        let images = crate::image_stash::take(session_id);
        match self.relay_images(&images).await {
            Some(desc) => {
                let note = format!(
                    "\n\n## 图片内容（由视觉转述模型转写，原始图片未直接下发）\n{desc}"
                );
                user_msg.content.push_str(&note);
            }
            None => {
                let vision = self
                    .orch_candidates()
                    .first()
                    .and_then(|(pid, _, model)| self.model_pool.capability(pid, model, true));
                if vision == Some(false) && !images.is_empty() {
                    user_msg.content.push_str(
                        "\n\n（用户随本轮附带了图片，但当前模型不支持视觉且未配置视觉转述模型，图片已忽略。）",
                    );
                } else {
                    user_msg.images = images;
                }
            }
        }
        let mut messages = vec![
            ChatMessage::system(format!(
                "{system_prompt}\n\n## 当前可调度子个体\n{registry_brief}\n\n{skill_brief}{playbook_brief}{stats_block}{memory_block}"
            )),
            user_msg,
        ];

        let mut last_err = String::new();
        for _attempt in 0..3 {
            let output = self.call_orch_stream(session_id, &messages).await?;
            match parse::parse_plan(&output) {
                Ok(plan) => {
                    let unknown: Vec<String> = plan
                        .nodes
                        .iter()
                        .filter(|n| self.registry.get(&n.agent_identifier).is_none())
                        .map(|n| n.agent_identifier.clone())
                        .collect();
                    if self.registry.single_mode() && !plan.nodes.is_empty() {
                        last_err = "单体模式只支持 L0 直答：nodes 必须为空，回答放进 finalAnswer".into();
                    } else if unknown.is_empty() {
                        return Ok(plan);
                    } else {
                        last_err = format!("以下 agentIdentifier 不在注册表：{}", unknown.join("、"));
                    }
                }
                Err(e) => last_err = e,
            }
            messages.push(ChatMessage::assistant(output));
            messages.push(ChatMessage::user(format!(
                "【警告】计划未通过校验：{last_err}\n【要求】修正后仅输出 OrchestratorPlan JSON。"
            )));
        }
        anyhow::bail!("指挥体计划连续失败：{last_err}")
    }

    // ---------------------------------------------------------- 收束

    pub async fn converge(
        &self,
        session_id: &str,
        plan: &OrchestratorPlan,
        reports: &Reports,
        graph: &TaskGraphModel,
    ) -> anyhow::Result<String> {
        // 组感知：收束提示取组内主智能体的职责提示词
        let primary = self
            .registry
            .primary()
            .ok_or_else(|| anyhow::anyhow!("当前组未设置主智能体，无法收束"))?;
        let system_prompt = self
            .registry
            .load_prompt(&primary.prompt_file)
            .unwrap_or_else(|_| format!("# {}\n\n你是本组的主智能体。", primary.name));
        let digest = graph
            .list()
            .iter()
            .map(|n| {
                let summary = reports
                    .get(&n.id)
                    .map(|r| r.summary.clone())
                    .unwrap_or_else(|| "无回流".to_string());
                format!("- {} {}({}) [{}]: {}", n.id, n.title, n.agent_identifier, n.status.label(), summary)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!(
                "【收束请求】\n目标：{}\n验收：{}\n\n各节点回流摘要：\n{digest}\n\n【要求】按固定六段结构输出最终收束结果（任务边界/已启用链路/关键结论/冲突裁决/残余未知/最终交付），语言纪律：客观简洁，句式前缀陈述，零情绪。",
                plan.goal,
                plan.acceptance.join("；")
            )),
        ];
        self.call_orch_stream(session_id, &messages).await
    }

    async fn call_orch_stream(
        &self,
        session_id: &str,
        messages: &[ChatMessage],
    ) -> anyhow::Result<String> {
        // 回退链逐档尝试：失败且未发出任何 token 时切换下一档案并冷却失败者
        let candidates = self.orch_candidates();
        let mut last_err: Option<anyhow::Error> = None;
        for (pid, provider, model) in candidates {
            let mut req = ChatRequest::new(model, messages.to_vec());
            req.key_hint = Some("__orch__".into());
            let (tx, mut rx) = unbounded_channel::<String>();
            let req_clone = req.clone();
            let stream_provider = provider.clone();
            let handle = tokio::spawn(async move { stream_provider.stream(req_clone, tx).await });

            let mut acc = String::new();
            while let Some(delta) = rx.recv().await {
                acc.push_str(&delta);
                self.emit(session_id, "orchestrator.token", serde_json::json!({ "delta": delta }));
            }
            match handle.await {
                Ok(Ok(resp)) => {
                    self.failover.clear(&pid);
                    if acc.is_empty() {
                        let fallback = provider.chat(req).await?;
                        self.emit(
                            session_id,
                            "orchestrator.token",
                            serde_json::json!({ "delta": fallback.content }),
                        );
                        return Ok(fallback.content);
                    }
                    return Ok(if resp.content.is_empty() { acc } else { resp.content });
                }
                Ok(Err(e)) => {
                    if !acc.is_empty() || e.to_string().contains("未配置 API Key") {
                        return Err(e);
                    }
                    self.failover.cool(&pid, crate::provider::FAILOVER_COOLDOWN_SECS);
                    self.emit(
                        session_id,
                        "model.failover",
                        serde_json::json!({ "profile": pid, "error": e.to_string() }),
                    );
                    last_err = Some(e);
                }
                Err(e) => return Err(anyhow::anyhow!("流式任务失败: {e}")),
            }
        }
        // 候选链耗尽：用首候选做最后一次非流式尝试，仍失败才报错
        if let Some(last) = last_err {
            let (_, provider, model) = self
                .orch_candidates()
                .into_iter()
                .next()
                .unwrap_or_else(|| (String::new(), self.orch_provider.clone(), self.orch_model.clone()));
            let mut req = ChatRequest::new(model, messages.to_vec());
            req.key_hint = Some("__orch__".into());
            if let Ok(resp) = provider.chat(req).await {
                self.emit(session_id, "orchestrator.token", serde_json::json!({ "delta": resp.content }));
                return Ok(resp.content);
            }
            return Err(last);
        }
        // 无候选链（池空）：全局档案直答
        let mut req = ChatRequest::new(self.orch_model.clone(), messages.to_vec());
        req.key_hint = Some("__orch__".into());
        let resp = self.orch_provider.chat(req).await?;
        self.emit(session_id, "orchestrator.token", serde_json::json!({ "delta": resp.content }));
        Ok(resp.content)
    }
}

// ---------------------------------------------------------------- 节点执行上下文

pub struct ExecCtx {
    pub orchestrator: Arc<Orchestrator>,
    pub session_id: String,
    pub plan: OrchestratorPlan,
    pub reports: Arc<Mutex<Reports>>,
}

impl ExecCtx {
    pub async fn run_node(&self, node: TaskNode) -> NodeOutcome {
        let o = self.orchestrator.clone();
        // 上游回流快照（短暂持锁，随即释放，不跨 await）
        let snapshot: Arc<Reports> = Arc::new(self.reports.lock().await.clone());
        let o = o.as_ref();
        let Some(def) = o.registry.get(&node.agent_identifier) else {
            return NodeOutcome::failed(
                node.id.clone(),
                format!("注册表不存在个体: {}", node.agent_identifier),
            );
        };

        let order = DispatchOrder {
            task_node_id: node.id.clone(),
            objective: node.objective.clone(),
            acceptance: node.acceptance.clone(),
            boundary: self.plan.boundary.clone(),
            inputs: {
                let mut inputs = vec![DispatchInput {
                    ref_id: "user".into(),
                    kind: "userInput".into(),
                    summary: self.plan.goal.clone(),
                }];
                for dep in &node.depends_on {
                    if let Some(r) = snapshot.get(dep) {
                        inputs.push(DispatchInput {
                            ref_id: dep.clone(),
                            kind: "syncReport".into(),
                            summary: r.summary.clone(),
                        });
                    }
                }
                // 个体记忆 + 群体记忆：该个体的私有教训与共享决策/偏好（docs/08）
                if o.memory_enabled {
                    let query = format!("{} {} {}", def.name, def.identifier, node.title);
                    if let Ok(hits) = o.memory.recall_for_agent(&def.identifier, &query, 3, Some(&o.registry.active_group()), o.embed_query(&query).await.as_deref()) {
                        if !hits.is_empty() {
                            let summary = hits
                                .iter()
                                .map(|h| {
                                    let layer = if h.entry.agent_id.is_some() { "个体记忆" } else { "群体记忆" };
                                    format!(
                                        "[{layer}|{}] {}：{}",
                                        h.entry.kind.key(),
                                        h.entry.title,
                                        h.entry.body.replace('\n', " ").chars().take(160).collect::<String>()
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(" ｜ ");
                            inputs.push(DispatchInput {
                                ref_id: "memory".into(),
                                kind: "memory".into(),
                                summary,
                            });
                        }
                    }
                }
                // 技能包注入（docs/10）：任务目标命中触发词时，把技能指令随派发下发
                if let Ok(skills) = o.registry.load_skills() {
                    let haystack = format!("{} {} {}", self.plan.goal, node.title, node.objective);
                    for s in skills.iter().filter(|s| s.applies_to(&def.identifier) && s.matches(&haystack)) {
                        inputs.push(DispatchInput {
                            ref_id: format!("skill:{}", s.id),
                            kind: "skill".into(),
                            summary: s.instructions.clone(),
                        });
                    }
                }
                inputs
            },
            // 主智能体权限注入：自定义组的主智能体获得组内个体管理工具（docs/09）
            tool_allowlist: {
                let mut allowlist = def.tools.clone();
                let group_meta = o.registry.active_group_meta();
                if group_meta
                    .as_ref()
                    .map(|m| m.primary.as_deref() == Some(def.identifier.as_str()) && !m.builtin)
                    .unwrap_or(false)
                    && !allowlist.contains(&ToolName::AgentManage)
                {
                    allowlist.push(ToolName::AgentManage);
                }
                allowlist
            },
            constraints: DispatchConstraints { max_steps: 5, timeout_ms: 120_000 },
            report_format: "SyncReport".into(),
        };

        o.emit(
            &self.session_id,
            "dispatch.sent",
            serde_json::json!({
                "nodeId": node.id,
                "agentIdentifier": def.identifier,
                "adaptationRevision": o
                    .registry
                    .load_adaptation(&def.identifier)
                    .ok()
                    .flatten()
                    .map(|a| a.revision)
                    .unwrap_or(0),
                "order": serde_json::to_value(&order).unwrap_or(serde_json::Value::Null)
            }),
        );

        // 全连结：派发消息进总线（可观测；替换为分布式总线时业务零改动）
        let env = Envelope::new(
            format!("task.{}.dispatch", node.graph_id),
            o.orch_id(),
            def.identifier.clone(),
            "dispatch",
            self.session_id.clone(),
            serde_json::to_value(&order).unwrap_or(serde_json::Value::Null),
        );
        let _ = o.store.append_event(&env);

        let session_id = self.session_id.clone();
        let node_id = node.id.clone();
        let agent_label = def.identifier.clone();
        let events = o.events.clone();
        // 远程工作者优先（失败/超时回落本地）；令牌流统一进事件总线
        let result = {
            let orch = o;
            let sid = session_id.clone();
            let nid = node_id.clone();
            let label = agent_label.clone();
            let evts = events.clone();
            // 先判定远程可用性（注册表同步查询，避免闭包在分支间移动）
            let use_remote = orch.remote_enabled
                && orch.remote.as_ref().map(|r| r.accepts(&def)).unwrap_or(false);
            if use_remote {
                let remote = orch.remote.clone().unwrap();
                let def2 = def.clone();
                let order2 = order.clone();
                let sid2 = sid.clone();
                match remote
                    .execute(
                        &sid2,
                        &def2,
                        &order2,
                        &move |delta: String| {
                            let _ = evts.send(CoreEvent {
                                kind: "unit.token".into(),
                                session_id: sid.clone(),
                                payload: serde_json::json!({
                                    "agentId": label, "nodeId": nid, "delta": delta
                                }),
                            });
                        },
                    )
                    .await
                {
                    Ok(report) => Ok(report),
                    Err(e) => {
                        let _ = orch.events.send(CoreEvent {
                            kind: "remote.fallback".into(),
                            session_id: self.session_id.clone(),
                            payload: serde_json::json!({ "agentId": def.identifier, "reason": e.to_string() }),
                        });
                        let chain = orch.unit_chain_for(&def);
                        orch.unit_runtime
                            .execute(&def, &order, chain, |_delta| {})
                            .await
                    }
                }
            } else {
                let chain = orch.unit_chain_for(&def);
                orch.unit_runtime
                    .execute(&def, &order, chain, move |delta| {
                        let _ = evts.send(CoreEvent {
                            kind: "unit.token".into(),
                            session_id: sid.clone(),
                            payload: serde_json::json!({
                                "agentId": label, "nodeId": nid, "delta": delta
                            }),
                        });
                    })
                    .await
            }
        };

        match result {
            Ok(report) => {
                let report_id = match o.store.add_sync_report(&report) {
                    Ok(id) => id,
                    Err(e) => return NodeOutcome::failed(node.id.clone(), e.to_string()),
                };
                if let Err(e) = o.store.add_evidence(&self.session_id, &report.evidence, Some(&report_id)) {
                    eprintln!("[orchestrator] 证据写入失败: {e}");
                }
                let _ = o.store.add_message(
                    &self.session_id,
                    MessageRole::Unit,
                    Some(def.identifier.as_str()),
                    report.statements.clone(),
                );

                o.emit(
                    &self.session_id,
                    "sync.received",
                    serde_json::json!({
                        "nodeId": node.id,
                        "report": serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
                        "schemaValid": true
                    }),
                );

                // 三账：证据与风险（原子读改写，避免并发节点互相覆盖）
                let ledger = o
                    .store
                    .mutate_ledger(&self.session_id, |l| {
                        for e in &report.evidence {
                            l.evidence.confirmed.push(LedgerEvidenceItem {
                                text: format!("{} — {}", e.reference, e.note),
                                level: e.level,
                                source: def.identifier.clone(),
                            });
                        }
                        for r in &report.risks {
                            l.risk.impact.push(format!("{:?}: {}", r.severity, r.text));
                        }
                    })
                    .unwrap_or_default();
                o.emit(&self.session_id, "ledger.updated", serde_json::to_value(&ledger).unwrap_or_default());

                self.reports.lock().await.insert(node.id.clone(), report.clone());

                if matches!(report.status, SyncStatus::Blocked) {
                    return NodeOutcome::blocked(node.id.clone());
                }

                let mut appends: Vec<NewNode> = Vec::new();
                let need_arbitration = matches!(report.status, SyncStatus::NeedArbitration)
                    || report.conflicts.as_ref().map(|c| !c.is_empty()).unwrap_or(false);
                if need_arbitration {
                    let points = report
                        .conflicts
                        .as_ref()
                        .map(|cs| cs.iter().map(|c| c.point.clone()).collect::<Vec<_>>().join("；"))
                        .unwrap_or_default();
                    o.emit(
                        &self.session_id,
                        "arbitration.required",
                        serde_json::json!({ "nodeId": node.id, "conflicts": report.conflicts }),
                    );
                    // 裁决个体按数据声明动态选路（registry::arbiter），无命中回退主智能体
                    let arbiter_id = o
                        .registry
                        .arbiter()
                        .map(|d| d.identifier)
                        .unwrap_or_else(|| o.orch_id());
                    appends.push(NewNode {
                        agent_identifier: arbiter_id,
                        title: format!("裁决：{}", node.title),
                        objective: format!(
                            "基于以下冲突回流做出裁决：{points}。回流摘要：{}",
                            report.summary
                        ),
                        acceptance: vec![
                            "裁决结论带证据等级".into(),
                            "残余风险与回退路径明确".into(),
                        ],
                        priority: Priority::P0,
                    });
                }
                NodeOutcome::done(node.id.clone()).with_appends(appends)
            }
            Err(e) => NodeOutcome::failed(node.id.clone(), e.to_string()),
        }
    }
}

/// 提炼结果净化：只保留 "- " 列表行，最多 6 条、每条 ≤100 字、去重
fn sanitize_adaptation(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("• ")) else { continue };
        let rest = rest.trim();
        if rest.is_empty() || rest.contains("暂无") {
            continue;
        }
        let item = rest.chars().take(100).collect::<String>();
        if !lines.contains(&item) {
            lines.push(item);
        }
        if lines.len() >= 6 {
            break;
        }
    }
    lines.into_iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("
")
}

// ---------------------------------------------------------------- 收束文本 → 陈述序列

/// 最近窗口消息渲染为文本（新→旧输入，按时间正序输出）
fn render_recent(msgs: &[crate::types::ChatMessage]) -> String {
    render_recent_hlp(&msgs.iter().collect::<Vec<_>>())
}

fn render_recent_hlp(msgs: &[&crate::types::ChatMessage]) -> String {
    msgs.iter()
        .map(|m| {
            let who = match m.role {
                MessageRole::User => "用户",
                MessageRole::Orchestrator => m.agent_id.as_deref().unwrap_or("指挥体"),
                MessageRole::Unit => m.agent_id.as_deref().unwrap_or("子个体"),
                MessageRole::System => "系统",
            };
            let body = m.statements.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("；");
            format!("[{who}] {}", body.chars().take(300).collect::<String>())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn text_to_statements(text: &str) -> Vec<Statement> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let mut matched = false;
        for tag in [
            SpeechTag::肯定,
            SpeechTag::否定,
            SpeechTag::疑问,
            SpeechTag::报告,
            SpeechTag::提案,
            SpeechTag::警告,
            SpeechTag::要求,
            SpeechTag::观测,
        ] {
            let prefix = format!("【{}】", tag.as_str());
            if let Some(rest) = t.strip_prefix(&prefix) {
                out.push(Statement::new(tag, rest.trim()));
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        if let Some(rest) = t.strip_prefix("##") {
            out.push(Statement::report(rest.trim()));
        } else {
            out.push(Statement::report(t.trim_start_matches('-').trim()));
        }
    }
    if out.is_empty() {
        out.push(Statement::report("（收束输出为空）"));
    }
    out
}
