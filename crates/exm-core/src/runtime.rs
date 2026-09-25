//! 子个体运行时 —— docs/01 §2.3
//! 装配 → LLM 循环（流式）→ 工具调用约定 → SyncReport 契约校验 → 回流

use crate::parse;
use crate::provider::{ChatMessage, ChatRequest, LlmProvider};
use crate::registry::LocalRegistry;
use crate::tools::ToolGateway;
use crate::types::{AgentDefinition, DispatchOrder, SyncReport, ToolName};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

pub const SYNC_REWRITE_LIMIT: u32 = 2;

/// 工具结果 → 回填文本（【报告】成功 / 【警告】失败），语义与审计一致
fn tool_feedback(name: &str, r: crate::tools::ToolResult) -> String {
    if r.ok {
        format!("【报告】工具 {name} 执行结果：\n{}", r.output)
    } else {
        format!("【警告】工具 {name} 执行失败：{}", r.error.unwrap_or_default())
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
    /// `on_token` 用于把流式增量推给渠道（WebUI/CLI）。
    pub async fn execute<F>(
        &self,
        def: &AgentDefinition,
        order: &DispatchOrder,
        chain: crate::provider::ModelChain,
        session_id: &str,
        mut on_token: F,
    ) -> anyhow::Result<SyncReport>
    where
        F: FnMut(&str),
    {
        // 多 Key 粘性键位按个体区分：每个子个体用自己的 Key（限额才在该键位上前进）
        let agent_hint = Some(def.identifier.clone());
        // 系统提示 = 职责提示词 + 说话风格（人设，热读取：修改后下一次派发即生效）
        let base_prompt = self.registry.load_prompt(&def.prompt_file)?;
        let persona = self.registry.persona(&def.identifier).unwrap_or_else(|e| {
            eprintln!("[runtime] 读取人设失败（{}）：{e}，使用默认", def.identifier);
            LocalRegistry::DEFAULT_PERSONA.to_string()
        });
        let system_prompt = format!("{base_prompt}\n\n## 说话风格（人设）\n{persona}");
        let order_json = serde_json::to_string_pretty(order)?;
        let mut messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(format!(
                "【要求】以下是本节点的调度指令（DispatchOrder）。严格在边界内执行，最终只输出 SyncReport JSON。\n```json\n{order_json}\n```"
            )),
        ];

        let max_steps = order.constraints.max_steps.max(1);
        let mut rewrites: u32 = 0;
        let mut usage_prompt: u64 = 0;
        let mut usage_completion: u64 = 0;
        let mut last_model = String::new();
        // 原生 function calling：按白名单生成工具 schema + 该个体可见的 MCP 工具
        // （web_search 仅在搜索后端已配置时下发——不承诺不存在的能力）
        let mut specs = crate::tools::ToolGateway::tool_specs(
            &order.tool_allowlist,
            self.tools.search_ready(),
            self.tools.browser_ready(),
        );
        // 声明式自定义工具（按 agents 可见性）与 MCP 第三方工具一并下发
        specs.extend(self.tools.custom_specs_for(&def.identifier));
        specs.extend(self.tools.mcp().tool_snapshot_for(&def.identifier));

        for _step in 0..max_steps {
            let (text, calls, usage) = self
                .call_llm(&messages, &agent_hint, &chain, &specs, &mut on_token)
                .await?;
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
                    .run_tool_calls(&def.identifier, &order.tool_allowlist, &calls)
                    .await;
                for (c, feedback) in calls.iter().zip(feedbacks) {
                    let mut msg = ChatMessage::tool_result(&c.id, feedback);
                    msg.name = Some(c.name.clone());
                    messages.push(msg);
                }
                continue;
            }

            // 1b) 文本协议兜底（Mock / 不支持原生调用的端点）
            if let Some(raw) = parse::extract_json(&text) {
                if let Some((tool, args)) = parse::as_tool_call(&raw) {
                    let result = self
                        .tools
                        .execute(&def.identifier, &order.tool_allowlist, tool, &args)
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

    /// 同轮工具调用：只读工具并发（join_all），写工具与 MCP 按原顺序串行。
    /// 返回与 `calls` 同序的回填文本（【报告】/【警告】），保证与 tool_call_id 配对。
    async fn run_tool_calls(
        &self,
        agent_id: &str,
        allowlist: &[ToolName],
        calls: &[crate::provider::ToolCall],
    ) -> Vec<String> {
        let mut out: Vec<Option<String>> = vec![None; calls.len()];
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
                    let r = self.tools.execute(agent_id, allowlist, tool, &c.arguments).await;
                    tool_feedback(&c.name, r)
                }
            });
            let results = futures_util::future::join_all(futs).await;
            for (i, text) in readonly.iter().zip(results) {
                out[*i] = Some(text);
            }
        }
        // 写 / MCP / 自定义 / 未知：串行
        for i in serial {
            let c = &calls[i];
            let text = if c.name.starts_with("mcp:") {
                let r = self.tools.execute_mcp(agent_id, &c.name, &c.arguments).await;
                tool_feedback(&c.name, r)
            } else {
                // 统一入口：内置按白名单放行，声明式自定义按 agents 可见性放行
                let r = self
                    .tools
                    .execute_named(agent_id, allowlist, &c.name, &c.arguments)
                    .await;
                tool_feedback(&c.name, r)
            };
            out[i] = Some(text);
        }
        out.into_iter().map(|x| x.unwrap_or_default()).collect()
    }

    async fn call_llm<F>(
        &self,
        messages: &[ChatMessage],
        hint: &Option<String>,
        chain: &crate::provider::ModelChain,
        tools: &[crate::provider::ToolSpec],
        on_token: &mut F,
    ) -> anyhow::Result<(String, Vec<crate::provider::ToolCall>, (u64, u64))>
    where
        F: FnMut(&str),
    {
        // 候选链为空 → 运行时默认（全局生效档案）
        let mut candidates: Vec<(String, Arc<dyn LlmProvider>, String)> = chain.candidates.clone();
        if candidates.is_empty() {
            let (p, m) = self.default_target();
            candidates.push((String::new(), p, m));
        }
        let mut last_err: Option<anyhow::Error> = None;
        for (pid, provider, model) in candidates {
            let mut req = ChatRequest::new(model, messages.to_vec());
            req.key_hint = hint.clone();
            if !tools.is_empty() {
                req.tools = tools.to_vec();
            }
            let (tx, mut rx) = unbounded_channel::<String>();
            let req_clone = req.clone();
            let stream_provider = provider.clone();

            // 注意：tx 必须被 move 进任务（不能保留原 sender），否则 rx 永不关闭 → 自锁
            let handle = tokio::spawn(async move { stream_provider.stream(req_clone, tx).await });

            let mut emitted = false;
            while let Some(delta) = rx.recv().await {
                emitted = true;
                on_token(&delta);
            }
            match handle.await {
                Ok(Ok(resp)) => {
                    if !pid.is_empty() {
                        chain.failover.clear(&pid);
                    }
                    if resp.content.trim().is_empty() && resp.tool_calls.is_empty() {
                        // 流式空响应退化非流式
                        let fallback = provider.chat(req).await?;
                        on_token(&fallback.content);
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
