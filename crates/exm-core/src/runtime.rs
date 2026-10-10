//! 子个体运行时 —— docs/01 §2.3
//! 装配 → LLM 循环（流式：正文 + 思维链双轨）→ 工具调用约定 → SyncReport 契约校验 → 回流

use crate::parse;
use crate::provider::{ChatMessage, ChatRequest, LlmProvider, StreamDelta, ToolCall};
use crate::registry::LocalRegistry;
use crate::tools::ToolGateway;
use crate::types::{AgentDefinition, DispatchOrder, SyncReport, ToolName};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

pub const SYNC_REWRITE_LIMIT: u32 = 2;

/// 工具结果 → 回填文本（【报告】成功 / 【警告】失败），语义与审计一致
fn tool_feedback(name: &str, r: &crate::tools::ToolResult) -> String {
    if r.ok {
        format!("【报告】工具 {name} 执行结果：\n{}", r.output)
    } else {
        format!("【警告】工具 {name} 执行失败：{}", r.error.as_deref().unwrap_or_default())
    }
}

/// 子个体执行器：持 Provider / 工具网关 / 注册表（注册表只读共享）
pub struct AgentRuntime {
    registry: Arc<LocalRegistry>,
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolGateway>,
    model: String,
}

impl AgentRuntime {
    pub fn new(
        registry: Arc<LocalRegistry>,
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolGateway>,
        model: impl Into<String>,
    ) -> Self {
        AgentRuntime { registry, provider, tools, model: model.into() }
    }

    /// 默认执行目标（全局生效档案）：未解析出个体/组专属模型时回退用
    pub fn default_target(&self) -> (Arc<dyn LlmProvider>, String) {
        (self.provider.clone(), self.model.clone())
    }

    /// 执行一个节点，产出 SyncReport。
    /// `chain` = 按个体/组默认模型展开的回退候选链（含全局档案兜底）。
    /// `on_delta` 用于把流式增量推给渠道（正文 / 思维链分轨）。
    /// `attach_images` = 目标模型具备视觉能力：工具副产物截图以图像消息回灌（computer use 闭环）。
    pub async fn execute<F>(
        &self,
        def: &AgentDefinition,
        order: &DispatchOrder,
        chain: crate::provider::ModelChain,
        session_id: &str,
        attach_images: bool,
        mut on_delta: F,
    ) -> anyhow::Result<SyncReport>
    where
        F: FnMut(StreamDelta),
    {
        // 多 Key 粘性键位按个体区分：每个子个体用自己的 Key（限额才在该键位上前进）
        let agent_hint = Some(def.identifier.clone());
        // 系统提示词 = 职责提示词（内置最优，已含统一智械纪律）。
        // 子个体无人格层（SOUL 仅限用户面智能体：组主智能体 / 单体）。
        let system_prompt = self.registry.load_prompt(&def.prompt_file)?;
        let order_json = serde_json::to_string_pretty(order)?;
        let mut messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!(
                "【要求】以下是本节点的调度指令（DispatchOrder）。严格在边界内执行，最终只输出 SyncReport JSON。\n```json\n{order_json}\n```"
            )),
        ];

        let max_steps = order.constraints.max_steps.max(1);
        // 派发时限（指挥体按步数分档声明，timeout_ms = 步数 × 30s）：0 = 不限（旧调用方兼容）。
        // 没有这道闸，挂死的 LLM 流会让节点无限期占住编排并发槽，整轮调度随之卡死——
        // max_steps 只约束「思考-调工具」轮数，约束不了单次 LLM 调用内部的无限等待。
        let deadline = (order.constraints.timeout_ms > 0)
            .then(|| std::time::Instant::now() + std::time::Duration::from_millis(order.constraints.timeout_ms));
        let mut rewrites: u32 = 0;
        let mut usage_prompt: u64 = 0;
        let mut usage_completion: u64 = 0;
        let mut last_model = String::new();
        // 原生 function calling：按白名单生成工具 schema + 该个体可见的 MCP 工具
        // （web_search / computer 仅在后端就绪时下发——不承诺不存在的能力；
        //   文件记忆模式下 memory_write/link 必然失败，同样不下发）
        let mut specs = crate::tools::ToolGateway::tool_specs_gated(
            &order.tool_allowlist,
            self.tools.search_ready(),
            self.tools.browser_ready(),
            self.tools.computer_ready(),
            self.tools.memory_deep(),
        );
        // 声明式自定义工具（按 agents 可见性）与 MCP 第三方工具一并下发
        specs.extend(self.tools.custom_specs_for(&def.identifier));
        specs.extend(self.tools.mcp().tool_snapshot_for(&def.identifier));

        for _step in 0..max_steps {
            // 停止检查点：会话被请求停止时终止本节点执行
            if crate::round_trace::is_cancelled(session_id) {
                anyhow::bail!("本轮已被用户停止");
            }
            // 时限检查点：派发预算耗尽即快速失败（把并发槽还给调度器），而非等 LLM 挂到天荒地老
            if let Some(dl) = deadline {
                if std::time::Instant::now() >= dl {
                    self.tools.record_usage(session_id, usage_prompt, usage_completion, &last_model);
                    anyhow::bail!(
                        "个体 {} 超过派发时限 {}ms（{} 步内未产出合法 SyncReport）",
                        def.identifier,
                        order.constraints.timeout_ms,
                        _step
                    );
                }
            }
            let (text, calls, usage) = match self
                .call_llm(&messages, &agent_hint, &chain, &specs, deadline, &mut on_delta)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    // LLM 调用失败（含超时）：已消耗的用量照记，失败原因回流调度器
                    self.tools.record_usage(session_id, usage_prompt, usage_completion, &last_model);
                    return Err(e);
                }
            };
            usage_prompt += usage.0;
            usage_completion += usage.1;
            if let Some((_, _, m)) = chain.candidates.first() {
                last_model = m.clone();
            }

            // 1a) 原生 function calling：协议级工具调用
            //     只读工具（read/grep/glob/web_fetch/web_search）并发执行；写工具按原顺序串行
            if !calls.is_empty() {
                let mut assistant = ChatMessage::assistant(text.clone());
                assistant.tool_calls = calls.clone();
                messages.push(assistant);
                let feedbacks = self
                    .run_tool_calls(
                        &def.identifier,
                        session_id,
                        &order.tool_allowlist,
                        &calls,
                        attach_images,
                    )
                    .await;
                for (c, (feedback, images)) in calls.iter().zip(feedbacks) {
                    let mut msg = ChatMessage::tool_result(&c.id, feedback);
                    msg.name = Some(c.name.clone());
                    messages.push(msg);
                    // 视觉回灌：截图以 user 消息附带（OpenAI 兼容端点不支持 tool 消息带图）
                    if let Some((note, data_urls)) = images {
                        let mut um = ChatMessage::user(note);
                        um.images = data_urls;
                        messages.push(um);
                    }
                }
                continue;
            }

            // 1b) 文本协议兜底（Mock / 不支持原生调用的端点）
            if let Some(raw) = parse::extract_json(&text) {
                if let Some((tool, args)) = parse::as_tool_call(&raw) {
                    let result = self
                        .tools
                        .execute(&def.identifier, session_id, &order.tool_allowlist, tool, &args)
                        .await;
                    messages.push(ChatMessage::assistant(text));
                    let feedback = if result.ok {
                        format!("【报告】工具 {} 执行结果：\n{}", tool.key(), result.output)
                    } else {
                        format!(
                            "【警告】工具 {} 执行失败：{}",
                            tool.key(),
                            result.error.unwrap_or_default()
                        )
                    };
                    messages.push(ChatMessage::user(feedback));
                    if attach_images {
                        if let Some((note, data_urls)) = self.image_note(&tool.key().to_string(), &result.images) {
                            let mut um = ChatMessage::user(note);
                            um.images = data_urls;
                            messages.push(um);
                        }
                    }
                    continue;
                }
            }

            // 2) 回流契约校验
            match parse::parse_sync_report(&text) {
                Ok(mut report) => {
                    // 归属强制校正
                    report.source_agent = def.identifier.clone();
                    report.task_node_id = order.task_node_id.clone();
                    self.tools.record_usage(session_id, usage_prompt, usage_completion, &last_model);
                    return Ok(report);
                }
                Err(err) => {
                    rewrites += 1;
                    if rewrites > SYNC_REWRITE_LIMIT {
                        anyhow::bail!(
                            "SyncReport 契约连续校验失败（{rewrites} 次）：{err}"
                        );
                    }
                    messages.push(ChatMessage::assistant(text));
                    messages.push(ChatMessage::user(format!(
                        "【警告】上一次输出未通过校验：{err}\n【要求】仅输出一个符合 SyncReport 契约的 JSON 代码块，不要输出其他内容。"
                    )));
                }
            }
        }

        self.tools.record_usage(session_id, usage_prompt, usage_completion, &last_model);
        anyhow::bail!("个体 {} 超过最大步数 {max_steps} 仍未产出合法 SyncReport", def.identifier)
    }

    /// 截图回灌材料：读文件 → data URL；不可读时返回 None
    fn image_note(&self, tool: &str, images: &[String]) -> Option<(String, Vec<String>)> {
        if images.is_empty() {
            return None;
        }
        let mut data_urls = Vec::new();
        for p in images {
            if let Some(url) = self.tools.read_image_data_url(p) {
                data_urls.push(url);
            }
        }
        if data_urls.is_empty() {
            return None;
        }
        Some((
            format!(
                "【附图】以下是工具 {tool} 的执行截图（已作为图像附带）。请直接观察画面内容并决定下一步。"
            ),
            data_urls,
        ))
    }

    /// 直接对话执行(常见单 agent 应用同款):系统提示 + 会话历史 + 工具循环,
    /// 无派发概念、无 SyncReport 契约——模型产出不含工具调用的文本即为终点。
    /// 供单体模式使用;工具面 = 个体 allowlist + 可见自定义工具 + MCP。
    pub async fn chat_execute<F>(
        &self,
        def: &AgentDefinition,
        system_prompt: String,
        history: Vec<ChatMessage>,
        session_id: &str,
        chain: crate::provider::ModelChain,
        attach_images: bool,
        mut on_delta: F,
    ) -> anyhow::Result<String>
    where
        F: FnMut(StreamDelta),
    {
        let agent_hint = Some(def.identifier.clone());
        let mut messages = vec![ChatMessage::system(system_prompt)];
        messages.extend(history);

        let max_steps = 24usize;
        let mut usage_prompt: u64 = 0;
        let mut usage_completion: u64 = 0;
        let mut last_model = String::new();

        // 原生 function calling：按白名单生成工具 schema + 该个体可见的 MCP 工具
        // （后端未就绪 / 模式不支持的工具不下发——不承诺不存在的能力）
        let mut specs = crate::tools::ToolGateway::tool_specs_gated(
            &def.tools,
            self.tools.search_ready(),
            self.tools.browser_ready(),
            self.tools.computer_ready(),
            self.tools.memory_deep(),
        );
        specs.extend(self.tools.custom_specs_for(&def.identifier));
        specs.extend(self.tools.mcp().tool_snapshot_for(&def.identifier));

        for _step in 0..max_steps {
            // 停止检查点:会话被请求停止时终止本循环
            if crate::round_trace::is_cancelled(session_id) {
                anyhow::bail!("本轮已被用户停止");
            }
            let (text, calls, usage) = self
                .call_llm(&messages, &agent_hint, &chain, &specs, None, &mut on_delta)
                .await?;
            usage_prompt += usage.0;
            usage_completion += usage.1;
            if let Some((_, _, m)) = chain.candidates.first() {
                last_model = m.clone();
            }

            // 原生 function calling:只读并发 / 写串行,结果回灌后继续
            if !calls.is_empty() {
                let mut assistant = ChatMessage::assistant(text.clone());
                assistant.tool_calls = calls.clone();
                messages.push(assistant);
                let feedbacks = self
                    .run_tool_calls(
                        &def.identifier,
                        session_id,
                        &def.tools,
                        &calls,
                        attach_images,
                    )
                    .await;
                for (c, (feedback, images)) in calls.iter().zip(feedbacks) {
                    let mut msg = ChatMessage::tool_result(&c.id, feedback);
                    msg.name = Some(c.name.clone());
                    messages.push(msg);
                    if let Some((note, data_urls)) = images {
                        let mut um = ChatMessage::user(note);
                        um.images = data_urls;
                        messages.push(um);
                    }
                }
                continue;
            }

            // 文本协议兜底(Mock / 不支持原生调用的端点)
            if let Some(raw) = parse::extract_json(&text) {
                if let Some((tool, args)) = parse::as_tool_call(&raw) {
                    let result = self
                        .tools
                        .execute(&def.identifier, session_id, &def.tools, tool, &args)
                        .await;
                    messages.push(ChatMessage::assistant(text));
                    let feedback = if result.ok {
                        format!("【报告】工具 {} 执行结果：\n{}", tool.key(), result.output)
                    } else {
                        format!(
                            "【警告】工具 {} 执行失败：{}",
                            tool.key(),
                            result.error.unwrap_or_default()
                        )
                    };
                    messages.push(ChatMessage::user(feedback));
                    if attach_images {
                        if let Some((note, data_urls)) =
                            self.image_note(&tool.key().to_string(), &result.images)
                        {
                            let mut um = ChatMessage::user(note);
                            um.images = data_urls;
                            messages.push(um);
                        }
                    }
                    continue;
                }
            }

            // 无工具调用 = 最终回复
            self.tools
                .record_usage(session_id, usage_prompt, usage_completion, &last_model);
            return Ok(text);
        }

        self.tools
            .record_usage(session_id, usage_prompt, usage_completion, &last_model);
        anyhow::bail!(
            "个体 {} 超过最大步数 {max_steps} 仍未产出最终回复",
            def.identifier
        )
    }

    /// 同轮工具调用：只读工具并发（join_all），写工具与 MCP 按原顺序串行。
    /// 返回与 `calls` 同序的（回填文本, 截图回灌材料）——保证与 tool_call_id 配对。
    async fn run_tool_calls(
        &self,
        agent_id: &str,
        session_id: &str,
        allowlist: &[ToolName],
        calls: &[ToolCall],
        attach_images: bool,
    ) -> Vec<(String, Option<(String, Vec<String>)>)> {
        let mut out: Vec<Option<(String, Option<(String, Vec<String>)>)>> = vec![None; calls.len()];
        let mut readonly: Vec<usize> = Vec::new();
        let mut serial: Vec<usize> = Vec::new();
        for (i, c) in calls.iter().enumerate() {
            let ro = !c.name.starts_with("mcp:")
                && ToolName::parse(&c.name).map(|t| t.is_readonly()).unwrap_or(false);
            if ro {
                readonly.push(i);
            } else {
                serial.push(i);
            }
        }
        // 只读并发
        if !readonly.is_empty() {
            let futs = readonly.iter().map(|i| {
                let c = &calls[*i];
                async move {
                    let tool = ToolName::parse(&c.name).unwrap_or(ToolName::Read);
                    let r = self
                        .tools
                        .execute(agent_id, session_id, allowlist, tool, &c.arguments)
                        .await;
                    (tool_feedback(&c.name, &r), None)
                }
            });
            let results = futures_util::future::join_all(futs).await;
            for (i, pair) in readonly.iter().zip(results) {
                out[*i] = Some(pair);
            }
        }
        // 写 / MCP / 自定义 / 未知：串行
        for i in serial {
            let c = &calls[i];
            let (r, name) = if c.name.starts_with("mcp:") {
                (self.tools.execute_mcp(agent_id, session_id, &c.name, &c.arguments).await, c.name.clone())
            } else {
                // 统一入口：内置按白名单放行，声明式自定义按 agents 可见性放行
                (
                    self.tools
                        .execute_named(agent_id, session_id, allowlist, &c.name, &c.arguments)
                        .await,
                    c.name.clone(),
                )
            };
            let images = if attach_images {
                self.image_note(&name, &r.images)
            } else {
                None
            };
            out[i] = Some((tool_feedback(&name, &r), images));
        }
        out.into_iter().map(|x| x.unwrap_or_default()).collect()
    }

    /// `deadline` = 派发时限（Some = execute 路径按 DispatchOrder 预算收口；None = 不限时）。
    /// 挂死的 SSE 连接在剩余预算内等不到收尾时：abort 流任务并报可诊断错误，节点不被无限拖住。
    async fn call_llm<F>(
        &self,
        messages: &[ChatMessage],
        hint: &Option<String>,
        chain: &crate::provider::ModelChain,
        tools: &[crate::provider::ToolSpec],
        deadline: Option<std::time::Instant>,
        on_delta: &mut F,
    ) -> anyhow::Result<(String, Vec<ToolCall>, (u64, u64))>
    where
        F: FnMut(StreamDelta),
    {
        // 候选链为空 → 运行时默认（全局生效档案）
        let mut candidates: Vec<(String, Arc<dyn LlmProvider>, String)> = chain.candidates.clone();
        if candidates.is_empty() {
            let (p, m) = self.default_target();
            candidates.push((String::new(), p, m));
        }
        let mut last_err: Option<anyhow::Error> = None;
        for (pid, provider, model) in candidates {
            let mut req = ChatRequest::new(model.clone(), messages.to_vec());
            req.key_hint = hint.clone();
            if !tools.is_empty() {
                req.tools = tools.to_vec();
            }
            let (tx, mut rx) = unbounded_channel::<StreamDelta>();
            let req_clone = req.clone();
            let stream_provider = provider.clone();

            // 注意：tx 必须被 move 进任务（不能保留原 sender），否则 rx 永不关闭 → 自锁
            let handle = tokio::spawn(async move { stream_provider.stream(req_clone, tx).await });

            let mut emitted = false;
            // 有界收增量：到点（deadline）或流自然关闭即出循环；超时路径 abort 流任务，
            // 连接随任务取消而释放（不残留读端拖住运行时收尾）
            let drain = async {
                while let Some(delta) = rx.recv().await {
                    emitted = true;
                    on_delta(delta);
                }
            };
            match deadline {
                Some(dl) => {
                    if tokio::time::timeout_at(tokio::time::Instant::from_std(dl), drain)
                        .await
                        .is_err()
                    {
                        handle.abort();
                        return Err(anyhow::anyhow!(
                            "LLM 调用超时（候选 {pid}/{model}）：时限内未完成流式响应，已中止"
                        ));
                    }
                }
                None => drain.await,
            }
            match handle.await {
                Ok(Ok(resp)) => {
                    if !pid.is_empty() {
                        chain.failover.clear(&pid);
                    }
                    if resp.content.trim().is_empty() && resp.tool_calls.is_empty() {
                        // 流式空响应退化非流式
                        let fallback = provider.chat(req).await?;
                        if !fallback.reasoning.is_empty() {
                            on_delta(StreamDelta::Thinking(fallback.reasoning.clone()));
                        }
                        on_delta(StreamDelta::Text(fallback.content.clone()));
                        let usage = (fallback.prompt_tokens, fallback.completion_tokens);
                        return Ok((fallback.content, fallback.tool_calls, usage));
                    }
                    let usage = (resp.prompt_tokens, resp.completion_tokens);
                    return Ok((resp.content, resp.tool_calls, usage));
                }
                Ok(Err(e)) => {
                    // 已发出 token 的流不再重试（避免重复输出）；Mock 守卫错误直抛
                    if emitted || e.to_string().contains("未配置 API Key") {
                        return Err(e);
                    }
                    if !pid.is_empty() {
                        chain.failover.cool(&pid, crate::provider::FAILOVER_COOLDOWN_SECS);
                    }
                    last_err = Some(e);
                }
                Err(e) => return Err(anyhow::anyhow!("流式任务失败: {e}")),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("无可用模型候选")))
    }
}

// ---------------------------------------------------------------- 派发时限测试

#[cfg(test)]
mod timeout_tests {
    use super::*;
    use crate::config::SecurityConfig;
    use crate::provider::{ChatResponse, FailoverState};
    use crate::types::{DispatchBoundary, DispatchConstraints, DispatchInput, DispatchOrder};
    use std::time::Duration;

    /// 挂死流替身：永不产出增量、长眠不醒（模拟端点挂死 / 网络黑洞）
    struct HangingStreamProvider;

    #[async_trait::async_trait]
    impl LlmProvider for HangingStreamProvider {
        fn name(&self) -> &'static str {
            "hanging"
        }
        async fn chat(&self, _req: ChatRequest) -> anyhow::Result<ChatResponse> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(ChatResponse::default())
        }
        async fn stream(
            &self,
            _req: ChatRequest,
            _tx: tokio::sync::mpsc::UnboundedSender<StreamDelta>,
        ) -> anyhow::Result<ChatResponse> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(ChatResponse::default())
        }
    }

    fn runtime_with(provider: Arc<dyn LlmProvider>) -> AgentRuntime {
        let base = std::env::temp_dir().join(format!("exm-rt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let reg_dir = base.join("entities");
        std::fs::create_dir_all(reg_dir.join("prompts")).unwrap();
        let meta = crate::types::GroupMeta {
            id: "exmachina".into(),
            name: "测试组".into(),
            description: String::new(),
            primary: None,
            workspace: None,
            model: None,
            capabilities: None,
            builtin: true,
            created_at: crate::types::now_iso(),
        };
        std::fs::write(
            reg_dir.join("group.json"),
            serde_json::to_string_pretty(&meta).unwrap(),
        )
        .unwrap();
        // 提示词按组布局解析：groups/<gid>/prompts/<file>.md
        let prompts_dir = reg_dir.join("groups").join("exmachina").join("prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        std::fs::write(prompts_dir.join("timeout-agent.md"), "# 测试个体\n你是测试个体。").unwrap();
        let registry = Arc::new(LocalRegistry::new(&reg_dir).expect("临时注册表创建失败"));
        let store = Arc::new(crate::store::Store::open(base.join("data")).expect("临时存储创建失败"));
        let (events, _rx) = tokio::sync::broadcast::channel::<crate::types::CoreEvent>(16);
        let tools = Arc::new(ToolGateway::new(
            &base,
            store,
            registry.clone(),
            events,
            SecurityConfig::default(),
        ));
        AgentRuntime::new(registry, provider, tools, "test-model")
    }

    fn order_with(timeout_ms: u64) -> DispatchOrder {
        DispatchOrder {
            task_node_id: "T1".into(),
            objective: "时限演练".into(),
            acceptance: vec!["产出 SyncReport".into()],
            boundary: DispatchBoundary { in_scope: vec![], forbidden: vec![] },
            inputs: vec![DispatchInput {
                ref_id: "user".into(),
                kind: "userInput".into(),
                summary: "时限演练".into(),
            }],
            tool_allowlist: vec![],
            constraints: DispatchConstraints { max_steps: 3, timeout_ms },
            report_format: "SyncReport".into(),
        }
    }

    fn def_with() -> AgentDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "时限演练体", "identifier": "timeout-agent", "domain": "公共",
            "tier": "unit", "description": "派发时限演练", "capabilities": ["测试"],
            "promptFile": "timeout-agent.md"
        }))
        .unwrap()
    }

    /// 挂死的 LLM 流必须在派发时限内被掐断并报可诊断错误（回归：timeout_ms 声明无人执行，
    /// 节点无限期占住编排并发槽）
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 派发时限_挂死流被掐断并报错() {
        let rt = runtime_with(Arc::new(HangingStreamProvider));
        let def = def_with();
        let order = order_with(400); // 400ms 时限
        let chain = crate::provider::ModelChain {
            candidates: vec![(String::new(), Arc::new(HangingStreamProvider), "m".into())],
            failover: Arc::new(FailoverState::default()),
        };
        let started = std::time::Instant::now();
        let r = rt.execute(&def, &order, chain, "ses-timeout", false, |_| {}).await;
        let elapsed = started.elapsed();
        let err = r.expect_err("挂死流应在时限内失败");
        assert!(err.to_string().contains("超时"), "错误应可诊断为超时: {err}");
        assert!(
            elapsed >= Duration::from_millis(400) && elapsed < Duration::from_secs(10),
            "应约在时限处失败而非挂满 60s，实际 {elapsed:?}"
        );
    }

    /// 快速正常的 LLM 不应被时限误伤（时限内正常产出 SyncReport → 成功回流）
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 派发时限_正常流不受影响() {
        let rt = runtime_with(Arc::new(crate::provider::MockLlmProvider));
        let def = def_with();
        let order = order_with(10_000);
        let chain = crate::provider::ModelChain {
            candidates: vec![(String::new(), Arc::new(crate::provider::MockLlmProvider), "m".into())],
            failover: Arc::new(FailoverState::default()),
        };
        let report = rt
            .execute(&def, &order, chain, "ses-ok", false, |_| {})
            .await
            .expect("时限充裕时应正常执行");
        assert_eq!(report.task_node_id, "T1");
        assert_eq!(report.status, crate::types::SyncStatus::Done);
    }
}
