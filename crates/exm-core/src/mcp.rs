//! MCP（Model Context Protocol）客户端 —— 第三方工具生态接入（docs/架构与设计.md 扩展点）。
//!
//! 传输：stdio（本地子进程，JSON-RPC 逐行）| http（Streamable HTTP，POST JSON-RPC）。
//! 生命周期：懒连接（首次调用时 initialize），工具清单缓存快照供规划注入；
//! 服务器配置在 .exmachina/config.json 的 mcpServers 段，apply_config 热重建。

use crate::provider::ToolSpec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// MCP 服务器配置（config.json → mcpServers[]）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub id: String,
    /// stdio | http
    pub transport: String,
    /// stdio：要拉起的命令
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// http：端点 URL（如 https://host/mcp）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 允许使用该服务器工具的个体（空 = 全体）
    #[serde(default)]
    pub allowed_agents: Vec<String>,
    /// 启用
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 工具调用结果（MCP content 块规约为文本）
pub struct McpToolResult {
    pub ok: bool,
    pub text: String,
}

// ---------------------------------------------------------------- JSON-RPC 帧与客户端

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct RpcResponse {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone)]
struct McpToolInfo {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

/// HTTP 传输：无状态请求/响应（服务器端会话头透传保存）
struct HttpClient {
    url: String,
    client: reqwest::Client,
    session_id: std::sync::Mutex<Option<String>>,
}

impl HttpClient {
    fn new(url: &str) -> Self {
        HttpClient { url: url.to_string(), client: reqwest::Client::new(), session_id: std::sync::Mutex::new(None) }
    }

    async fn call_raw(&self, id: u64, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let body = serde_json::to_value(RpcRequest { jsonrpc: "2.0", id, method, params })?;
        let mut rb = self
            .client
            .post(&self.url)
            .header("accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(sid) = self.session_id.lock().unwrap().clone() {
            rb = rb.header("mcp-session-id", sid);
        }
        let resp = rb.send().await.map_err(|e| anyhow::anyhow!("MCP HTTP 请求失败: {e}"))?;
        if let Some(sid) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *self.session_id.lock().unwrap() = Some(sid.to_string());
        }
        let status = resp.status().as_u16();
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let text = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            anyhow::bail!("MCP HTTP {status}: {}", text.chars().take(200).collect::<String>());
        }
        // 响应可能是 JSON，或 SSE（data: 单帧）
        let payload = if ctype.contains("event-stream") {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(str::trim)
                .last()
                .unwrap_or("")
                .to_string()
        } else {
            text
        };
        let v: serde_json::Value = serde_json::from_str(payload.trim())?;
        if let Some(err) = v.get("error") {
            anyhow::bail!("MCP 错误: {}", err.get("message").and_then(|m| m.as_str()).unwrap_or("未知"));
        }
        Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }
}

/// stdio 传输：常驻子进程，stdin/stdout 逐行 JSON-RPC（BufReader 常驻避免丢缓冲）
struct StdioClient {
    #[allow(dead_code)]
    child: tokio::process::Child,
    stdin: Pin<Box<tokio::process::ChildStdin>>,
    reader: tokio::sync::Mutex<BufReader<tokio::process::ChildStdout>>,
}

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use std::pin::Pin;

impl StdioClient {
    async fn spawn(cfg: &McpServerConfig) -> anyhow::Result<Self> {
        let cmd = cfg.command.clone().unwrap_or_default();
        if cmd.trim().is_empty() {
            anyhow::bail!("stdio 服务器缺少 command: {}", cfg.id);
        }
        let mut command = tokio::process::Command::new(&cmd);
        command.args(&cfg.args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        for (k, v) in &cfg.env {
            command.env(k, v);
        }
        let mut child = command.spawn().map_err(|e| anyhow::anyhow!("MCP 进程拉起失败（{}/{}）: {e}", cfg.id, cmd))?;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("无 stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("无 stdout"))?;
        use tokio::io::AsyncWriteExt;
        stdin.flush().await.ok(); // 探活
        Ok(StdioClient { child, stdin: Box::pin(stdin), reader: tokio::sync::Mutex::new(BufReader::new(stdout)) })
    }

    async fn call_raw(&mut self, id: u64, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let body = serde_json::to_string(&RpcRequest { jsonrpc: "2.0", id, method, params })?;
        {
            let mut w = self.stdin.as_mut();
            w.write_all(body.as_bytes()).await?;
            w.write_all(b"\n").await?;
            w.flush().await?;
        }
        // 逐行读：跳过通知（无 id），匹配响应 id；BufReader 常驻保缓冲
        let mut reader = self.reader.lock().await;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("MCP 进程已退出: stdout 关闭");
            }
            if line.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
            if v.get("id").and_then(|x| x.as_u64()) != Some(id) {
                continue; // 通知或别人的响应
            }
            if let Some(err) = v.get("error") {
                anyhow::bail!("MCP 错误: {}", err.get("message").and_then(|m| m.as_str()).unwrap_or("未知"));
            }
            return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
        }
    }
}

// ---------------------------------------------------------------- 注册表

enum Transport {
    Http(HttpClient),
    Stdio(StdioClient),
}

/// 单服务器连接：懒初始化 + 工具/资源/提示词清单
struct McpConnection {
    cfg: McpServerConfig,
    transport: Option<Transport>,
    next_id: u64,
    tools: Vec<McpToolInfo>,
    /// resources/list：(uri, 名称)
    resources: Vec<(String, String)>,
    /// prompts/list：(名称, 描述)
    prompts: Vec<(String, String)>,
}

impl McpConnection {
    fn new(cfg: McpServerConfig) -> Self {
        McpConnection {
            cfg,
            transport: None,
            next_id: 1,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        }
    }

    async fn ensure_init(&mut self) -> anyhow::Result<()> {
        if self.transport.is_some() {
            return Ok(());
        }
        let t = match self.cfg.transport.as_str() {
            "http" => match &self.cfg.url {
                Some(url) => Transport::Http(HttpClient::new(url)),
                None => anyhow::bail!("http 服务器缺少 url: {}", self.cfg.id),
            },
            _ => Transport::Stdio(StdioClient::spawn(&self.cfg).await?),
        };
        self.transport = Some(t);
        // initialize 握手（失败则退化为 Failed，避免反复拉起）
        let params = serde_json::json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "exmachina", "version": env!("CARGO_PKG_VERSION") },
        });
        match self.call_raw("initialize", params).await {
            Ok(_) => Ok(()),
            Err(e) => {
                self.transport = None;
                anyhow::bail!("MCP initialize 失败（{}）: {e}", self.cfg.id)
            }
        }
    }

    async fn call_raw(&mut self, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        match &mut self.transport {
            Some(Transport::Http(c)) => c.call_raw(id, method, params).await,
            Some(Transport::Stdio(c)) => c.call_raw(id, method, params).await,
            None => anyhow::bail!("MCP 未初始化"),
        }
    }

    /// 刷新清单（initialize 后 tools/list + resources/list + prompts/list，后两者尽力而为）
    pub async fn refresh_tools(&mut self) -> anyhow::Result<usize> {
        self.ensure_init().await?;
        let result = self.call_raw("tools/list", serde_json::json!({})).await?;
        let tools: Vec<McpToolInfo> = result
            .get("tools")
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .unwrap_or_default();
        let n = tools.len();
        self.tools = tools;
        // resources / prompts：服务器可能不支持 —— 失败即视为空，不阻塞工具面
        if let Ok(r) = self.call_raw("resources/list", serde_json::json!({})).await {
            self.resources = r
                .get("resources")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| {
                            let uri = x.get("uri").and_then(|u| u.as_str())?.to_string();
                            let name = x
                                .get("name")
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                            Some((uri, name))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        if let Ok(r) = self.call_raw("prompts/list", serde_json::json!({})).await {
            self.prompts = r
                .get("prompts")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| {
                            let name = x.get("name").and_then(|u| u.as_str())?.to_string();
                            let desc = x
                                .get("description")
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                            Some((name, desc))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        Ok(n)
    }

    pub fn tool_snapshot(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tools
            .iter()
            .map(|t| ToolSpec {
                name: format!("mcp:{}:{}", self.cfg.id, t.name),
                description: if t.description.is_empty() { format!("MCP 工具 {}/{}", self.cfg.id, t.name) } else { t.description.clone() },
                parameters: if t.input_schema.is_null() { serde_json::json!({ "type": "object", "properties": {} }) } else { t.input_schema.clone() },
            })
            .collect();
        // 资源读取（MCP resources）：把服务器暴露的文档/数据纳入可调用工具面
        if !self.resources.is_empty() {
            let list = self
                .resources
                .iter()
                .take(30)
                .map(|(uri, name)| if name.is_empty() { format!("- {uri}") } else { format!("- {uri}（{name}）") })
                .collect::<Vec<_>>()
                .join("\n");
            specs.push(ToolSpec {
                name: format!("mcp:{}:__resource__", self.cfg.id),
                description: format!(
                    "读取 MCP 服务器 {} 暴露的资源。可用资源：\n{list}",
                    self.cfg.id
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "uri": { "type": "string", "description": "资源 URI" } },
                    "required": ["uri"]
                }),
            });
        }
        // 提示词模板（MCP prompts）：按名取回模板正文，供个体在特定任务上直接套用
        if !self.prompts.is_empty() {
            let list = self
                .prompts
                .iter()
                .take(30)
                .map(|(n, d)| if d.is_empty() { format!("- {n}") } else { format!("- {n}：{d}") })
                .collect::<Vec<_>>()
                .join("\n");
            specs.push(ToolSpec {
                name: format!("mcp:{}:__prompt__", self.cfg.id),
                description: format!("取回 MCP 服务器 {} 的提示词模板正文。可用模板：\n{list}", self.cfg.id),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "模板名" },
                        "arguments": { "type": "object", "description": "模板参数（可选）" }
                    },
                    "required": ["name"]
                }),
            });
        }
        specs
    }

    fn find_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t.name == name)
    }

    /// 调用工具：name 为服务器内工具名（不带 mcp: 前缀）。
    /// `__resource__` / `__prompt__` 为合成工具：分别走 resources/read 与 prompts/get。
    pub async fn call_tool(&mut self, name: &str, arguments: &serde_json::Value) -> McpToolResult {
        if name == "__resource__" {
            let uri = arguments.get("uri").and_then(|v| v.as_str()).unwrap_or("");
            if uri.trim().is_empty() {
                return McpToolResult { ok: false, text: "缺少 uri".into() };
            }
            return match self.call_raw("resources/read", serde_json::json!({ "uri": uri })).await {
                Ok(v) => {
                    let text = v
                        .get("contents")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| serde_json::to_string(&v).unwrap_or_default());
                    McpToolResult { ok: true, text }
                }
                Err(e) => McpToolResult { ok: false, text: format!("读取资源失败: {e}") },
            };
        }
        if name == "__prompt__" {
            let pname = arguments.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if pname.trim().is_empty() {
                return McpToolResult { ok: false, text: "缺少 name".into() };
            }
            let params = serde_json::json!({
                "name": pname,
                "arguments": arguments.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({})),
            });
            return match self.call_raw("prompts/get", params).await {
                Ok(v) => {
                    let text = v
                        .get("messages")
                        .and_then(|m| m.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| {
                                    m.pointer("/content/text").and_then(|t| t.as_str()).map(String::from)
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| serde_json::to_string(&v).unwrap_or_default());
                    McpToolResult { ok: true, text }
                }
                Err(e) => McpToolResult { ok: false, text: format!("取回提示词失败: {e}") },
            };
        }
        if !self.find_tool(name) {
            // 清单可能过期：刷新一次再试
            let _ = self.refresh_tools().await;
            if !self.find_tool(name) {
                return McpToolResult { ok: false, text: format!("MCP 工具不存在: {}/{}", self.cfg.id, name) };
            }
        }
        let params = serde_json::json!({ "name": name, "arguments": arguments });
        match self.call_raw("tools/call", params).await {
            Ok(result) => {
                let is_error = result.get("isError").and_then(|e| e.as_bool()).unwrap_or(false);
                let text = result
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|b| {
                                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                                    b.get("text").and_then(|t| t.as_str()).map(String::from)
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| serde_json::to_string(&result).unwrap_or_default());
                McpToolResult { ok: !is_error, text }
            }
            Err(e) => McpToolResult { ok: false, text: e.to_string() },
        }
    }
}

/// MCP 注册表：多服务器连接池 + 同步工具快照缓存
/// 连接池为异步锁（调用跨 await）；快照缓存为同步锁（规划注入热路径，绝不跨 await）。
pub struct McpRegistry {
    conns: tokio::sync::Mutex<HashMap<String, McpConnection>>,
    /// id -> (服务器配置摘要, 工具快照)——同步读取
    snapshot: parking_lot::Mutex<HashMap<String, (McpServerConfig, Vec<ToolSpec>)>>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        McpRegistry {
            conns: tokio::sync::Mutex::new(HashMap::new()),
            snapshot: parking_lot::Mutex::new(HashMap::new()),
        }
    }
}

impl McpRegistry {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn sync_snapshot(&self, id: &str, cfg: &McpServerConfig, tools: Vec<ToolSpec>) {
        self.snapshot.lock().insert(id.to_string(), (cfg.clone(), tools));
    }

    /// apply_config 时重建（配置未变的服务器保留连接）。
    /// 同步上下文安全：连接池正被异步调用占用时仅重建快照，连接留给下次调用自然重建。
    pub fn configure(&self, servers: &[McpServerConfig]) {
        let mut next_snapshot = HashMap::new();
        let mut next_conns = HashMap::new();
        let mut drained: HashMap<String, McpConnection> = match self.conns.try_lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(_) => HashMap::new(), // 忙碌：跳过连接复用（懒重建）
        };
        for cfg in servers {
            if !cfg.enabled {
                continue;
            }
            let mut conn = drained.remove(&cfg.id).unwrap_or_else(|| McpConnection::new(cfg.clone()));
            let unchanged = conn.cfg.command == cfg.command
                && conn.cfg.args == cfg.args
                && conn.cfg.url == cfg.url
                && conn.cfg.transport == cfg.transport;
            if !unchanged {
                conn = McpConnection::new(cfg.clone());
            } else {
                conn.cfg = cfg.clone();
            }
            let specs = conn.tool_snapshot();
            next_snapshot.insert(cfg.id.clone(), (cfg.clone(), specs));
            next_conns.insert(cfg.id.clone(), conn);
        }
        if let Ok(mut guard) = self.conns.try_lock() {
            *guard = next_conns;
        }
        *self.snapshot.lock() = next_snapshot;
    }

    /// 刷新全部工具清单（启动/手动触发；失败的服务器不阻塞其他）
    pub async fn refresh_all(&self) -> Vec<(String, anyhow::Result<usize>)> {
        let mut conns = self.conns.lock().await;
        let mut out = Vec::new();
        let ids: Vec<String> = conns.keys().cloned().collect();
        for id in ids {
            let Some(c) = conns.get_mut(&id) else { continue };
            let r = c.refresh_tools().await;
            let specs = c.tool_snapshot();
            self.sync_snapshot(&id, &c.cfg.clone(), specs);
            out.push((id, r));
        }
        out
    }

    /// 工具快照（供规划注入）：按 allowedAgents 过滤个体可见的 MCP 工具
    pub fn tool_snapshot_for(&self, agent_id: &str) -> Vec<ToolSpec> {
        self.snapshot
            .lock()
            .values()
            .filter(|(cfg, _)| cfg.allowed_agents.is_empty() || cfg.allowed_agents.iter().any(|a| a == agent_id))
            .flat_map(|(_, specs)| specs.clone())
            .collect()
    }

    /// 调用 MCP 工具（全名 mcp:server:tool）
    pub async fn call(&self, full_name: &str, agent_id: &str, arguments: &serde_json::Value) -> Option<McpToolResult> {
        let Some(rest) = full_name.strip_prefix("mcp:") else { return None };
        let Some((sid, tool)) = rest.split_once(':') else { return None };
        let allowed = {
            let snap = self.snapshot.lock();
            let Some((cfg, _)) = snap.get(sid) else { return None };
            if !cfg.allowed_agents.is_empty() && !cfg.allowed_agents.iter().any(|a| a == agent_id) {
                return Some(McpToolResult { ok: false, text: format!("MCP 服务器 {sid} 未对个体 {agent_id} 开放") });
            }
            true
        };
        if !allowed {
            return None;
        }
        let mut conns = self.conns.lock().await;
        let c = conns.get_mut(sid)?;
        let r = c.call_tool(tool, arguments).await;
        // 清单可能在调用中刷新过：同步快照
        self.sync_snapshot(sid, &c.cfg.clone(), c.tool_snapshot());
        Some(r)
    }

    /// 服务器与工具清单（网关管理端点用）
    pub fn servers_brief(&self) -> Vec<serde_json::Value> {
        self.snapshot
            .lock()
            .values()
            .map(|(cfg, specs)| {
                serde_json::json!({
                    "id": cfg.id, "transport": cfg.transport,
                    "url": cfg.url, "command": cfg.command,
                    "enabled": cfg.enabled,
                    "allowedAgents": cfg.allowed_agents,
                    "tools": specs.iter().map(|s| serde_json::json!({
                        "name": s.name, "description": s.description,
                    })).collect::<Vec<_>>(),
                    // MCP 三要素：tools / resources / prompts（后两者以合成工具形式下发）
                    "resourcesReadable": specs.iter().any(|s| s.name.ends_with(":__resource__")),
                    "promptsAvailable": specs.iter().any(|s| s.name.ends_with(":__prompt__")),
                })
            })
            .collect()
    }
}
