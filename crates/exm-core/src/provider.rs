//! LLM Provider 抽象 —— docs/04 §5
//! 一套代码接所有 OpenAI 兼容端点；无密钥时使用确定性 Mock 通道。

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage { role: "assistant".into(), content: content.into() }
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
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        ChatRequest { model: model.into(), messages, temperature: 0.2, max_tokens: None, key_hint: None }
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
}

// ---------------------------------------------------------------- 模型档案运行池

/// 模型档案运行池：档案ID → (Provider, 指挥体模型, 子个体模型)。
/// 供按个体/组解析默认模型（提示格式："档案ID" 或 "档案ID/模型名"）；
/// 档案编辑经 apply_config 重建池，组/个体的提示则热读取。
pub struct ModelPool {
    entries: std::collections::HashMap<String, (std::sync::Arc<dyn LlmProvider>, String, String)>,
}

impl Default for ModelPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelPool {
    pub fn new() -> Self {
        ModelPool { entries: std::collections::HashMap::new() }
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
        let (pid, explicit) = match hint.split_once('/') {
            Some((p, m)) if !m.trim().is_empty() => (p, Some(m.trim().to_string())),
            _ => (hint, None),
        };
        let entry: &(std::sync::Arc<dyn LlmProvider>, String, String) =
            self.entries.get(pid.trim())?;
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
        Some((provider, model))
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse>;
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
        match self.api_format.as_str() {
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
                    .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
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
                    .map(|m| {
                        serde_json::json!({
                            "role": if m.role == "assistant" { "model" } else { &m.role },
                            "parts": [{ "text": m.content }],
                        })
                    })
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
                    .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
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
            return Ok(ChatResponse { content, prompt_tokens: pt, completion_tokens: ct });
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
                if payload == "[DONE]" {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                    if let Some(delta) = self.parse_delta(&v) {
                        acc.push_str(&delta);
                        let _ = tx.send(delta);
                    }
                }
            }
        }
        Ok(ChatResponse { content: acc, prompt_tokens: 0, completion_tokens: 0 })
    }
}

// ---------------------------------------------------------------- Mock 通道

/// 确定性模拟：计划/子个体/收束三类请求各自产出合法契约产物
pub struct MockLlmProvider;

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        Ok(ChatResponse { content: respond(&req), prompt_tokens: 100, completion_tokens: 200 })
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
        Ok(ChatResponse { content, prompt_tokens: 100, completion_tokens: 200 })
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

    if system.contains("经验改进要点合成") {
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
