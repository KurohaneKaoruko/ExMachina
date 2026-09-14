//! LLM Provider 抽象 —— docs/04 §5
//! 一套代码接所有 OpenAI 兼容端点；无密钥时使用确定性 Mock 通道。

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

/// 原生 function calling：暴露给模型的工具定义（parameters = JSON Schema 对象）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 模型发起的一次工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// assistant 消息携带的工具调用（回传线程用）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// tool 结果消息：对应的调用 id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// tool 结果消息：工具名（gemini functionResponse 需要）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 图片附件（多模态输入）：data URL（data:image/png;base64,…）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage { role: "system".into(), content: content.into(), tool_calls: Vec::new(), tool_call_id: None, name: None, images: Vec::new() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: "user".into(), content: content.into(), tool_calls: Vec::new(), tool_call_id: None, name: None, images: Vec::new() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage { role: "assistant".into(), content: content.into(), tool_calls: Vec::new(), tool_call_id: None, name: None, images: Vec::new() }
    }
    /// 工具结果消息（openai 形态 role=tool；anthropic/gemini 在请求映射时转换）
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage { role: "tool".into(), content: content.into(), tool_calls: Vec::new(), tool_call_id: Some(call_id.into()), name: None, images: Vec::new() }
    }

    /// data URL 拆解：(mime, base64)；非 data URL 返回 None
    fn split_data_url(url: &str) -> Option<(String, String)> {
        let rest = url.strip_prefix("data:")?;
        let (meta, data) = rest.split_once(',')?;
        let mime = meta.split(';').next()?.to_string();
        Some((mime, data.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    /// 发起方标识（个体/指挥体）：多 Key 时按此粘性选键——同发起方不换 Key，
    /// 仅当前 Key 限额（429/402/配额类错误）时前进到下一把并继续粘住。
    pub key_hint: Option<String>,
    /// 原生 function calling：非空即随请求下发
    pub tools: Vec<ToolSpec>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        ChatRequest { model: model.into(), messages, temperature: 0.2, max_tokens: None, key_hint: None, tools: Vec::new() }
    }

    pub fn with_key_hint(mut self, hint: impl Into<String>) -> Self {
        self.key_hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub content: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 模型发起的工具调用（原生 function calling）
    pub tool_calls: Vec<ToolCall>,
}

// ---------------------------------------------------------------- 模型档案运行池

/// 模型档案运行池：档案ID → (Provider, 指挥体模型, 子个体模型)。
/// 供按个体/组解析默认模型（提示格式："档案ID" 或 "档案ID/模型名"）；
/// 档案编辑经 apply_config 重建池，组/个体的提示则热读取。
pub struct ModelPool {
    entries: std::collections::HashMap<String, (std::sync::Arc<dyn LlmProvider>, String, String)>,
    /// 档案失败回退链：id → 下一个档案 id
    fallbacks: std::collections::HashMap<String, String>,
    /// 全局生效档案 id（回退链的最终兜底）
    active_id: String,
    /// 档案嵌入模型（混合记忆检索）
    embed_models: std::collections::HashMap<String, String>,
}

impl Default for ModelPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelPool {
    pub fn new() -> Self {
        ModelPool {
            entries: std::collections::HashMap::new(),
            fallbacks: std::collections::HashMap::new(),
            active_id: String::new(),
            embed_models: std::collections::HashMap::new(),
        }
    }

    /// 声明档案嵌入模型
    pub fn set_embed_model(&mut self, id: &str, model: &str) {
        if model.trim().is_empty() {
            self.embed_models.remove(id);
        } else {
            self.embed_models.insert(id.to_string(), model.trim().to_string());
        }
    }

    /// 档案嵌入模型（未声明返回 None）
    pub fn profile_embed_model(&self, id: &str) -> Option<String> {
        self.embed_models.get(id).cloned()
    }

    /// 全局生效档案 id（build_orchestrator 注入）
    pub fn set_active(&mut self, id: impl Into<String>) {
        self.active_id = id.into();
    }

    pub fn active_id(&self) -> String {
        self.active_id.clone()
    }

    /// 声明档案失败回退（指向下一档案 id；空串清除）
    pub fn set_fallback(&mut self, id: &str, next: &str) {
        if next.trim().is_empty() {
            self.fallbacks.remove(id);
        } else {
            self.fallbacks.insert(id.to_string(), next.trim().to_string());
        }
    }

    /// 从起始档案展开回退链（去重 + 成环截断，最长 16 跳）
    pub fn chain(&self, start: &str) -> Vec<String> {
        let mut out = vec![start.to_string()];
        let mut cur = start.to_string();
        for _ in 0..16 {
            let Some(next) = self.fallbacks.get(&cur).cloned() else { break };
            if next == start || out.contains(&next) || next.is_empty() {
                break;
            }
            out.push(next.clone());
            cur = next;
        }
        out
    }

    /// 取档案的 (Provider, 模型名)；unit=true 取子个体模型
    pub fn entry(&self, id: &str, unit: bool) -> Option<(std::sync::Arc<dyn LlmProvider>, String)> {
        let (provider, orch, unit_model) = self.entries.get(id)?;
        Some((provider.clone(), if unit { unit_model.clone() } else { orch.clone() }))
    }

    pub fn insert(
        &mut self,
        id: impl Into<String>,
        provider: std::sync::Arc<dyn LlmProvider>,
        orch_model: impl Into<String>,
        unit_model: impl Into<String>,
    ) {
        self.entries.insert(id.into(), (provider, orch_model.into(), unit_model.into()));
    }

    /// 解析模型提示：返回 (Provider, 生效模型名)；档案不存在返回 None（调用方回退全局）
    pub fn resolve(
        &self,
        hint: &str,
        unit: bool,
    ) -> Option<(std::sync::Arc<dyn LlmProvider>, String)> {
        self.resolve_full(hint, unit).map(|(_, p, m)| (p, m))
    }

    /// 解析模型提示：返回 (档案ID, Provider, 生效模型名)
    pub fn resolve_full(
        &self,
        hint: &str,
        unit: bool,
    ) -> Option<(String, std::sync::Arc<dyn LlmProvider>, String)> {
        let (pid, explicit) = match hint.split_once('/') {
            Some((p, m)) if !m.trim().is_empty() => (p, Some(m.trim().to_string())),
            _ => (hint, None),
        };
        let pid = pid.trim().to_string();
        let entry: &(std::sync::Arc<dyn LlmProvider>, String, String) =
            self.entries.get(&pid)?;
        let provider: std::sync::Arc<dyn LlmProvider> = entry.0.clone();
        let model: String = match explicit {
            Some(m) => m,
            None => {
                if unit {
                    entry.2.clone()
                } else {
                    entry.1.clone()
                }
            }
        };
        Some((pid, provider, model))
    }
}

// ---------------------------------------------------------------- 失败回退

/// 失败冷却时长（秒）：档案失败后回退链上被跳过的时间窗
pub const FAILOVER_COOLDOWN_SECS: u64 = 45;

/// 失败冷却：档案请求失败后标记一段时间，回退链上被跳过（首个候选除外）
pub struct FailoverState {
    until: parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

impl Default for FailoverState {
    fn default() -> Self {
        FailoverState { until: parking_lot::Mutex::new(std::collections::HashMap::new()) }
    }
}

impl FailoverState {
    pub fn cooling(&self, id: &str) -> bool {
        let mut m = self.until.lock();
        match m.get(id) {
            Some(t) if *t > std::time::Instant::now() => true,
            Some(_) => {
                m.remove(id);
                false
            }
            None => false,
        }
    }

    /// 标记冷却（秒）
    pub fn cool(&self, id: &str, secs: u64) {
        self.until
            .lock()
            .insert(id.to_string(), std::time::Instant::now() + std::time::Duration::from_secs(secs));
    }

    /// 成功即解除冷却
    pub fn clear(&self, id: &str) {
        self.until.lock().remove(id);
    }
}

/// 回退候选链：按顺序尝试，失败（未发出内容前）切换下一候选并冷却失败者
pub struct ModelChain {
    pub candidates: Vec<(String, std::sync::Arc<dyn LlmProvider>, String)>,
    pub failover: std::sync::Arc<FailoverState>,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse>;
    /// 文本嵌入（混合记忆检索）；默认不支持，openai/azure 协议实现
    async fn embed(&self, _model: &str, _texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        anyhow::bail!("该通道不支持嵌入")
    }
    /// 流式：增量通过 tx 发出，最终返回完整响应
    async fn stream(
        &self,
        req: ChatRequest,
        tx: UnboundedSender<String>,
    ) -> anyhow::Result<ChatResponse>;
}

// ---------------------------------------------------------------- OpenAI 兼容

pub struct OpenAiCompatibleProvider {
    base_url: String,
    /// API 协议：openai（默认，/chat/completions）| anthropic（/v1/messages）
    api_format: String,
    /// Key 池：多把 Key 负载均衡——按发起方粘性选键，仅限额时前进（保留端点侧 KV 缓存）
    keys: Vec<String>,
    sticky: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(base_url: impl Into<String>, keys: Vec<String>) -> Self {
        Self::with_format(base_url, keys, "openai")
    }

    pub fn with_format(base_url: impl Into<String>, keys: Vec<String>, api_format: &str) -> Self {
        let fmt = match api_format {
            "anthropic" | "azure" | "gemini" => api_format,
            _ => "openai",
        };
        OpenAiCompatibleProvider {
            base_url: base_url.into(),
            api_format: fmt.into(),
            keys: keys.into_iter().filter(|k| !k.trim().is_empty()).collect(),
            sticky: std::sync::Mutex::new(std::collections::HashMap::new()),
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, model: &str, stream: bool) -> String {
        let base = self.base_url.trim_end_matches('/');
        match self.api_format.as_str() {
            "anthropic" => format!("{base}/v1/messages"),
            // Azure：部署名入 URL，鉴权用 api-key 头
            "azure" => format!("{base}/openai/deployments/{model}/chat/completions?api-version=2024-10-21"),
            // Gemini 原生：generateContent / streamGenerateContent(alt=sse)，模型入 URL
            "gemini" if stream => format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse"),
            "gemini" => format!("{base}/v1beta/models/{model}:generateContent"),
            _ => format!("{base}/chat/completions"),
        }
    }

    fn auth_header(&self, key: &str) -> (reqwest::header::HeaderName, reqwest::header::HeaderValue) {
        let val = reqwest::header::HeaderValue::from_str(key)
            .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static(""));
        match self.api_format.as_str() {
            "anthropic" => (reqwest::header::HeaderName::from_static("x-api-key"), val),
            "azure" => (reqwest::header::HeaderName::from_static("api-key"), val),
            "gemini" => (reqwest::header::HeaderName::from_static("x-goog-api-key"), val),
            _ => (
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
                    .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("")),
            ),
        }
    }

    fn build_request(
        &self,
        key: &str,
        model: &str,
        stream: bool,
        body: serde_json::Value,
    ) -> reqwest::RequestBuilder {
        let (name, value) = self.auth_header(key);
        let rb = self.client.post(self.url(model, stream)).header(name, value);
        match self.api_format.as_str() {
            "anthropic" => rb.header("anthropic-version", "2023-06-01").json(&body),
            _ => rb.json(&body),
        }
    }

    /// 嵌入用键：直接取第一把（无发起方粘性语义）
    fn key_hint_texts(&self) -> String {
        self.keys.first().cloned().unwrap_or_default()
    }

    /// 粘性选键：同 hint 稳定命中同一把；首次按 hint 散列分布（不同个体自然错开不同 Key）
    fn pick_index(&self, hint: &str) -> usize {
        let mut m = self.sticky.lock().unwrap();
        *m.entry(hint.to_string()).or_insert_with(|| {
            let seed: u32 = hint.bytes().fold(5381u32, |a, b| a.wrapping_mul(33).wrapping_add(b as u32));
            seed as usize % self.keys.len().max(1)
        })
    }

    /// 限额切换：前进到下一把并粘住
    fn rotate_index(&self, hint: &str) -> usize {
        let mut m = self.sticky.lock().unwrap();
        let next = (*m.entry(hint.to_string()).or_insert(0) + 1) % self.keys.len().max(1);
        m.insert(hint.to_string(), next);
        next
    }

    /// 限额类失败判定（429/402/403 或配额、限流文案）
    fn is_quota_failure(status: u16, body: &str) -> bool {
        let low = body.to_lowercase();
        [429, 402].contains(&status)
            || status == 403
            || low.contains("quota")
            || low.contains("rate limit")
            || low.contains("insufficient")
            || low.contains("too many requests")
            || low.contains("resource_exhausted")
    }

    fn key_for(&self, hint: &str) -> String {
        if self.keys.is_empty() {
            return String::new();
        }
        self.keys[self.pick_index(hint)].clone()
    }

    fn body(&self, req: &ChatRequest, stream: bool) -> serde_json::Value {
        let mut base = match self.api_format.as_str() {
            "anthropic" => {
                let system: String = req
                    .messages
                    .iter()
                    .filter(|m| m.role == "system")
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>()
                    .join("
");
                let messages: Vec<serde_json::Value> = req
                    .messages
                    .iter()
                    .filter(|m| m.role != "system")
                    .map(|m| self.map_message(m))
                    .collect();
                serde_json::json!({
                    "model": req.model, "system": system, "messages": messages,
                    "max_tokens": req.max_tokens.unwrap_or(1024), "stream": stream,
                })
            }
            "gemini" => {
                let system: String = req
                    .messages
                    .iter()
                    .filter(|m| m.role == "system")
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                let contents: Vec<serde_json::Value> = req
                    .messages
                    .iter()
                    .filter(|m| m.role != "system")
                    .map(|m| self.map_message(m))
                    .collect();
                let mut body = serde_json::json!({
                    "contents": contents,
                    "generationConfig": { "temperature": req.temperature },
                });
                if !system.is_empty() {
                    body["systemInstruction"] = serde_json::json!({ "parts": [{ "text": system }] });
                }
                if let Some(mt) = req.max_tokens {
                    body["generationConfig"]["maxOutputTokens"] = serde_json::json!(mt);
                }
                body
            }
            _ => {
                let messages: Vec<serde_json::Value> = req
                    .messages
                    .iter()
                    .map(|m| self.map_message(m))
                    .collect();
                let mut body = serde_json::json!({
                    "model": req.model, "messages": messages,
                    "temperature": req.temperature, "stream": stream,
                });
                if let Some(mt) = req.max_tokens {
                    body["max_tokens"] = serde_json::json!(mt);
                }
                body
            }
        };
        // 原生 function calling：按协议注入工具定义
        if !req.tools.is_empty() {
            match self.api_format.as_str() {
                "anthropic" => {
                    base["tools"] = serde_json::json!(req.tools.iter().map(|t| serde_json::json!({
                        "name": t.name, "description": t.description, "input_schema": t.parameters,
                    })).collect::<Vec<_>>());
                }
                "gemini" => {
                    base["tools"] = serde_json::json!([{ "functionDeclarations": req.tools.iter().map(|t| serde_json::json!({
                        "name": t.name, "description": t.description, "parameters": t.parameters,
                    })).collect::<Vec<_>>() }]);
                }
                _ => {
                    base["tools"] = serde_json::json!(req.tools.iter().map(|t| serde_json::json!({
                        "type": "function",
                        "function": { "name": t.name, "description": t.description, "parameters": t.parameters },
                    })).collect::<Vec<_>>());
                    base["tool_choice"] = serde_json::json!("auto");
                }
            }
        }
        base
    }

    /// 单条消息 → 各协议请求形态（含原生工具调用线程）
    fn map_message(&self, m: &ChatMessage) -> serde_json::Value {
        match self.api_format.as_str() {
            "anthropic" => {
                if !m.tool_calls.is_empty() {
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(serde_json::json!({ "type": "text", "text": m.content }));
                    }
                    for c in &m.tool_calls {
                        blocks.push(serde_json::json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments }));
                    }
                    serde_json::json!({ "role": "assistant", "content": blocks })
                } else if m.role == "tool" {
                    serde_json::json!({
                        "role": "user",
                        "content": [{ "type": "tool_result", "tool_use_id": m.tool_call_id.clone().unwrap_or_default(), "content": m.content }],
                    })
                } else if !m.images.is_empty() {
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(serde_json::json!({ "type": "text", "text": m.content }));
                    }
                    for img in &m.images {
                        if let Some((mime, data)) = ChatMessage::split_data_url(img) {
                            blocks.push(serde_json::json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }));
                        }
                    }
                    serde_json::json!({ "role": m.role, "content": blocks })
                } else {
                    serde_json::json!({ "role": m.role, "content": m.content })
                }
            }
            "gemini" => {
                if !m.tool_calls.is_empty() {
                    let parts: Vec<serde_json::Value> = m.tool_calls.iter()
                        .map(|c| serde_json::json!({ "functionCall": { "name": c.name, "args": c.arguments } }))
                        .collect();
                    serde_json::json!({ "role": "model", "parts": parts })
                } else if m.role == "tool" {
                    let name = m.name.clone().or_else(|| m.tool_call_id.clone()).unwrap_or_default();
                    serde_json::json!({
                        "role": "user",
                        "parts": [{ "functionResponse": { "name": name, "response": { "result": m.content } } }],
                    })
                } else if !m.images.is_empty() {
                    let mut parts: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        parts.push(serde_json::json!({ "text": m.content }));
                    }
                    for img in &m.images {
                        if let Some((mime, data)) = ChatMessage::split_data_url(img) {
                            parts.push(serde_json::json!({ "inline_data": { "mime_type": mime, "data": data } }));
                        }
                    }
                    let role = if m.role == "assistant" { "model" } else { m.role.as_str() };
                    serde_json::json!({ "role": role, "parts": parts })
                } else {
                    let role = if m.role == "assistant" { "model" } else { m.role.as_str() };
                    serde_json::json!({ "role": role, "parts": [{ "text": m.content }] })
                }
            }
            _ => {
                if !m.tool_calls.is_empty() {
                    let calls: Vec<serde_json::Value> = m.tool_calls.iter().map(|c| serde_json::json!({
                        "id": c.id, "type": "function",
                        "function": { "name": c.name, "arguments": serde_json::to_string(&c.arguments).unwrap_or_default() },
                    })).collect();
                    let content = if m.content.is_empty() { serde_json::Value::Null } else { serde_json::json!(m.content) };
                    serde_json::json!({ "role": "assistant", "content": content, "tool_calls": calls })
                } else if m.role == "tool" {
                    serde_json::json!({
                        "role": "tool",
                        "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                        "content": m.content,
                    })
                } else if !m.images.is_empty() {
                    let mut parts: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        parts.push(serde_json::json!({ "type": "text", "text": m.content }));
                    }
                    for img in &m.images {
                        parts.push(serde_json::json!({ "type": "image_url", "image_url": { "url": img } }));
                    }
                    serde_json::json!({ "role": m.role, "content": parts })
                } else {
                    serde_json::json!({ "role": m.role, "content": m.content })
                }
            }
        }
    }

    /// 非流式响应中的工具调用（各协议）
    fn parse_tool_calls(&self, v: &serde_json::Value) -> Vec<ToolCall> {
        match self.api_format.as_str() {
            "anthropic" => v.get("content").and_then(|c| c.as_array()).map(|arr| arr.iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .enumerate()
                .map(|(i, b)| ToolCall {
                    id: b.get("id").and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| format!("call_{i}")),
                    name: b.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    arguments: b.get("input").cloned().unwrap_or_else(|| serde_json::json!({})),
                }).collect()).unwrap_or_default(),
            "gemini" => v.pointer("/candidates/0/content/parts").and_then(|p| p.as_array()).map(|arr| arr.iter()
                .enumerate()
                .filter_map(|(i, part)| part.get("functionCall").map(|fc| ToolCall {
                    id: format!("call_{i}"),
                    name: fc.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                    arguments: fc.get("args").cloned().unwrap_or_else(|| serde_json::json!({})),
                })).collect()).unwrap_or_default(),
            _ => v.pointer("/choices/0/message/tool_calls").and_then(|c| c.as_array()).map(|arr| arr.iter()
                .enumerate()
                .filter_map(|(i, tc)| {
                    let name = tc.pointer("/function/name").and_then(|n| n.as_str())?;
                    let raw = tc.pointer("/function/arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                    Some(ToolCall {
                        id: tc.get("id").and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| format!("call_{i}")),
                        name: name.to_string(),
                        arguments: serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({})),
                    })
                }).collect()).unwrap_or_default(),
        }
    }

    /// 非流式响应 → 文本（多协议）
    fn parse_response(&self, v: &serde_json::Value) -> Option<String> {
        match self.api_format.as_str() {
            "anthropic" => v.get("content").and_then(|c| c.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            }),
            "gemini" => Self::gemini_parts(v),
            _ => v.get("choices").and_then(|c| c.get(0))
                .and_then(|c| c.pointer("/message/content"))
                .and_then(|t| t.as_str())
                .map(String::from),
        }
    }

    /// 流式增量提取（多协议）
    fn parse_delta(&self, v: &serde_json::Value) -> Option<String> {
        match self.api_format.as_str() {
            "anthropic" => v.pointer("/delta/text").and_then(|t| t.as_str()).map(String::from),
            "gemini" => Self::gemini_parts(v),
            _ => v.get("choices").and_then(|c| c.get(0))
                .and_then(|c| c.pointer("/delta/content"))
                .and_then(|t| t.as_str())
                .map(String::from),
        }
    }

    /// Gemini：candidates[0].content.parts[*].text 拼接（非流式与流式分片同构）
    fn gemini_parts(v: &serde_json::Value) -> Option<String> {
        v.get("candidates").and_then(|c| c.get(0)).and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts")).and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
    }
}

#[derive(Deserialize)]
struct ChatCompletion {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<RespMessage>,
    delta: Option<RespMessage>,
}

#[derive(Deserialize)]
struct RespMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    /// 文本嵌入：openai 协议走 /embeddings；azure 走 deployments/{model}/embeddings；其余不支持
    async fn embed(&self, model: &str, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if self.keys.is_empty() {
            anyhow::bail!("未配置 API Key（嵌入不可用）");
        }
        let fmt = self.api_format.as_str();
        if fmt != "openai" && fmt != "azure" {
            anyhow::bail!("协议 {fmt} 暂不支持 /embeddings 端点");
        }
        let base = self.base_url.trim_end_matches('/');
        let url = if fmt == "azure" {
            format!("{base}/openai/deployments/{model}/embeddings?api-version=2024-10-21")
        } else {
            format!("{base}/embeddings")
        };
        let key = self.key_hint_texts();
        let (name, value) = self.auth_header(&key);
        let resp = self
            .client
            .post(&url)
            .header(name, value)
            .json(&serde_json::json!({ "model": model, "input": texts }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("嵌入请求失败: {e}"))?;
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        if !(200..300).contains(&status) {
            anyhow::bail!("嵌入失败 {status}: {}", body.to_string().chars().take(200).collect::<String>());
        }
        let mut out: Vec<(usize, Vec<f32>)> = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|d| {
                        let idx = d.get("index").and_then(|i| i.as_u64())? as usize;
                        let vec = d.get("embedding")?.as_array()?;
                        let v: Vec<f32> = vec.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
                        Some((idx, v))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|(i, _)| *i);
        let vectors: Vec<Vec<f32>> = out.into_iter().map(|(_, v)| v).collect();
        if vectors.len() != texts.len() {
            anyhow::bail!("嵌入返回数量不符（{} / {}）", vectors.len(), texts.len());
        }
        Ok(vectors)
    }

    fn name(&self) -> &'static str {
        match self.api_format.as_str() {
            "anthropic" => "anthropic",
            "azure" => "azure-openai",
            "gemini" => "google-gemini",
            _ => "openai-compatible",
        }
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        let hint = req.key_hint.clone().unwrap_or_default();
        if self.keys.is_empty() {
            anyhow::bail!("未配置 API Key（Mock 通道不应使用真实 Provider）");
        }
        let mut last_err = String::new();
        for _ in 0..self.keys.len() {
            let key = self.key_for(&hint);
            let resp = self
                .build_request(&key, &req.model, false, self.body(&req, false))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("LLM 请求失败: {e}"))?;
            let status = resp.status().as_u16();
            if !(200..300).contains(&status) {
                let text = resp.text().await.unwrap_or_default();
                if Self::is_quota_failure(status, &text) && self.keys.len() > 1 {
                    last_err = format!("Key#{p} 限额（{status}）", p = self.pick_index(&hint));
                    self.rotate_index(&hint);
                    continue; // 限额 → 切换下一把 Key 并粘住
                }
                anyhow::bail!("LLM 请求失败 {status}: {}", text.chars().take(300).collect::<String>());
            }
            let parsed: serde_json::Value = resp.json().await?;
            let content = self.parse_response(&parsed).unwrap_or_default();
            let tool_calls = self.parse_tool_calls(&parsed);
            let usage = parsed.get("usage");
            let pt = usage
                .and_then(|u| {
                    u.get("input_tokens")
                        .or_else(|| u.get("prompt_tokens"))
                        .or_else(|| u.get("promptTokenCount"))
                })
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let ct = usage
                .and_then(|u| {
                    u.get("output_tokens")
                        .or_else(|| u.get("completion_tokens"))
                        .or_else(|| u.get("candidatesTokenCount"))
                })
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            return Ok(ChatResponse { content, prompt_tokens: pt, completion_tokens: ct, tool_calls });
        }
        anyhow::bail!("全部 API Key 均不可用：{last_err}")
    }

    async fn stream(
        &self,
        req: ChatRequest,
        tx: UnboundedSender<String>,
    ) -> anyhow::Result<ChatResponse> {
        let hint = req.key_hint.clone().unwrap_or_default();
        if self.keys.is_empty() {
            anyhow::bail!("未配置 API Key（Mock 通道不应使用真实 Provider）");
        }
        let resp = loop {
            let key = self.key_for(&hint);
            let r = self
                .build_request(&key, &req.model, true, self.body(&req, true))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("LLM 流式请求失败: {e}"))?;
            let status = r.status().as_u16();
            if !(200..300).contains(&status) {
                let text = r.text().await.unwrap_or_default();
                if Self::is_quota_failure(status, &text) && self.keys.len() > 1 {
                    self.rotate_index(&hint);
                    continue;
                }
                anyhow::bail!("LLM 流式请求失败 {status}: {}", text.chars().take(300).collect::<String>());
            }
            break r;
        };

        let mut acc = String::new();
        let mut buf = String::new();
        // 工具调用流式分片组装：openai/anthropic 按索引拼增量，gemini 为整块
        let mut call_frags: Vec<(String, String, String)> = Vec::new(); // (id, name, arguments-json)
        let mut gemini_calls: Vec<ToolCall> = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(pos) = buf.find('\n') {
                let line: String = buf.drain(..=pos).collect();
                let line = line.trim();
                if !line.starts_with("data:") {
                    continue;
                }
                let payload = line.trim_start_matches("data:").trim();
                if payload == "[DONE]" {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                    match self.api_format.as_str() {
                        "anthropic" => {
                            let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            if ty == "content_block_start" {
                                if let Some(block) = v.pointer("/content_block") {
                                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                        let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                        while call_frags.len() <= idx {
                                            call_frags.push((String::new(), String::new(), String::new()));
                                        }
                                        call_frags[idx].0 = block.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        call_frags[idx].1 = block.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                    }
                                }
                            } else if ty == "content_block_delta" {
                                if let Some(d) = v.get("delta") {
                                    if d.get("type").and_then(|t| t.as_str()) == Some("input_json_delta") {
                                        let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                        while call_frags.len() <= idx {
                                            call_frags.push((String::new(), String::new(), String::new()));
                                        }
                                        call_frags[idx].2.push_str(d.get("partial_json").and_then(|p| p.as_str()).unwrap_or(""));
                                    }
                                }
                            }
                        }
                        "gemini" => {
                            if let Some(parts) = v.pointer("/candidates/0/content/parts").and_then(|p| p.as_array()) {
                                for part in parts {
                                    if let Some(fc) = part.get("functionCall") {
                                        gemini_calls.push(ToolCall {
                                            id: format!("gem_{}", gemini_calls.len()),
                                            name: fc.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                                            arguments: fc.get("args").cloned().unwrap_or_else(|| serde_json::json!({})),
                                        });
                                    }
                                }
                            }
                        }
                        _ => {
                            if let Some(fcs) = v.pointer("/choices/0/delta/tool_calls").and_then(|c| c.as_array()) {
                                for tc in fcs {
                                    let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(call_frags.len() as u64) as usize;
                                    while call_frags.len() <= idx {
                                        call_frags.push((String::new(), String::new(), String::new()));
                                    }
                                    if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                                        call_frags[idx].0 = id.to_string();
                                    }
                                    if let Some(nm) = tc.pointer("/function/name").and_then(|x| x.as_str()) {
                                        call_frags[idx].1 = nm.to_string();
                                    }
                                    if let Some(a) = tc.pointer("/function/arguments").and_then(|x| x.as_str()) {
                                        call_frags[idx].2.push_str(a);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(delta) = self.parse_delta(&v) {
                        acc.push_str(&delta);
                        let _ = tx.send(delta);
                    }
                }
            }
        }
        let mut tool_calls: Vec<ToolCall> = call_frags
            .into_iter()
            .enumerate()
            .filter(|(_, (_, name, _))| !name.is_empty())
            .map(|(i, (id, name, args))| ToolCall {
                id: if id.is_empty() { format!("call_{i}") } else { id },
                name,
                arguments: serde_json::from_str(&args).unwrap_or_else(|_| serde_json::json!({})),
            })
            .collect();
        tool_calls.extend(gemini_calls);
        Ok(ChatResponse { content: acc, prompt_tokens: 0, completion_tokens: 0, tool_calls })
    }
}

// ---------------------------------------------------------------- Mock 通道

/// 确定性模拟：计划/子个体/收束三类请求各自产出合法契约产物
pub struct MockLlmProvider;

/// 确定性伪嵌入（Mock 通道）：64 维，token 哈希累加后归一——同义文本向量稳定相近
fn mock_embedding(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; 64];
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        let gram: String = if i + 1 < chars.len() { chars[i..i + 2].iter().collect() } else { chars[i].to_string() };
        let h: u32 = gram.bytes().fold(2166136261u32, |a, b| a.wrapping_mul(16777619).wrapping_add(b as u32));
        v[(h as usize) % 64] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter().map(|x| x / norm).collect()
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        Ok(ChatResponse { content: respond(&req), prompt_tokens: 100, completion_tokens: 200, tool_calls: Vec::new() })
    }

    async fn embed(&self, _model: &str, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| mock_embedding(t)).collect())
    }

    async fn stream(
        &self,
        req: ChatRequest,
        tx: UnboundedSender<String>,
    ) -> anyhow::Result<ChatResponse> {
        let content = respond(&req);
        for chunk in content.as_bytes().chunks(48) {
            let piece = String::from_utf8_lossy(chunk).to_string();
            let _ = tx.send(piece);
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        Ok(ChatResponse { content, prompt_tokens: 100, completion_tokens: 200, tool_calls: Vec::new() })
    }
}

fn respond(req: &ChatRequest) -> String {
    let system = req
        .messages
        .iter()
        .find(|m| m.role == "system")
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let user = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();

    if system.contains("会话压缩") {
        "目标与进展：用户持续在推进当前任务；关键结论与未决事项已由最近对话承载。".to_string()
    } else if system.contains("经验改进要点合成") {
        adaptation_text(&system)
    } else if user.starts_with("【收束请求】") {
        converge_text(&user)
    } else if system.contains("OrchestratorPlan") {
        // 指挥体规划请求（内置组与自定义组共用该契约标记）
        plan_json(&system, &user)
    } else {
        sync_json(&system, &user)
    }
}

/// 模拟通道的经验要点合成：确定性产出
fn adaptation_text(system: &str) -> String {
    let agent = system
        .split("个体：")
        .nth(1)
        .and_then(|s| s.split("（").next())
        .unwrap_or("个体")
        .trim()
        .to_string();
    format!(
        "- {agent} 在失败与受阻任务上先给出最小复现路径，再扩大处理范围
- 受阻回流必须携带解除条件，避免重复尝试同一方案
- 结论与证据等级对齐：无直接证据的判断显式标注为推断
- 输出前自查 SyncReport 契约字段完整性，缺项视为未完成"
    )
}

fn goal_of(user: &str) -> String {
    user.split("【要求】")
        .next()
        .unwrap_or(user)
        .replace("用户任务输入：", "")
        .trim()
        .chars()
        .take(200)
        .collect::<String>()
}

/// 从规划提示的「可调度子个体」清单解析 identifier（组感知：自定义组也能出合法计划）。
/// 仅接受 "- ident（" 形式（全角左括号紧跟），避免误吞可靠性统计/记忆等其它块。
fn parse_dispatchable(system: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in system.lines() {
        if line.starts_with("## ") {
            in_block = line.contains("可调度子个体");
            continue;
        }
        if !in_block {
            continue;
        }
        let Some(rest) = line.strip_prefix("- ") else { continue };
        let ident: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !ident.is_empty() && rest[ident.len()..].starts_with('（') {
            out.push(ident);
        }
    }
    out.dedup();
    out
}

fn plan_json(system: &str, user: &str) -> String {
    let goal = goal_of(user);
    let agents = parse_dispatchable(system);

    // 组内无子个体：主智能体 L0 直答（用户可先建组再让主智能体创建个体）
    if agents.is_empty() {
        let l0 = serde_json::json!({
            "routeLevel": "L0",
            "goal": goal,
            "boundary": {
                "inScope": ["当前会话目标"],
                "forbidden": ["不可逆变更", "越权调度"]
            },
            "acceptance": ["直接回答完整覆盖目标"],
            "nodes": [],
            "finalAnswer": [
                { "tag": "报告", "text": format!("（模拟通道·L0 直答）{goal}") },
                { "tag": "观测", "text": "当前组仅有主智能体；可要求主智能体创建子个体后再分发任务。" }
            ]
        });
        return format!("```json\n{}\n```", serde_json::to_string_pretty(&l0).unwrap_or_default());
    }

    let a = |i: usize| agents.get(i).cloned().unwrap_or_else(|| agents[0].clone());
    let nodes = if agents.len() == 1 {
        serde_json::json!([
            {
                "id": "T1", "title": "任务执行", "agentIdentifier": a(0),
                "objective": format!("完成以下任务的全部职责范围内工作：{goal}"),
                "acceptance": ["结论带证据等级", "残余未知显式保留"],
                "dependsOn": [], "priority": "P1"
            },
            {
                "id": "T2", "title": "复核收束", "agentIdentifier": a(0),
                "objective": format!("复核 T1 产出，整理结论、风险与下一步：{goal}"),
                "acceptance": ["结论带证据等级", "风险与残余未知完整"],
                "dependsOn": ["T1"], "priority": "P1"
            }
        ])
    } else {
        serde_json::json!([
            {
                "id": "T1", "title": "背景补齐", "agentIdentifier": a(0),
                "objective": format!("补齐以下任务的背景、依赖、约束与边界：{goal}"),
                "acceptance": ["背景事实带证据等级", "缺口显式列出"],
                "dependsOn": [], "priority": "P1"
            },
            {
                "id": "T2", "title": "入口侦察", "agentIdentifier": a(1 % agents.len()),
                "objective": format!("定位与任务相关的文件、入口、依赖与现有模式：{goal}"),
                "acceptance": ["相关位置清单", "现有模式说明"],
                "dependsOn": [], "priority": "P1"
            },
            {
                "id": "T3", "title": "方案比对", "agentIdentifier": a(0),
                "objective": format!("基于 T1/T2 回流，对比候选路径的差异、取舍与适配边界：{goal}"),
                "acceptance": ["候选对比表", "推荐项与理由"],
                "dependsOn": ["T1", "T2"], "priority": "P1"
            },
            {
                "id": "T4", "title": "汇报收束", "agentIdentifier": a(agents.len().min(3) - 1),
                "objective": format!("汇总全部上游回流，输出结构化结论、风险与下一步：{goal}"),
                "acceptance": ["结论带证据等级", "风险与残余未知完整"],
                "dependsOn": ["T3"], "priority": "P1"
            }
        ])
    };

    let plan = serde_json::json!({
        "routeLevel": if agents.len() == 1 { "L1" } else { "L2" },
        "boundary": {
            "inScope": ["当前会话目标", "用户提供的材料"],
            "forbidden": ["不可逆变更", "越权调度", "无证据结论"]
        },
        "goal": goal,
        "acceptance": ["产出带证据等级的结论", "残余未知显式保留"],
        "nodes": nodes
    });
    format!("```json\n{}\n```", serde_json::to_string_pretty(&plan).unwrap_or_default())
}

fn sync_json(system: &str, user: &str) -> String {
    // identifier 只取 ASCII 字母/数字/连字符（提示词里紧跟中文说明，不能整段吞掉）
    let identifier = system
        .split("identifier: ")
        .nth(1)
        .map(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown-agent".to_string());

    let (node_id, objective) = if let Some(v) = crate::parse::extract_json(user) {
        (
            v.get("taskNodeId").and_then(|x| x.as_str()).unwrap_or("T?").to_string(),
            v.get("objective")
                .and_then(|x| x.as_str())
                .unwrap_or("未解析到目标")
                .chars()
                .take(120)
                .collect::<String>(),
        )
    } else {
        ("T?".to_string(), "未解析到目标".to_string())
    };

    let report = serde_json::json!({
        "sourceAgent": identifier,
        "taskNodeId": node_id,
        "status": "done",
        "statements": [
            { "tag": "报告", "text": format!("节点 {node_id} 执行完成（模拟通道）。目标：{objective}") },
            { "tag": "肯定", "text": "已按 DispatchOrder 的边界与验收口径完成本节点职责范围内的分析。", "evidenceLevel": "B" },
            { "tag": "观测", "text": "当前为模拟执行通道；接入真实 LLM 端点后此处为实际推理产出。" }
        ],
        "summary": format!("节点 {node_id}（{identifier}）已完成：{objective}。结论为模拟产出，证据等级 B，未发现阻断项。"),
        "evidence": [{
            "level": "B", "kind": "reasoning",
            "ref": format!("mock://{identifier}/{node_id}"),
            "note": "模拟通道占位证据；真实通道下为代码/日志/测试等直接证据。"
        }],
        "risks": [{
            "text": "模拟通道结论不可直接支撑高风险决策",
            "severity": "low",
            "revertPath": "配置真实 LLM 端点后重跑该节点"
        }],
        "blockers": [],
        "nextSuggestion": {},
        "confidence": 0.82
    });
    format!("```json\n{}\n```", serde_json::to_string_pretty(&report).unwrap_or_default())
}

fn converge_text(user: &str) -> String {
    let goal = goal_of(user);
    [
        "## 任务边界".to_string(),
        "- 目标：以用户输入为准；排除项：不可逆变更与越权动作。".into(),
        "".into(),
        "## 已启用链路".into(),
        "- 链路与个体序列见任务图；并行安排按 DAG 拓扑执行（模拟通道）。".into(),
        "".into(),
        "## 关键结论".into(),
        "- 【报告】全部节点已回流并通过 SyncReport 契约校验。（证据B，置信度 0.82）".into(),
        "- 【观测】当前为模拟执行通道，结论结构完整、内容为占位。".into(),
        "".into(),
        "## 冲突裁决（如有）".into(),
        "- 本轮未出现 need_arbitration 回流，无需裁决。".into(),
        "".into(),
        "## 残余未知".into(),
        "- 接入真实 LLM 端点后需以 A/B 级证据复核本轮结论。".into(),
        "".into(),
        "## 最终交付".into(),
        "- 流程闭环已验证：对话 → 分解 → 并行调度 → 回流 → 收束。".into(),
        format!("- 输入摘要：{goal}"),
    ]
    .join("\n")
}
