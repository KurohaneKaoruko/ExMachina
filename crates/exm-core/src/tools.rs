//! 工具网关 —— docs/01 §2.6：白名单 + 工作区沙箱 + 审计

use crate::config::SecurityConfig;
use crate::registry::LocalRegistry;
use crate::store::Store;
use crate::types::{normalize_domain, AgentDefinition, ApprovalRequest, CoreEvent, ToolName, Tier};
use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub struct ToolResult {
    pub ok: bool,
    pub output: String,
    pub error: Option<String>,
}

impl ToolResult {
    fn ok(output: impl Into<String>) -> Self {
        ToolResult { ok: true, output: output.into(), error: None }
    }
    fn err(msg: impl Into<String>) -> Self {
        ToolResult { ok: false, output: String::new(), error: Some(msg.into()) }
    }
}

/// 终端命令白名单（前缀匹配）
const TERMINAL_ALLOW: &[&str] = &[
    "dir", "ls", "type", "echo", "node", "npm", "git status", "git log", "git diff", "findstr",
    "cargo", "rustc",
];

/// 高危命令特征（审批闸门 risky 模式匹配子串）
const DESTRUCTIVE_PATTERNS: &[&str] = &[
    "rm ", "rm -", "del ", "rmdir", "rd /s", "format ", "shutdown", "taskkill", "reg delete",
    "reg add", "dd if=", "mkfs", "git push --force", "git push -f", "git reset --hard",
    "git clean", "git rebase", "drop table", "drop database", "truncate table", "remove-item",
    "invoke-expression", "curl | sh", "curl | bash",
];

pub fn is_destructive_command(lowered: &str) -> bool {
    DESTRUCTIVE_PATTERNS.iter().any(|p| lowered.contains(p))
}

/// 跨平台 shell 执行（工具闸门与审批代执行共用）
/// 极简 HTML 正文提取：剥 script/style 块与标签、压缩空白（零正则依赖）
fn strip_html(html: &str) -> String {
    let lower = html.to_lowercase();
    let mut out = String::with_capacity(html.len() / 2);
    let bytes = html.as_bytes();
    let mut i = 0usize;
    let mut skip_until: Option<&str> = None;
    while i < bytes.len() {
        if let Some(end) = skip_until {
            // 在被跳过的块内寻找结束标记
            if lower[i..].starts_with(end) {
                skip_until = None;
                i += end.len();
                // 吞掉闭合标签的剩余部分
                while i < bytes.len() && html[i..].chars().next().map(|c| c != '>').unwrap_or(false) {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
            } else {
                i += 1;
            }
            continue;
        }
        if lower[i..].starts_with("<script") {
            skip_until = Some("</script");
        } else if lower[i..].starts_with("<style") {
            skip_until = Some("</style");
        } else if bytes[i] == b'<' {
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
        } else {
            let ch = html[i..].chars().next().unwrap_or(' ');
            out.push(if ch.is_whitespace() { ' ' } else { ch });
            i += ch.len_utf8();
        }
    }
    let mut compact = String::with_capacity(out.len());
    let mut last_space = true;
    for c in out.chars() {
        if c == ' ' {
            if !last_space {
                compact.push(' ');
            }
            last_space = true;
        } else {
            compact.push(c);
            last_space = false;
        }
    }
    compact
}

/// 异步执行终端命令：超时强杀（kill_on_drop），不阻塞运行时线程
pub async fn execute_shell_command_timed(workspace_root: &Path, cmd: &str, timeout_secs: u64) -> ToolResult {
    #[cfg(target_os = "windows")]
    let mut process = {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut process = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    let spawned = process
        .current_dir(workspace_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return ToolResult::err(format!("命令执行失败: {e}")),
    };
    let pid = child.id();
    // 管道读取放入独立任务：超时路径可 abort，避免残留读端拖住运行时收尾
    let out_task = child.stdout.take().map(|mut s| {
        tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut b).await;
            b
        })
    });
    let err_task = child.stderr.take().map(|mut s| {
        tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut b).await;
            b
        })
    });

    let budget = std::time::Duration::from_secs(timeout_secs.max(1));
    match tokio::time::timeout(budget, child.wait()).await {
        Err(_) => {
            // 进程树强杀：Windows taskkill /T /F（cmd 的子进程一并终止）
            #[cfg(target_os = "windows")]
            {
                if let Some(pid) = pid {
                    let _ = tokio::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .await;
                }
            }
            // 有界回收：等待被杀进程退出，最长 3s——超时路径绝不无限阻塞
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await;
            if let Some(t) = out_task {
                t.abort();
            }
            if let Some(t) = err_task {
                t.abort();
            }
            ToolResult::err(format!("命令超时（{timeout_secs}s）已终止"))
        }
        Ok(Err(e)) => ToolResult::err(format!("命令执行失败: {e}")),
        Ok(Ok(status)) => {
            let stdout = match out_task {
                Some(t) => t.await.unwrap_or_default(),
                None => Vec::new(),
            };
            let stderr = match err_task {
                Some(t) => t.await.unwrap_or_default(),
                None => Vec::new(),
            };
            let text = String::from_utf8_lossy(&stdout).to_string();
            let err = String::from_utf8_lossy(&stderr).to_string();
            let mut text = text.chars().take(8000).collect::<String>();
            if !err.trim().is_empty() {
                text.push_str("\n[stderr] ");
                text.push_str(&err.chars().take(2000).collect::<String>());
            }
            ToolResult { ok: status.success(), output: text, error: None }
        }
    }
}

pub struct ToolGateway {
    workspace_root: PathBuf,
    store: Arc<Store>,
    registry: Arc<LocalRegistry>,
    events: tokio::sync::broadcast::Sender<CoreEvent>,
    security: SecurityConfig,
    web_search: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
    /// MCP 服务器池（第三方工具：mcp:server:tool 全名空间）
    mcp: Arc<crate::mcp::McpRegistry>,
}

impl ToolGateway {
    pub fn new(
        workspace_root: impl AsRef<Path>,
        store: Arc<Store>,
        registry: Arc<LocalRegistry>,
        events: tokio::sync::broadcast::Sender<CoreEvent>,
        security: SecurityConfig,
    ) -> Self {
        ToolGateway {
            workspace_root: workspace_root.as_ref().to_path_buf(),
            store,
            registry,
            events,
            security,
            web_search: None,
            mcp: crate::mcp::McpRegistry::shared(),
        }
    }

    /// 注入 MCP 服务器池（build_orchestrator 配置后传入同一实例）
    pub fn with_mcp(mut self, mcp: Arc<crate::mcp::McpRegistry>) -> Self {
        self.mcp = mcp;
        self
    }

    pub fn mcp(&self) -> Arc<crate::mcp::McpRegistry> {
        self.mcp.clone()
    }

    /// 执行 MCP 工具（全名 mcp:server:tool）；审计与内置工具一致落库
    pub async fn execute_mcp(&self, agent_id: &str, full_name: &str, args: &serde_json::Value) -> ToolResult {
        let started = Instant::now();
        let result = match self.mcp.call(full_name, agent_id, args).await {
            Some(r) => ToolResult {
                ok: r.ok,
                output: r.text.chars().take(8000).collect(),
                error: if r.ok { None } else { Some(r.text.chars().take(2000).collect()) },
            },
            None => ToolResult::err(format!("MCP 工具不存在或未开放: {full_name}")),
        };
        let summary: String = if result.ok {
            result.output.chars().take(200).collect()
        } else {
            result.error.clone().unwrap_or_default().chars().take(200).collect()
        };
        let _ = self.store.audit_tool(agent_id, full_name, args, &summary, started.elapsed().as_millis() as u64);
        result
    }

    pub fn with_web_search(
        mut self,
        f: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> Self {
        self.web_search = Some(f);
        self
    }

    /// 生效工作区根：激活组声明了 workspace 时以其为根（相对路径相对全局根），否则全局根
    fn effective_root(&self) -> PathBuf {
        let global = self.workspace_root.clone();
        let ws = self
            .registry
            .active_group_meta()
            .and_then(|m| m.workspace)
            .filter(|s| !s.trim().is_empty());
        match ws {
            Some(rel) => {
                let p = PathBuf::from(rel.trim());
                if p.is_absolute() {
                    p
                } else {
                    global.join(p)
                }
            }
            None => global,
        }
    }

    /// 原生 function calling：按白名单生成工具 schema（docs/协议与契约.md）
    pub fn tool_specs(allowlist: &[ToolName]) -> Vec<crate::provider::ToolSpec> {
        let mut specs: Vec<crate::provider::ToolSpec> = Vec::new();
        let mut push = |name: ToolName, description: &str, parameters: serde_json::Value| {
            if allowlist.contains(&name) {
                specs.push(crate::provider::ToolSpec { name: name.key().to_string(), description: description.to_string(), parameters });
            }
        };
        push(ToolName::Read, "读取工作区内文件或目录内容", serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "相对工作区的文件或目录路径" },
                "maxChars": { "type": "integer", "description": "最多返回字符数，默认 8000" }
            },
            "required": ["path"]
        }));
        push(ToolName::Filesystem, "文件系统操作：list/write/mkdir/delete", serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["list", "write", "mkdir", "delete"] },
                "path": { "type": "string", "description": "相对工作区的路径" },
                "content": { "type": "string", "description": "op=write 时的文件内容" }
            },
            "required": ["op", "path"]
        }));
        push(ToolName::Terminal, "在工作区内执行终端命令（受审批闸门与白名单约束）", serde_json::json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"]
        }));
        push(ToolName::WebSearch, "联网搜索并返回结果摘要", serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }));
        push(ToolName::WebFetch, "抓取网页 URL 并提取正文文本（自动剥离标签与脚本）", serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "http(s) 地址" },
                "maxChars": { "type": "integer", "description": "最多返回字符数，默认 8000" }
            },
            "required": ["url"]
        }));
        push(ToolName::AgentManage, "组内个体管理（仅主智能体）：create/update/remove/setPrimary", serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["create", "update", "remove"] },
                "identifier": { "type": "string" },
                "name": { "type": "string" },
                "description": { "type": "string" },
                "domain": { "type": "string" },
                "tier": { "type": "string", "enum": ["unit", "orchestrator"] },
                "prompt": { "type": "string" },
                "capabilities": { "type": "array", "items": { "type": "string" } },
                "setPrimary": { "type": "boolean" }
            },
            "required": ["op"]
        }));
        specs
    }

    /// 执行工具：白名单外拒绝，执行后写审计
    pub async fn execute(
        &self,
        agent_id: &str,
        allowlist: &[ToolName],
        tool: ToolName,
        args: &serde_json::Value,
    ) -> ToolResult {
        let started = Instant::now();
        let result = if !allowlist.contains(&tool) {
            ToolResult::err(format!("工具 {} 不在本节点放行白名单内", tool.key()))
        } else if tool == ToolName::AgentManage
            && self
                .registry
                .active_group_meta()
                .and_then(|m| m.primary)
                .as_deref()
                != Some(agent_id)
        {
            // 纵深防御：allowlist 注入之外再校验调用者身份
            ToolResult::err("agent_manage 仅限激活组主智能体使用")
        } else {
            self.dispatch(agent_id, tool, args).await
        };
        let summary = if result.ok {
            result.output.chars().take(200).collect::<String>()
        } else {
            result.error.clone().unwrap_or_default().chars().take(200).collect()
        };
        let _ = self.store.audit_tool(agent_id, tool.key(), args, &summary, started.elapsed().as_millis() as u64);
        result
    }

    async fn dispatch(&self, agent_id: &str, tool: ToolName, args: &serde_json::Value) -> ToolResult {
        match tool {
            ToolName::Read => self.tool_read(args),
            ToolName::Filesystem => self.tool_fs(args),
            ToolName::Terminal => self.tool_terminal(agent_id, args).await,
            ToolName::WebSearch => self.tool_web_search(args),
            ToolName::WebFetch => self.tool_web_fetch(args).await,
            ToolName::AgentManage => self.tool_agent_manage(args),
        }
    }

    /// 智能体管理（docs/09）：仅激活组的主智能体可用；内置组定义受保护。
    /// 审计由 execute() 统一落库。
    fn tool_agent_manage(&self, args: &serde_json::Value) -> ToolResult {
        // 权限闸门：调用者必须是激活组的主智能体，且当前组非内置
        let Some(meta) = self.registry.active_group_meta() else {
            return ToolResult::err("无激活组");
        };
        if meta.builtin {
            return ToolResult::err("内置组的定义受保护：不可增删改个体");
        }
        // 调用者身份由网关在 args 上层校验（execute 时传入 agent_id），
        // 这里校验 op 与内容；调用者==主智能体的强校验在 execute() 的 allowlist 注入处保证。

        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("");
        match self.agent_manage_op(&meta, op, args) {
            Ok(v) => ToolResult::ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn agent_manage_op(
        &self,
        meta: &crate::types::GroupMeta,
        op: &str,
        args: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        match op {
            "list" => {
                let defs = self.registry.list();
                Ok(serde_json::json!({
                    "group": meta,
                    "agents": defs.iter().map(|d| serde_json::json!({
                        "name": d.name, "identifier": d.identifier,
                        "domain": d.domain, "tier": d.tier, "description": d.description,
                    })).collect::<Vec<_>>()
                }))
            }
            "group_info" => Ok(serde_json::json!({
                "group": meta,
                "count": self.registry.count(),
                "isBuiltin": self.registry.is_builtin(&meta.id),
            })),
            "create" | "update" => {
                let identifier = s("identifier");
                let name = s("name");
                let description = s("description");
                if (op == "create" && (identifier.is_empty() || name.is_empty() || description.is_empty()))
                    || (op == "update" && identifier.is_empty())
                {
                    bail!("create 需要 identifier/name/description；update 需要 identifier");
                }
                // update：基于现有定义合并
                let mut def = match (op, self.registry.get(&identifier)) {
                    ("update", Some(existing)) => existing,
                    _ => AgentDefinition {
                        name: name.clone(),
                        identifier: identifier.clone(),
                        domain: if s("domain").is_empty() { "自定义".into() } else { s("domain") },
                        tier: Tier::Unit,
                        description: description.clone(),
                        capabilities: args
                            .get("capabilities")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                            .unwrap_or_default(),
                        tools: vec![],
                        when_to_call: s("whenToCall"),
                        dependencies: vec![],
                        composable_with: vec![],
                        input_schema: Default::default(),
                        output_schema: Default::default(),
                        prompt_file: String::new(),
                        model_hint: None,
                    },
                };
                if !name.is_empty() {
                    def.name = name;
                }
                if !description.is_empty() {
                    def.description = description;
                }
                if !s("domain").is_empty() {
                    def.domain = normalize_domain(&s("domain"));
                }
                if let Some(t) = args.get("tier").and_then(|v| v.as_str()) {
                    if t == "orchestrator" {
                        def.tier = Tier::Orchestrator;
                    }
                }
                if let Some(t) = args.get("prompt").and_then(|v| v.as_str()) {
                    def.prompt_file = format!("{}.md", def.identifier);
                    self.registry.write_prompt(&def.prompt_file, t)?;
                }
                let saved = self.registry.upsert_agent(&meta.id, def, None)?;
                // 首个个体或显式指定时设立主智能体
                if args.get("setPrimary").and_then(|v| v.as_bool()).unwrap_or(false)
                    || meta.primary.is_none()
                {
                    self.registry.set_primary(&meta.id, &saved.identifier)?;
                }
                Ok(serde_json::to_value(&saved)?)
            }
            "delete" => {
                let identifier = s("identifier");
                self.registry.remove_agent(&meta.id, &identifier)?;
                Ok(serde_json::json!({ "deleted": identifier }))
            }
            "set_primary" => {
                let identifier = s("identifier");
                self.registry.set_primary(&meta.id, &identifier)?;
                Ok(serde_json::json!({ "primary": identifier }))
            }
            other => bail!("未知 agent_manage 操作: {other}"),
        }
    }

    fn resolve_safe(&self, raw: &str) -> anyhow::Result<PathBuf> {
        let root_base = self.effective_root();
        let candidate = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            root_base.join(raw)
        };
        let root = root_base.canonicalize().unwrap_or_else(|_| root_base.clone());
        let normalized = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if !normalized.starts_with(&root) && !candidate.starts_with(&self.workspace_root) {
            bail!("路径越界: {raw}");
        }
        Ok(candidate)
    }

    fn tool_read(&self, args: &serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let max_chars = args.get("maxChars").and_then(|v| v.as_u64()).unwrap_or(8000) as usize;
        match self.resolve_safe(path) {
            Ok(p) => {
                if p.is_dir() {
                    match std::fs::read_dir(&p) {
                        Ok(entries) => {
                            let names: Vec<String> = entries
                                .filter_map(|e| e.ok())
                                .map(|e| e.file_name().to_string_lossy().to_string())
                                .take(200)
                                .collect();
                            ToolResult::ok(names.join("\n"))
                        }
                        Err(e) => ToolResult::err(format!("列目录失败: {e}")),
                    }
                } else {
                    match std::fs::read_to_string(&p) {
                        Ok(content) => ToolResult::ok(content.chars().take(max_chars).collect::<String>()),
                        Err(e) => ToolResult::err(format!("读取失败: {e}")),
                    }
                }
            }
            Err(e) => ToolResult::err(e.to_string()),
        }
    }

    fn tool_fs(&self, args: &serde_json::Value) -> ToolResult {
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("write");
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let target = match self.resolve_safe(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        match op {
            "write" => {
                let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&target, content).context("写入失败") {
                    Ok(_) => ToolResult::ok(format!("written: {}", target.display())),
                    Err(e) => ToolResult::err(e.to_string()),
                }
            }
            "mkdir" => match std::fs::create_dir_all(&target) {
                Ok(_) => ToolResult::ok(format!("mkdir: {}", target.display())),
                Err(e) => ToolResult::err(e.to_string()),
            },
            other => ToolResult::err(format!("未知 filesystem 操作: {other}")),
        }
    }

    async fn tool_terminal(&self, agent_id: &str, args: &serde_json::Value) -> ToolResult {
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let lowered = cmd.trim().to_lowercase();
        if !TERMINAL_ALLOW.iter().any(|p| lowered.starts_with(p)) {
            return ToolResult::err(format!("命令不在白名单: {cmd}"));
        }
        // 审批闸门（docs/10）：risky 拦高危命令，always 全拦；白名单前缀放行
        let mode = self.security.exec_approval.as_str();
        if mode == "risky" || mode == "always" {
            let allowlisted = self
                .security
                .exec_allowlist
                .iter()
                .any(|p| !p.trim().is_empty() && lowered.starts_with(p.trim().to_lowercase().as_str()));
            let gated = mode == "always" || is_destructive_command(&lowered);
            if gated && !allowlisted {
                return self.request_approval(agent_id, cmd);
            }
        }
        execute_shell_command_timed(&self.effective_root(), cmd, 120).await
    }

    /// 命令拦截：落审批单 + 发事件，个体回流受阻等待用户决定
    fn request_approval(&self, agent_id: &str, cmd: &str) -> ToolResult {
        let req = ApprovalRequest {
            id: crate::types::new_id()[..8].to_string(),
            session_id: String::new(),
            node_id: None,
            agent_id: agent_id.to_string(),
            command: cmd.to_string(),
            status: "pending".into(),
            result: None,
            created_at: crate::types::now_iso(),
            decided_at: None,
        };
        let _ = self.store.add_approval(&req);
        let _ = self.events.send(CoreEvent {
            kind: "approval.required".into(),
            session_id: String::new(),
            payload: serde_json::json!({
                "approvalId": req.id, "agentId": agent_id, "command": cmd,
            }),
        });
        ToolResult::err(format!(
            "命令已拦截待人工审批（审批单 {}）：{cmd}。回流受阻原因；用户批准后由系统代执行。",
            req.id
        ))
    }

    /// 抓取网页并提取正文（大小/时长受限，标签与脚本剥离）
    async fn tool_web_fetch(&self, args: &serde_json::Value) -> ToolResult {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let max_chars = args.get("maxChars").and_then(|v| v.as_u64()).unwrap_or(8000) as usize;
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ToolResult::err("url 必须以 http(s):// 开头");
        }
        let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build() {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("HTTP 客户端构建失败: {e}")),
        };
        let resp = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("抓取失败: {e}")),
        };
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return ToolResult::err(format!("HTTP {status}"));
        }
        // 截断到 2MB 以内再读
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return ToolResult::err(format!("读取响应失败: {e}")),
        };
        let raw = String::from_utf8_lossy(&bytes[..bytes.len().min(2 * 1024 * 1024)]);
        let text = strip_html(&raw);
        let text = text.trim().chars().take(max_chars).collect::<String>();
        if text.is_empty() {
            ToolResult::err("页面无可提取文本（可能是纯二进制或脚本渲染页）")
        } else {
            ToolResult::ok(text)
        }
    }

    fn tool_web_search(&self, args: &serde_json::Value) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        match &self.web_search {
            Some(f) => ToolResult::ok(f(query)),
            None => ToolResult::err(format!("搜索渠道未配置（query: {query}）")),
        }
    }
}