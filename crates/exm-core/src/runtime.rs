//! 子个体运行时 —— docs/01 §2.3
//! 装配 → LLM 循环（流式）→ 工具调用约定 → SyncReport 契约校验 → 回流

use crate::parse;
use crate::provider::{ChatMessage, ChatRequest, LlmProvider};
use crate::registry::LocalRegistry;
use crate::tools::ToolGateway;
use crate::types::{AgentDefinition, DispatchOrder, SyncReport};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

pub const SYNC_REWRITE_LIMIT: u32 = 2;

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
    /// `resolved` = 按个体/组默认模型解析出的 (Provider, 模型名)；None = 用全局生效档案。
    /// `on_token` 用于把流式增量推给渠道（WebUI/CLI）。
    pub async fn execute<F>(
        &self,
        def: &AgentDefinition,
        order: &DispatchOrder,
        resolved: Option<(Arc<dyn LlmProvider>, String)>,
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

        for _step in 0..max_steps {
            let text = self
                .call_llm(&messages, &agent_hint, &resolved, &mut on_token)
                .await?;

            // 1) 工具调用约定
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

        anyhow::bail!("个体 {} 超过最大步数 {max_steps} 仍未产出合法 SyncReport", def.identifier)
    }

    async fn call_llm<F>(
        &self,
        messages: &[ChatMessage],
        hint: &Option<String>,
        resolved: &Option<(Arc<dyn LlmProvider>, String)>,
        on_token: &mut F,
    ) -> anyhow::Result<String>
    where
        F: FnMut(&str),
    {
        let provider = resolved
            .as_ref()
            .map(|(p, _)| p.clone())
            .unwrap_or_else(|| self.provider.clone());
        let model = resolved
            .as_ref()
            .map(|(_, m)| m.clone())
            .unwrap_or_else(|| self.model.clone());
        let mut req = ChatRequest::new(model, messages.to_vec());
        req.key_hint = hint.clone();
        let (tx, mut rx) = unbounded_channel::<String>();
        let req_clone = req.clone();
        let stream_provider = provider.clone();

        // 注意：tx 必须被 move 进任务（不能保留原 sender），否则 rx 永不关闭 → 自锁
        let handle = tokio::spawn(async move { stream_provider.stream(req_clone, tx).await });

        while let Some(delta) = rx.recv().await {
            on_token(&delta);
        }
        let resp = handle.await??;
        if resp.content.trim().is_empty() {
            // 流式空响应退化非流式
            let fallback = provider.chat(req).await?;
            on_token(&fallback.content);
            return Ok(fallback.content);
        }
        Ok(resp.content)
    }
}
