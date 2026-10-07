//! MCP 服务端（组 10）—— 把启用的内置工具按 ACL 暴露给外部 MCP 客户端（design D9）
//!
//! stdio 模式（`exm mcp serve`，CLI 子进程逐行 JSON-RPC）与 HTTP 模式（网关 `/<http_path>` 挂载）
//! 共用同一处理面：`McpServer::handle_request`。安全口径与内部执行完全一致：
//! 路径沙箱 / 审批闸门 / 工具审计 / 限流全部生效，审计来源标注 MCP 服务端与客户端身份。
//! ACL：允许的组、会话范围与工具清单；未授权范围对客户端不可见（不进入 tools/list）。
//! 默认关闭（`mcpServe.enabled = false` 时 handle_request 直接拒绝）。

use crate::tools::ToolGateway;
use crate::types::ToolName;
use crate::Core;
use serde_json::{json, Value};
use std::sync::Arc;

pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const SERVER_NAME: &str = "exmachina";

/// MCP 服务端处理面：`client` 为客户端身份标注（审计可归因；如 `stdio` / `http:<addr>`）
pub struct McpServer {
    core: Arc<Core>,
    client: String,
}

impl McpServer {
    pub fn new(core: Arc<Core>, client: impl Into<String>) -> Self {
        McpServer { core, client: client.into() }
    }

    fn acl(&self) -> crate::config::McpServeConfig {
        self.core.config().mcp_serve.clone()
    }

    /// 会话组上下文是否在 ACL 允许范围（allowed_groups 空 = 全部组）
    fn group_allowed(&self, acl: &crate::config::McpServeConfig, group: &str) -> bool {
        acl.allowed_groups.is_empty() || acl.allowed_groups.iter().any(|g| g == group)
    }

    /// 会话是否在 ACL 允许范围（allowed_sessions 空 = 全部会话）
    fn session_allowed(&self, acl: &crate::config::McpServeConfig, session_id: &str) -> bool {
        acl.allowed_sessions.is_empty() || acl.allowed_sessions.iter().any(|s| s == session_id)
    }

    /// 会话亲和校验：存在且 active 且在 ACL 范围 → 返回会话；否则拒绝（可读错误）
    fn resolve_session(&self, acl: &crate::config::McpServeConfig, session_id: &str) -> anyhow::Result<crate::types::Session> {
        let s = self
            .core
            .store
            .get_session(session_id)?
            .ok_or_else(|| anyhow::anyhow!("会话不存在: {session_id}"))?;
        if s.status != "active" {
            anyhow::bail!("会话已关闭: {session_id}");
        }
        if !self.session_allowed(acl, session_id) {
            anyhow::bail!("会话不在授权范围: {session_id}");
        }
        if !self.group_allowed(acl, &s.group_id) {
            anyhow::bail!("会话所属组不在授权范围: {}", s.group_id);
        }
        Ok(s)
    }

    /// 工具清单（ACL 可见性矩阵）：基础 = 内置启用工具（后端就绪口径与内部派发一致），
    /// 过滤 = allowed_tools（空 = 全部）。范围外工具不进入清单（不可见亦不可调用）。
    pub fn catalog(&self) -> Vec<crate::provider::ToolSpec> {
        let acl = self.acl();
        if !acl.enabled {
            return vec![];
        }
        let cfg = self.core.config();
        let gateway = ToolGateway::new(
            &cfg.workspace_root,
            self.core.store.clone(),
            self.core.registry.clone(),
            self.core.events.clone(),
            cfg.security.clone(),
        )
        .with_search(cfg.search.clone())
        .with_browser(cfg.browser.clone())
        .with_computer(cfg.computer.clone());
        let base: Vec<ToolName> = vec![
            ToolName::Read,
            ToolName::Edit,
            ToolName::Patch,
            ToolName::Grep,
            ToolName::Glob,
            ToolName::Filesystem,
            ToolName::Terminal,
            ToolName::WebSearch,
            ToolName::WebFetch,
            ToolName::Schedule,
            ToolName::Browser,
            ToolName::Computer,
        ];
        ToolGateway::tool_specs(
            &base,
            gateway.search_ready(),
            gateway.browser_ready(),
            gateway.computer_ready(),
        )
        .into_iter()
        .filter(|s| acl.allowed_tools.is_empty() || acl.allowed_tools.iter().any(|t| t == &s.name))
        .collect()
    }

    /// 执行一次工具调用：会话亲和（组上下文切换）+ ACL 可见性 + 与内部执行同路径。
    /// 审计 agent 标注 `mcp/<client>`（与内部调用可区分，spec：审计可归因）。
    pub async fn call(&self, session_id: &str, tool: &str, args: &Value) -> anyhow::Result<Value> {
        let acl = self.acl();
        if !acl.enabled {
            anyhow::bail!("MCP 服务端未启用");
        }
        let session = self.resolve_session(&acl, session_id)?;
        let spec = self
            .catalog()
            .into_iter()
            .find(|s| s.name == tool)
            .ok_or_else(|| anyhow::anyhow!("工具不可用或不在授权范围: {tool}"))?;
        let tool_name = ToolName::parse(&spec.name)
            .ok_or_else(|| anyhow::anyhow!("未知工具: {tool}"))?;

        // 会话亲和：在该会话组的上下文中执行（执行完还原）
        let prev_group = self.core.registry.active_group();
        let mut switched = false;
        if session.group_id != prev_group && self.core.group_meta(&session.group_id).is_some() {
            switched = self.core.registry.set_active_group(&session.group_id).is_ok();
        }
        let agent_label = format!("mcp/{}", self.client);
        let allowlist: Vec<ToolName> = vec![tool_name];
        let result = self
            .core
            .orchestrator()
            .tools
            .execute_named(&agent_label, session_id, &allowlist, &spec.name, args)
            .await;
        if switched {
            let _ = self.core.registry.set_active_group(&prev_group);
        }
        if result.ok {
            Ok(json!({
                "content": [{ "type": "text", "text": result.output }],
                "isError": false,
            }))
        } else {
            Ok(json!({
                "content": [{ "type": "text", "text": result.error.unwrap_or_else(|| "执行失败".into()) }],
                "isError": true,
            }))
        }
    }

    /// JSON-RPC 2.0 单请求处理（stdio 与 HTTP 共用）。通知类请求返回 None。
    pub async fn handle_request(&self, req: &Value) -> Option<Value> {
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let reply = |result: Value| -> Option<Value> {
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        };
        let err = |code: i64, message: &str| -> Option<Value> {
            Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
        };

        match method {
            // 通知（无 id）：不回复
            "notifications/initialized" | "notifications/cancelled" => None,
            "initialize" => {
                if !self.acl().enabled {
                    return err(-32000, "MCP 服务端未启用（mcpServe.enabled = false）");
                }
                reply(json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                }))
            }
            "ping" => reply(json!({})),
            "tools/list" => {
                if !self.acl().enabled {
                    return err(-32000, "MCP 服务端未启用（mcpServe.enabled = false）");
                }
                let tools: Vec<Value> = self
                    .catalog()
                    .into_iter()
                    .map(|s| json!({ "name": s.name, "description": s.description, "inputSchema": s.parameters }))
                    .collect();
                reply(json!({ "tools": tools }))
            }
            "tools/call" => {
                if !self.acl().enabled {
                    return err(-32000, "MCP 服务端未启用（mcpServe.enabled = false）");
                }
                let params = req.get("params").cloned().unwrap_or(json!({}));
                let tool = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mut arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                // 会话亲和：会话标识 = arguments.session（保留字段，从载荷剥离后不进工具实参）
                let session_id = arguments
                    .get("session")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let session_id = match session_id {
                    Some(s) => s,
                    None => {
                        return err(-32002, "缺少会话标识：请在 arguments.session 提供有效会话 id（会话亲和）")
                    }
                };
                if let Some(obj) = arguments.as_object_mut() {
                    obj.remove("session");
                }
                if tool.is_empty() {
                    return err(-32602, "缺少工具名（params.name）");
                }
                match self.call(&session_id, &tool, &arguments).await {
                    Ok(v) => reply(v),
                    // 工具不可见 / 会话无效 / 执行失败：isError 语义差异——
                    // 前两者是协议级拒绝（对不可见工具保持「不可用」口径），执行失败走工具级 isError
                    Err(e) => {
                        let msg = e.to_string();
                        let code = if msg.contains("不在授权范围") || msg.contains("未知工具") {
                            -32602
                        } else if msg.contains("会话") {
                            -32002
                        } else {
                            -32000
                        };
                        err(code, &msg)
                    }
                }
            }
            other => err(-32601, &format!("未知方法: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentDefinition;

    async fn server_with(security: impl FnOnce(&mut crate::config::ExmConfig)) -> (Arc<Core>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("exm-mcpd-{}", crate::types::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = crate::config::ExmConfig::load(&dir);
        cfg.use_mock = true;
        cfg.memory_enabled = false;
        cfg.mcp_serve.enabled = true;
        cfg.mcp_serve.allowed_tools = vec!["read".into(), "terminal".into(), "filesystem".into()];
        security(&mut cfg);
        let core = Arc::new(Core::with_config(cfg).unwrap());
        core.registry().create_group(Some("t".into()), "mcp组", "").unwrap();
        let def = AgentDefinition {
            name: "服务体".into(),
            identifier: "mcp-orch".into(),
            domain: "测试".into(),
            tier: crate::types::Tier::Orchestrator,
            description: "测试主智能体".into(),
            capabilities: vec![],
            tools: vec![],
            when_to_call: String::new(),
            link: None,
            dependencies: vec![],
            composable_with: vec![],
            input_schema: Default::default(),
            output_schema: Default::default(),
            prompt_file: String::new(),
            model_hint: None,
        };
        core.registry().upsert_agent("t", def, None).unwrap();
        core.registry().set_primary("t", "mcp-orch").unwrap();
        core.registry().set_active_group("t").unwrap();
        (core, dir)
    }

    async fn rpc(srv: &McpServer, method: &str, params: Value) -> Value {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        srv.handle_request(&req).await.expect("应回复")
    }

    /// 端到端协议脚本：initialize → tools/list → tools/call(read) → tools/call(terminal)
    #[tokio::test]
    async fn 协议端到端_initialize_清单_调用read与terminal成功() {
        let (core, dir) = server_with(|_| {}).await;
        std::fs::write(dir.join("hello.txt"), "mcp 你好").unwrap();
        let s = core.create_session("mcp 端到端").unwrap();
        let srv = McpServer::new(core.clone(), "stdio");

        // initialize
        let init = rpc(&srv, "initialize", json!({ "protocolVersion": PROTOCOL_VERSION, "capabilities": {} })).await;
        assert_eq!(init["result"]["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        // 通知不回复
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(srv.handle_request(&note).await.is_none());

        // tools/list：ACL 只放行 read/terminal/filesystem
        let list = rpc(&srv, "tools/list", json!({})).await;
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"read") && names.contains(&"terminal"));
        assert!(!names.contains(&"browser"), "ACL 外工具不可见：{names:?}");
        assert!(!names.contains(&"edit"), "ACL 外工具不可见：{names:?}");

        // tools/call read：会话亲和（组 t 工作区 = 全局工作区），读到刚写的文件
        let call = rpc(
            &srv,
            "tools/call",
            json!({ "name": "read", "arguments": { "session": s.id, "path": "hello.txt" } }),
        )
        .await;
        assert_eq!(call["result"]["isError"], false, "{}", call);
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("mcp 你好"), "read 应返回文件内容: {text}");

        // tools/call terminal（白名单命令，无审批闸门时直执行）
        let call = rpc(
            &srv,
            "tools/call",
            json!({ "name": "terminal", "arguments": { "session": s.id, "command": "echo mcp-ok" } }),
        )
        .await;
        assert_eq!(call["result"]["isError"], false, "{}", call);
        assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("mcp-ok"));

        // 审计来源标注 mcp/stdio
        let audit = core.store.list_tool_audit("mcp/stdio", 10).unwrap();
        assert!(audit.iter().any(|a| a["tool"] == "read"), "审计应含来源标注 mcp/stdio");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn 会话亲和_无效标识拒绝且无副作用() {
        let (core, dir) = server_with(|_| {}).await;
        let srv = McpServer::new(core.clone(), "stdio");
        // 缺会话标识
        let missing = rpc(&srv, "tools/call", json!({ "name": "read", "arguments": { "path": "x.txt" } })).await;
        assert!(missing["error"]["code"] == -32002, "{}", missing);
        // 不存在的会话
        let ghost = rpc(
            &srv,
            "tools/call",
            json!({ "name": "read", "arguments": { "session": "no-such", "path": "x.txt" } }),
        )
        .await;
        assert!(ghost["error"]["message"].as_str().unwrap().contains("会话不存在"), "{}", ghost);
        assert_eq!(core.store.list_tool_audit("mcp/stdio", 10).unwrap().len(), 0, "拒绝的调用无执行副作用");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn acl_可见性矩阵_组范围与会话范围_默认关闭() {
        let (core, dir) = server_with(|cfg| {
            cfg.mcp_serve.allowed_groups = vec!["t".into()];
            cfg.mcp_serve.allowed_sessions = vec![];
        }).await;
        let srv = McpServer::new(core.clone(), "http:127.0.0.1:1");
        let ok_session = core.create_session("范围内").unwrap();
        assert!(srv.resolve_session(&srv.acl(), &ok_session.id).is_ok(), "范围内的会话放行");

        // 组范围：建一个组外会话（default 组）→ 拒绝
        core.registry().set_active_group("default").unwrap();
        let out_session = core.create_session("范围外").unwrap();
        core.registry().set_active_group("t").unwrap();
        let err = srv.resolve_session(&srv.acl(), &out_session.id);
        assert!(err.is_err(), "组范围外会话应拒绝");
        assert!(err.unwrap_err().to_string().contains("授权范围"));

        // 会话范围清单：未列入 → 拒绝
        let (core2, dir2) = server_with(|cfg| {
            cfg.mcp_serve.allowed_sessions = vec!["specific".into()];
        }).await;
        let srv2 = McpServer::new(core2.clone(), "stdio");
        let other = core2.create_session("未列入").unwrap();
        assert!(srv2.resolve_session(&srv2.acl(), &other.id).is_err(), "会话范围外拒绝");

        // 默认关闭：handle_request 拒绝
        let (core3, dir3) = server_with(|cfg| cfg.mcp_serve.enabled = false).await;
        let srv3 = McpServer::new(core3.clone(), "stdio");
        let denied = rpc(&srv3, "tools/list", json!({})).await;
        assert!(denied["error"]["code"] == -32000, "{}", denied);
        assert!(srv3.catalog().is_empty(), "未启用时清单为空");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
        let _ = std::fs::remove_dir_all(&dir3);
    }

    /// 外部调用安全口径（10.4）：外部触发的受限命令 → 审批卡出现 → 未批准前返回错误
    /// （终端白名单只放行单一安全命令，受限口径以 exec_approval=always 覆盖全部命令，
    ///   与 smoke.rs 的审批闸门用例同口径）
    #[tokio::test]
    async fn 安全口径_受限命令触发审批并返回错误() {
        let (core, dir) = server_with(|cfg| {
            cfg.security.exec_approval = "always".into();
            cfg.security.approval_wait_secs = 0; // 旧语义：拦截即受阻回流（不等待）
        }).await;
        let s = core.create_session("审批口径").unwrap();
        let srv = McpServer::new(core.clone(), "http:10.0.0.9");
        let resp = rpc(
            &srv,
            "tools/call",
            json!({ "name": "terminal", "arguments": { "session": s.id, "command": "echo blocked-mcp" } }),
        )
        .await;
        assert_eq!(resp["result"]["isError"], true, "审批拦截应返回工具级错误: {resp}");
        assert!(
            resp["result"]["content"][0]["text"].as_str().unwrap().contains("待人工审批"),
            "错误信息应说明审批口径: {resp}"
        );
        // 审批卡出现（pending 审批单入存储）
        let pending = core.approval_list(Some("pending"), 10).unwrap();
        assert!(pending.iter().any(|a| a.command.contains("blocked-mcp")), "审批请求应已登记: {pending:?}");
        // 审计来源可归因：mcp/http:10.0.0.9
        let audit = core.store.list_tool_audit("mcp/http:10.0.0.9", 10).unwrap();
        assert!(audit.iter().any(|a| a["tool"] == "terminal"), "审计应标注来源与客户端身份");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
