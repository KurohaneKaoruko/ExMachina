//! 工具网关 —— docs/01 §2.6：白名单 + 工作区沙箱 + 审计
//!
//! 工具面（对标主流 agent 的「手」）：read（行号/分页）/ edit（精确替换）/ grep / glob /
//! filesystem（write/mkdir/list）/ terminal（白名单 + 审批 + 后台长任务）/ web_search（真实后端，
//! 未配置即不下发）/ web_fetch / agent_manage（组内编成）/ schedule（自主排程）。

use crate::config::{SearchConfig, SecurityConfig, SandboxConfig};
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

/// 终端命令白名单规则：命令名 → 允许的子命令前缀（空 = 该命令整体放行）。
/// 只匹配**首命令**且拒绝 shell 连接符（`&&` `|` `;` 等），避免「白名单命令 + 追加危险命令」绕过。
struct AllowRule {
    cmd: &'static str,
    subs: &'static [&'static str],
}

const TERMINAL_ALLOW: &[AllowRule] = &[
    AllowRule { cmd: "dir", subs: &[] },
    AllowRule { cmd: "ls", subs: &[] },
    AllowRule { cmd: "type", subs: &[] },
    AllowRule { cmd: "echo", subs: &[] },
    AllowRule { cmd: "findstr", subs: &[] },
    AllowRule { cmd: "where", subs: &[] },
    AllowRule { cmd: "head", subs: &[] },
    AllowRule { cmd: "tail", subs: &[] },
    AllowRule { cmd: "wc", subs: &[] },
    AllowRule { cmd: "node", subs: &[] },
    AllowRule { cmd: "npx", subs: &[] },
    AllowRule { cmd: "npm", subs: &["run", "test", "ci", "install", "ls", "view", "why", "audit", "outdated", "pack"] },
    AllowRule { cmd: "pnpm", subs: &["run", "test", "install", "list", "ls", "why", "audit", "outdated"] },
    AllowRule { cmd: "yarn", subs: &["run", "test", "install", "list", "why", "audit", "outdated"] },
    AllowRule {
        cmd: "git",
        subs: &[
            "status", "log", "diff", "show", "branch", "fetch", "add", "commit", "checkout",
            "switch", "restore", "stash", "rev-parse", "ls-files", "blame", "describe", "tag",
            "remote", "config", "apply", "merge-base", "shortlog", "grep", "worktree", "init",
        ],
    },
    AllowRule {
        cmd: "cargo",
        subs: &[
            "build", "check", "test", "run", "bench", "fmt", "clippy", "doc", "metadata", "tree",
            "nextest", "update", "add", "remove", "fetch", "verify-project", "locate-project",
            "pkgid", "search", "info", "clean",
        ],
    },
    AllowRule { cmd: "rustc", subs: &[] },
    AllowRule { cmd: "rustfmt", subs: &[] },
    AllowRule { cmd: "python", subs: &[] },
    AllowRule { cmd: "python3", subs: &[] },
    AllowRule { cmd: "pip", subs: &["list", "show", "freeze", "download"] },
];

/// shell 连接符：白名单只放行单一命令，带连接符一律拒绝（防「白名单命令 && 危险命令」绕过）
const SHELL_CHAIN_MARKS: &[&str] = &["&&", "||", ";", "|", "`", "$(", ">", "<", "&"];

/// 剥掉引号包裹的片段（引号内的 `>` `|` 等是普通字符，不应误判为连接符）
fn strip_quoted(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut quote: Option<char> = None;
    for ch in cmd.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                } else {
                    out.push(ch);
                }
            }
        }
    }
    out
}

fn contains_shell_chain(cmd: &str) -> bool {
    let bare = strip_quoted(cmd);
    SHELL_CHAIN_MARKS.iter().any(|m| bare.contains(m))
}

/// 白名单判定：首命令 + （有子命令约束时）子命令前缀
fn terminal_allowed(cmd: &str) -> bool {
    let lowered = cmd.trim().to_lowercase();
    let mut parts = lowered.split_whitespace();
    let Some(head_raw) = parts.next() else { return false };
    let head = head_raw.trim_end_matches(".exe");
    let Some(rule) = TERMINAL_ALLOW.iter().find(|r| r.cmd == head) else { return false };
    if rule.subs.is_empty() {
        return true;
    }
    let rest: Vec<&str> = parts.collect();
    rule.subs.iter().any(|s| {
        let want: Vec<&str> = s.split_whitespace().collect();
        rest.len() >= want.len() && want.iter().zip(rest.iter()).all(|(a, b)| a == b)
    })
}

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

/// 后台任务（长命令）：输出累积缓冲 + 终态码 + 中止句柄
pub struct BgTask {
    pub cmd: String,
    pub started: Instant,
    pub buf: Arc<parking_lot::Mutex<String>>,
    /// None = 运行中；Some(code) = 已退出
    pub state: Arc<parking_lot::Mutex<Option<i32>>>,
    pub abort: tokio::task::AbortHandle,
}

/// 执行一条钩子命令（同步、30s 上界）；返回 (是否成功, 输出摘要)。
/// 钩子在工作区内执行，环境变量注入上下文（工具名/参数/agent/工作区）。
pub fn run_hook_command(hook: &str, cwd: &Path, envs: &[(&str, &str)]) -> (bool, String) {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", hook]);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", hook]);
        c
    };
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.current_dir(cwd).stdin(std::process::Stdio::null());
    let out = cmd.output();
    match out {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            let trimmed: String = text.trim().chars().take(600).collect();
            (o.status.success(), trimmed)
        }
        Err(e) => (false, format!("钩子执行失败: {e}")),
    }
}

/// 沙箱执行参数：程序 + 参数 + 环境（净化后）
pub struct ShellOpts {
    pub program: String,
    pub args: Vec<String>,
    /// true = 清空继承环境后只注入 envs
    pub env_clear: bool,
    pub envs: Vec<(String, String)>,
    /// 内存上限 MB（0 = 不限；Linux 经 ulimit、Windows 经作业对象生效）
    pub memory_mb: usize,
    /// 进程数上限（0 = 不限；仅 Windows 作业对象支持）
    pub max_processes: u32,
    /// 覆盖实际执行的命令文本（如加 ulimit 前缀）；None = 用调用方传入的 cmd
    pub cmd_override: Option<String>,
}

impl ShellOpts {
    /// 不加固（历史行为：直接 shell 执行，继承环境）
    pub fn plain() -> Self {
        #[cfg(target_os = "windows")]
        {
            ShellOpts {
                program: "cmd".into(),
                args: vec![],
                env_clear: false,
                envs: vec![],
                memory_mb: 0,
                max_processes: 0,
                cmd_override: None,
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            ShellOpts {
                program: "sh".into(),
                args: vec![],
                env_clear: false,
                envs: vec![],
                memory_mb: 0,
                max_processes: 0,
                cmd_override: None,
            }
        }
    }
}

/// 联网类命令特征（strict 模式下需要审批）
const NETWORK_COMMAND_MARKS: &[&str] = &[
    "curl", "wget", "pip install", "pip3 install", "npm install", "npm ci", "pnpm install",
    "yarn add", "cargo install", "cargo add", "apt ", "apt-get", "dnf ", "yum ", "brew ",
    "git clone", "git fetch", "git pull", "docker ", "ssh ", "scp ", "invoke-webrequest",
];

pub fn is_network_command(lowered: &str) -> bool {
    NETWORK_COMMAND_MARKS.iter().any(|m| lowered.contains(m))
}

/// 子进程环境白名单：只透传运行必需项，敏感变量（各类 KEY/TOKEN/SECRET）一律不外泄
const ENV_ALLOW: &[&str] = &[
    "PATH", "PATHEXT", "SystemRoot", "SystemDrive", "COMSPEC", "WINDIR", "LANG", "LC_ALL", "TERM",
    "OS", "ProgramData", "ProgramFiles", "ProgramFiles(x86)", "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE", "CARGO_HOME", "RUSTUP_HOME", "USER", "USERNAME", "SHELL",
];

#[cfg(not(target_os = "windows"))]
fn bwrap_available() -> bool {
    std::process::Command::new("bwrap")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------- Windows 作业对象

/// Windows 作业对象（Job Object）：把子进程收进作业，由内核强制
/// **内存上限 / 活动进程数上限**，并一律带 `KILL_ON_JOB_CLOSE` ——
/// 句柄一关（含任务被 abort 时的 Drop），作业内整棵进程树随之终止。
/// 这是 `taskkill` 之外的最后一道保险：agent 跑飞的长命令不会再拖着机器。
///
/// 非 Windows 平台是空类型（Linux 侧由 ulimit + bwrap 负责）。
pub struct WinJob {
    #[cfg(target_os = "windows")]
    handle: *mut core::ffi::c_void,
}

// 内核句柄本质是不透明整数；CloseHandle 可从任意线程调用。
// 不声明 Send 的话，任何跨 await 持有作业句柄的 future 都会变成 !Send（编译期报错）。
unsafe impl Send for WinJob {}
unsafe impl Sync for WinJob {}

#[cfg(target_os = "windows")]
impl WinJob {
    /// 建作业 → 设限 → 把子进程收进去；任一步失败返回 None（调用方继续按无作业执行）
    pub fn attach(process: *mut core::ffi::c_void, memory_mb: usize, max_processes: u32) -> Option<WinJob> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            let mut flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if memory_mb > 0 {
                let bytes = memory_mb * 1024 * 1024;
                flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_JOB_MEMORY;
                info.ProcessMemoryLimit = bytes;
                info.JobMemoryLimit = bytes;
            }
            if max_processes > 0 {
                flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                info.BasicLimitInformation.ActiveProcessLimit = max_processes;
            }
            info.BasicLimitInformation.LimitFlags = flags;
            let sized = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                sized,
            );
            if set == 0 {
                CloseHandle(job);
                return None;
            }
            // 子进程可能已在别的作业里（嵌套作业需 Windows 8+ 且外层允许）——失败即降级
            if AssignProcessToJobObject(job, process) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(WinJob { handle: job })
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl WinJob {
    pub fn attach(_process: *mut core::ffi::c_void, _memory_mb: usize, _max_processes: u32) -> Option<WinJob> {
        None
    }
}

#[cfg(target_os = "windows")]
impl Drop for WinJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        if !self.handle.is_null() {
            // KILL_ON_JOB_CLOSE：关句柄即终止作业内所有进程
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// 取子进程的原生句柄（Windows 传作业对象用；其他平台恒 None）
fn child_raw_handle(child: &tokio::process::Child) -> *mut core::ffi::c_void {
    #[cfg(target_os = "windows")]
    {
        // tokio::process::Child::raw_handle 是固有方法（无需引入 AsRawHandle）
        child.raw_handle().unwrap_or(std::ptr::null_mut())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = child;
        std::ptr::null_mut()
    }
}

/// 把子进程收进带资源上限的作业（Windows）；其他平台 / 句柄不可得时静默跳过
fn attach_job(child: &tokio::process::Child, opts: &ShellOpts) -> Option<WinJob> {
    let handle = child_raw_handle(child);
    if handle.is_null() {
        return None;
    }
    WinJob::attach(handle, opts.memory_mb, opts.max_processes)
}

/// 作业对象是否可用（沙箱加固自检 / 测试用）：建一个空作业即关闭
pub fn windows_job_available() -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let mut c = Command::new("cmd");
        c.args(["/C", "exit"]);
        let mut child = match c
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        let handle = {
            use std::os::windows::io::AsRawHandle;
            child.as_raw_handle() as *mut core::ffi::c_void
        };
        let ok = !handle.is_null() && WinJob::attach(handle, 256, 0).is_some();
        let _ = child.kill();
        let _ = child.wait();
        ok
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// 跨平台 shell 执行（工具闸门与审批代执行共用）。
/// 内部走 [`execute_shell_with`]（无加固）——需要沙箱时由调用方给出 [`ShellOpts`]。
pub async fn execute_shell_command_timed(workspace_root: &Path, cmd: &str, timeout_secs: u64) -> ToolResult {
    execute_shell_with(workspace_root, cmd, timeout_secs, &ShellOpts::plain()).await
}

/// 按 ShellOpts 执行：可换程序（bwrap）、清环境、限内存
pub async fn execute_shell_with(
    workspace_root: &Path,
    cmd: &str,
    timeout_secs: u64,
    opts: &ShellOpts,
) -> ToolResult {
    let mut process = tokio::process::Command::new(&opts.program);
    let effective: String = opts.cmd_override.clone().unwrap_or_else(|| cmd.to_string());
    if opts.args.is_empty() {
        // 默认：走 shell -c（bwrap 时 args 已含完整命令行）
        #[cfg(target_os = "windows")]
        process.args(["/C", effective.as_str()]);
        #[cfg(not(target_os = "windows"))]
        process.args(["-c", effective.as_str()]);
    } else {
        process.args(&opts.args);
    }
    if opts.env_clear {
        process.env_clear();
    }
    for (k, v) in &opts.envs {
        process.env(k, v);
    }
    if opts.memory_mb > 0 {
        let _ = process.env("EXM_MEM_LIMIT_MB", opts.memory_mb.to_string());
    }
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
    // `pid` 只在 Windows 路径下作为 taskkill 的 /PID 入参使用；
    // POSIX 上 tokio 已通过 spawn 时的 kill_on_drop(true) 自动 SIGKILL 子进程。
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    let pid = child.id();
    // Windows：收进作业对象（内存/进程数上限 + 句柄关闭即整树终止）。
    // 句柄活到函数末尾，Drop 即关 —— 命令结束后不留游离子进程。
    let _job = attach_job(&child, opts);
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

pub struct ToolGateway {
    workspace_root: PathBuf,
    store: Arc<Store>,
    registry: Arc<LocalRegistry>,
    events: tokio::sync::broadcast::Sender<CoreEvent>,
    security: SecurityConfig,
    /// 联网搜索后端（未配置 = web_search 不下发，避免「承诺不存在的能力」）
    search: SearchConfig,
    /// 沙箱执行策略（环境净化 / bwrap / strict 闸门）
    sandbox: SandboxConfig,
    /// 生命周期钩子（pre/post tool、run 收束）
    hooks: crate::config::HooksConfig,
    /// 定时任务存储（schedule 工具：AI 自主排程）
    cron: Option<Arc<crate::cron::CronStore>>,
    /// 后台任务注册表（长命令：启动后返回 id，可轮询/终止）
    background: Arc<parking_lot::Mutex<std::collections::HashMap<String, BgTask>>>,
    /// 浏览器配置与会话（懒启动常驻；串行化访问）
    browser_cfg: crate::config::BrowserConfig,
    browser: Arc<tokio::sync::Mutex<Option<crate::browser::BrowserSession>>>,
    /// MCP 服务器池（第三方工具：mcp:server:tool 全名空间）
    mcp: Arc<crate::mcp::McpRegistry>,
    /// 声明式自定义工具（配置驱动，免写 MCP 服务器；按 agents 可见性放行）
    custom: Vec<crate::config::CustomTool>,
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
            search: SearchConfig::default(),
            sandbox: SandboxConfig::default(),
            hooks: crate::config::HooksConfig::default(),
            cron: None,
            background: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            browser_cfg: crate::config::BrowserConfig::default(),
            browser: Arc::new(tokio::sync::Mutex::new(None)),
            mcp: crate::mcp::McpRegistry::shared(),
            custom: Vec::new(),
        }
    }

    /// 注入联网搜索后端配置（未配置时 web_search 不下发给模型）
    pub fn with_search(mut self, search: SearchConfig) -> Self {
        self.search = search;
        self
    }

    /// 注入声明式自定义工具（配置驱动）
    pub fn with_custom_tools(mut self, tools: Vec<crate::config::CustomTool>) -> Self {
        self.custom = tools;
        self
    }

    /// 该个体可见的自定义工具 schema（与内置工具一起随 function calling 下发）
    pub fn custom_specs_for(&self, agent_id: &str) -> Vec<crate::provider::ToolSpec> {
        self.custom
            .iter()
            .filter(|c| c.visible_to(agent_id))
            .map(|c| crate::provider::ToolSpec {
                name: c.name.trim().to_string(),
                description: if c.description.trim().is_empty() {
                    format!("自定义工具 {}（{}）", c.name, c.kind_norm())
                } else {
                    c.description.clone()
                },
                parameters: c.param_schema(),
            })
            .collect()
    }

    /// 自定义工具是否已声明（诊断 / doctor 用）
    pub fn has_custom_tool(&self, name: &str) -> bool {
        self.custom.iter().any(|c| c.name == name)
    }

    /// 注入生命周期钩子配置
    pub fn with_hooks(mut self, hooks: crate::config::HooksConfig) -> Self {
        self.hooks = hooks;
        self
    }

    /// 注入沙箱执行策略
    pub fn with_sandbox(mut self, sandbox: SandboxConfig) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// 注入浏览器配置（browser 工具）
    pub fn with_browser(mut self, browser: crate::config::BrowserConfig) -> Self {
        self.browser_cfg = browser;
        self
    }

    /// 浏览器是否可用（探测可执行文件；不可用时不下发 browser 工具，避免「承诺不存在的能力」）
    pub fn browser_ready(&self) -> bool {
        crate::browser::find_browser(&self.browser_cfg.executable).is_some()
    }

    /// 子进程环境（净化后）：白名单透传 + HOME/TMP 重定向到工作区内沙箱目录。
    /// 敏感变量（各类 KEY / TOKEN / SECRET）一律不外泄给工具子进程。
    fn sandbox_env(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for k in ENV_ALLOW {
            if let Ok(v) = std::env::var(k) {
                out.push((k.to_string(), v));
            }
        }
        let base = self.workspace_root.join(".exmachina").join("sandbox");
        let home = base.join("home");
        let tmp = base.join("tmp");
        let _ = std::fs::create_dir_all(&home);
        let _ = std::fs::create_dir_all(&tmp);
        out.push(("HOME".into(), home.display().to_string()));
        out.push(("USERPROFILE".into(), home.display().to_string()));
        out.push(("TMP".into(), tmp.display().to_string()));
        out.push(("TEMP".into(), tmp.display().to_string()));
        out.push(("EXM_SANDBOX".into(), self.sandbox.mode.clone()));
        out
    }

    /// 构造终端执行计划：按沙箱级别决定是否清环境、是否用 bubblewrap 做系统级隔离
    #[cfg_attr(target_os = "windows", allow(unused_variables))]
    fn shell_opts(&self, cmd: &str) -> ShellOpts {
        if !self.sandbox.enabled() {
            return ShellOpts::plain();
        }
        let root = self.effective_root();
        let envs = self.sandbox_env();
        let memory = self.sandbox.memory_mb;
        let max_procs = self.sandbox.max_processes as u32;
        #[cfg(not(target_os = "windows"))]
        {
            if self.sandbox.use_bwrap && bwrap_available() {
                let root_s = root.display().to_string();
                let mut args: Vec<String> = vec![
                    "--ro-bind".into(), "/".into(), "/".into(),
                    "--bind".into(), root_s.clone(), root_s.clone(),
                    "--dev".into(), "/dev".into(),
                    "--proc".into(), "/proc".into(),
                    "--tmpfs".into(), "/tmp".into(),
                    "--die-with-parent".into(),
                ];
                if !self.sandbox.allow_network {
                    args.push("--unshare-net".into());
                }
                args.push("--".into());
                args.push("sh".into());
                args.push("-c".into());
                let lim = if memory > 0 { format!("ulimit -v {} 2>/dev/null; ", memory * 1024) } else { String::new() };
                args.push(format!("{lim}{cmd}"));
                return ShellOpts { program: "bwrap".into(), args, env_clear: true, envs, memory_mb: memory, max_processes: max_procs, cmd_override: None };
            }
        }
        // 无 bwrap（或 Windows）：净化环境 + （Linux 下）ulimit 限内存
        #[cfg(target_os = "windows")]
        let program = "cmd";
        #[cfg(not(target_os = "windows"))]
        let program = "sh";
        #[cfg(not(target_os = "windows"))]
        let cmd_override = if memory > 0 {
            Some(format!("ulimit -v {} 2>/dev/null; {cmd}", memory * 1024))
        } else {
            None
        };
        #[cfg(target_os = "windows")]
        let cmd_override = None;
        ShellOpts {
            program: program.into(),
            args: Vec::new(),
            env_clear: true,
            envs,
            memory_mb: memory,
            max_processes: max_procs,
            cmd_override,
        }
    }

    /// 注入定时任务存储（schedule 工具）
    pub fn with_cron(mut self, cron: Arc<crate::cron::CronStore>) -> Self {
        self.cron = Some(cron);
        self
    }

    /// 搜索后端是否就绪（决定 web_search 是否出现在下发给模型的工具表里）
    pub fn search_ready(&self) -> bool {
        self.search.ready()
    }

    /// 记录一次 LLM 调用的真实用量（provider 返回的 usage；session 空 = 归入 unattributed）
    pub fn record_usage(&self, session_id: &str, prompt_tokens: u64, completion_tokens: u64, model: &str) {
        let scope = if session_id.trim().is_empty() { "unattributed" } else { session_id.trim() };
        let _ = self.store.add_usage(scope, prompt_tokens, completion_tokens, model);
    }

    /// 运行收束钩子（onRunEnd）：由指挥体在 run 结束时调用
    pub fn run_end_hooks(&self, status: &str, session_id: &str, agent_id: &str) -> Vec<String> {
        if self.hooks.on_run_end.is_empty() {
            return Vec::new();
        }
        let hooks = self.hooks.on_run_end.clone();
        let mut notes = Vec::new();
        for h in hooks {
            let (ok, out) = run_hook_command(
                &h,
                &self.effective_root(),
                &[
                    ("EXM_HOOK", "run_end"),
                    ("EXM_RUN_STATUS", status),
                    ("EXM_SESSION", session_id),
                    ("EXM_AGENT", agent_id),
                ],
            );
            if !ok {
                notes.push(format!("钩子失败（{h}）：{out}"));
            }
        }
        notes
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

    /// 原生 function calling：按白名单生成工具 schema（docs/协议与契约.md）。
    /// `search_ready` / `browser_ready` 决定联网搜索与浏览器工具是否下发——
    /// 后端能力未就绪时不承诺该工具（避免模型调用必然失败的工具）。
    pub fn tool_specs(
        allowlist: &[ToolName],
        search_ready: bool,
        browser_ready: bool,
    ) -> Vec<crate::provider::ToolSpec> {
        let mut specs: Vec<crate::provider::ToolSpec> = Vec::new();
        let mut push = |name: ToolName, description: &str, parameters: serde_json::Value| {
            if !allowlist.contains(&name) {
                return;
            }
            if name == ToolName::WebSearch && !search_ready {
                return;
            }
            if name == ToolName::Browser && !browser_ready {
                return; // 未探测到浏览器：不下发
            }
            specs.push(crate::provider::ToolSpec { name: name.key().to_string(), description: description.to_string(), parameters });
        };
        push(ToolName::Read, "读取工作区内文件或目录。文件按行返回并带行号，可用 offset/limit 分页续读；目录返回条目清单", serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "相对工作区的文件或目录路径" },
                "offset": { "type": "integer", "description": "起始行号（1 起，默认 1）" },
                "limit": { "type": "integer", "description": "最多返回行数（默认 400）" },
                "maxChars": { "type": "integer", "description": "返回字符上限（默认 24000）" }
            },
            "required": ["path"]
        }));
        push(ToolName::Edit, "精确编辑文件：把 old_string 替换为 new_string（old_string 必须唯一命中；整段替换请用 filesystem write）", serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "相对工作区的文件路径" },
                "old_string": { "type": "string", "description": "被替换的原文（含缩进与换行，必须与文件内容完全一致且唯一）" },
                "new_string": { "type": "string", "description": "替换后的内容；空串 = 删除该段" },
                "replace_all": { "type": "boolean", "description": "true = 替换全部命中（默认 false，多命中会报错并列出位置）" }
            },
            "required": ["path", "old_string", "new_string"]
        }));
        push(ToolName::Grep, "在工作区内按正则搜索文件内容（尊重 .gitignore），返回 文件:行号:内容", serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "正则表达式（Rust regex 语法）" },
                "path": { "type": "string", "description": "搜索起始路径（默认工作区根）" },
                "glob": { "type": "string", "description": "文件过滤，如 **/*.rs" },
                "ignoreCase": { "type": "boolean", "description": "忽略大小写（默认 false）" },
                "maxResults": { "type": "integer", "description": "最多返回条数（默认 100，上限 500）" }
            },
            "required": ["pattern"]
        }));
        push(ToolName::Glob, "按 glob 模式列出工作区内的文件路径（尊重 .gitignore），用于定位文件", serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "glob 模式，如 src/**/*.ts 或 **/*.md" },
                "path": { "type": "string", "description": "起始目录（默认工作区根）" }
            },
            "required": ["pattern"]
        }));
        push(ToolName::Filesystem, "文件系统操作：write（整文件写入/新建）、mkdir、list（条目清单）", serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["write", "mkdir", "list"] },
                "path": { "type": "string", "description": "相对工作区的路径" },
                "content": { "type": "string", "description": "op=write 时的文件内容" }
            },
            "required": ["op", "path"]
        }));
        push(ToolName::Terminal, "在工作区内执行终端命令（白名单 + 审批闸门）。长任务用 background=true 启动后轮询 taskId", serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "op=run 时必填；单一命令，不支持 && | ; 等连接符" },
                "op": { "type": "string", "enum": ["run", "poll", "kill"], "description": "默认 run" },
                "taskId": { "type": "string", "description": "op=poll/kill 时的后台任务 id" },
                "background": { "type": "boolean", "description": "true = 后台执行并立即返回任务 id（适合构建/测试等长任务）" },
                "timeoutSecs": { "type": "integer", "description": "前台命令超时（秒），上限由安全设置决定" }
            },
            "required": []
        }));
        push(ToolName::WebSearch, "联网搜索并返回结果摘要（标题 + 链接 + 摘要）", serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "maxResults": { "type": "integer", "description": "返回条数（默认取配置值）" }
            },
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
        push(ToolName::Schedule, "自主排程：创建/查看/删除定时或一次性任务（到期由调度器唤醒本组执行）。用于「每天巡检」「明天提醒跟进」这类自驱工作", serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["create", "list", "remove", "enable", "disable"] },
                "id": { "type": "string", "description": "op=list 时可过滤；remove/enable/disable 必填" },
                "name": { "type": "string", "description": "任务名（create 必填）" },
                "prompt": { "type": "string", "description": "到期时注入的提示词（create 必填）" },
                "cron": { "type": "string", "description": "五段 cron：分 时 日 月 周，如 0 9 * * *（与 at 二选一）" },
                "at": { "type": "string", "description": "一次性触发时间（ISO8601，如 2026-09-20T18:00:00+08:00）" }
            },
            "required": ["op"]
        }));
        push(ToolName::Browser, "浏览器自动化（headless Chrome + CDP）：导航网页、取渲染后正文、执行 JS、截图。适合抓动态页面 / 走一遍网页流程 / 留存页面证据", serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["open", "text", "eval", "screenshot", "close"], "description": "默认 open" },
                "url": { "type": "string", "description": "op=open：http(s):// 或 file:// 地址" },
                "expression": { "type": "string", "description": "op=eval：页面内执行的 JS 表达式（可点击/填表：document.querySelector('#x').click()）" },
                "name": { "type": "string", "description": "op=screenshot：截图文件名（不含扩展名；缺省用时间戳）" },
                "maxChars": { "type": "integer", "description": "正文返回上限（默认取配置值）" }
            },
            "required": ["op"]
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

    /// 执行内置工具（按枚举）：白名单外拒绝 → preTool 钩子（可拦截）→ 执行 → 结果落盘 → postTool 钩子 → 审计
    pub async fn execute(
        &self,
        agent_id: &str,
        allowlist: &[ToolName],
        tool: ToolName,
        args: &serde_json::Value,
    ) -> ToolResult {
        self.execute_named(agent_id, allowlist, tool.key(), args).await
    }

    /// 执行任意工具名：内置（按 `allowlist` 放行）或**声明式自定义工具**（按 `agents` 可见性放行）。
    /// 模型给出的工具名统一走这里，钩子 / 结果落盘 / 审计对两类工具一视同仁。
    pub async fn execute_named(
        &self,
        agent_id: &str,
        allowlist: &[ToolName],
        name: &str,
        args: &serde_json::Value,
    ) -> ToolResult {
        let started = Instant::now();
        // preTool 钩子：非零退出即拦截（返回原因给模型，计入审计）
        if !self.hooks.pre_tool.is_empty() {
            let args_json = serde_json::to_string(args).unwrap_or_default();
            let short_args: String = args_json.chars().take(4000).collect();
            let root = self.effective_root();
            let root_s = root.display().to_string();
            for h in self.hooks.pre_tool.clone() {
                let (ok, out) = run_hook_command(
                    &h,
                    &root,
                    &[
                        ("EXM_HOOK", "pre_tool"),
                        ("EXM_TOOL", name),
                        ("EXM_AGENT", agent_id),
                        ("EXM_ARGS", &short_args),
                        ("EXM_WORKSPACE", &root_s),
                    ],
                );
                if !ok {
                    let why = if out.trim().is_empty() { h.clone() } else { out.clone() };
                    let blocked = ToolResult::err(format!("preTool 钩子拦截：{why}"));
                    let _ = self.store.audit_tool(
                        agent_id,
                        name,
                        args,
                        "preTool 钩子拦截",
                        started.elapsed().as_millis() as u64,
                    );
                    return blocked;
                }
            }
        }

        let result = match ToolName::parse(name) {
            Some(tool) => {
                if !allowlist.contains(&tool) {
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
                }
            }
            // 非内置名：声明式自定义工具（可扩展工具面，免写 MCP 服务器）
            None => match self.custom.iter().find(|c| c.name == name) {
                Some(ct) if ct.visible_to(agent_id) => self.run_custom(agent_id, ct, args).await,
                Some(_) => ToolResult::err(format!(
                    "工具 {name} 未对本个体开放（自定义工具的 agents 未包含 {agent_id}）"
                )),
                None => ToolResult::err(format!("未知工具：{name}")),
            },
        };
        let result = self.spill_if_large(name, result);
        // postTool 钩子：仅留痕（失败不改变工具结果，但会追加到输出末尾提示）
        let mut result = result;
        if !self.hooks.post_tool.is_empty() {
            let root = self.effective_root();
            let root_s = root.display().to_string();
            let ok_s = if result.ok { "true" } else { "false" };
            let preview: String = if result.ok {
                result.output.chars().take(4000).collect()
            } else {
                result.error.clone().unwrap_or_default().chars().take(4000).collect()
            };
            for h in self.hooks.post_tool.clone() {
                let (hook_ok, out) = run_hook_command(
                    &h,
                    &root,
                    &[
                        ("EXM_HOOK", "post_tool"),
                        ("EXM_TOOL", name),
                        ("EXM_AGENT", agent_id),
                        ("EXM_TOOL_OK", ok_s),
                        ("EXM_OUTPUT", &preview),
                        ("EXM_WORKSPACE", &root_s),
                    ],
                );
                if !hook_ok {
                    let tail = format!("\n[postTool 钩子失败] {}", out.chars().take(300).collect::<String>());
                    if result.ok {
                        result.output.push_str(&tail);
                    } else if let Some(e) = result.error.as_mut() {
                        e.push_str(&tail);
                    }
                }
            }
        }
        let summary = if result.ok {
            result.output.chars().take(200).collect::<String>()
        } else {
            result.error.clone().unwrap_or_default().chars().take(200).collect()
        };
        let _ = self.store.audit_tool(agent_id, name, args, &summary, started.elapsed().as_millis() as u64);
        result
    }

    /// 结果超阈值时全文落盘并回填「摘要 + 路径 + 行数」，供模型按需用 read 取回。
    /// 阈值 0 = 关闭（仅按工具自身的截断）。
    fn spill_if_large(&self, name: &str, result: ToolResult) -> ToolResult {
        let limit = self.security.tool_output_spill_chars;
        if limit == 0 {
            return result;
        }
        let (text, is_err) = if result.ok {
            (result.output.clone(), false)
        } else {
            (result.error.clone().unwrap_or_default(), true)
        };
        if text.chars().count() <= limit {
            return result;
        }
        let dir = self.workspace_root.join(".exmachina").join("tool-output");
        if std::fs::create_dir_all(&dir).is_err() {
            return result;
        }
        let stamp = crate::types::now_iso().replace(':', "").replace('-', "");
        let file = dir.join(format!("{stamp}-{name}.txt"));
        if std::fs::write(&file, &text).is_err() {
            return result;
        }
        let lines = text.lines().count();
        let head: String = text.chars().take(4000).collect();
        let rel = file
            .strip_prefix(&self.workspace_root)
            .map(|p| p.display().to_string().replace('\\', "/"))
            .unwrap_or_else(|_| file.display().to_string());
        let note = format!(
            "{head}\n\n…（结果共 {total} 字符 / {lines} 行，已截断）\n完整内容已落盘：{rel}\n可用 read 工具按 offset/limit 分段读取。",
            total = text.chars().count()
        );
        if is_err {
            ToolResult::err(note)
        } else {
            ToolResult::ok(note)
        }
    }

    async fn dispatch(&self, agent_id: &str, tool: ToolName, args: &serde_json::Value) -> ToolResult {
        match tool {
            ToolName::Read => self.tool_read(args),
            ToolName::Edit => self.tool_edit(args),
            ToolName::Grep => self.tool_grep(args),
            ToolName::Glob => self.tool_glob(args),
            ToolName::Filesystem => self.tool_fs(args),
            ToolName::Terminal => self.tool_terminal(agent_id, args).await,
            ToolName::WebSearch => self.tool_web_search(args).await,
            ToolName::WebFetch => self.tool_web_fetch(args).await,
            ToolName::AgentManage => self.tool_agent_manage(args),
            ToolName::Schedule => self.tool_schedule(args),
            ToolName::Browser => self.tool_browser(args).await,
        }
    }

    // ---------------------------------------------------------------- 声明式自定义工具

    /// 执行声明式自定义工具：http（GET query / POST JSON）或 shell（模板插值 + 审批闸门）。
    /// 输出与内置工具同口径截断，并共享结果落盘 / 钩子 / 审计链路（execute_named 统一走这里）。
    async fn run_custom(
        &self,
        agent_id: &str,
        tool: &crate::config::CustomTool,
        args: &serde_json::Value,
    ) -> ToolResult {
        let Some(params) = args.as_object() else {
            return ToolResult::err("自定义工具的参数必须是对象");
        };
        // 插值用字符串表：字符串直取，其余 JSON 序列化
        let mut vals: std::collections::BTreeMap<String, String> = Default::default();
        for (k, v) in params {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            vals.insert(k.clone(), s);
        }
        // target 引用的占位：schema 声明 required 而调用未给 → 明确报错（拼空串比报错更坑）
        let required: Vec<String> = tool
            .parameters
            .pointer("/required")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        for name in placeholder_names(&tool.target) {
            if !vals.contains_key(&name) {
                if required.iter().any(|r| r == &name) {
                    return ToolResult::err(format!("自定义工具 {} 缺少必填参数 {name}", tool.name));
                }
                vals.insert(name, String::new());
            }
        }
        if tool.kind_norm() == "shell" {
            return self.run_custom_shell(agent_id, tool, &vals).await;
        }
        self.run_custom_http(tool, &vals, args).await
    }

    /// http 类自定义工具：GET 把实参拼进已插值的 URL；POST/PUT 把全部实参作为 JSON body
    async fn run_custom_http(
        &self,
        tool: &crate::config::CustomTool,
        vals: &std::collections::BTreeMap<String, String>,
        args: &serde_json::Value,
    ) -> ToolResult {
        let url = interpolate(&tool.target, vals, urlencode);
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ToolResult::err("http 类自定义工具的 target 必须是 http(s) URL");
        }
        let method = {
            let m = tool.method.trim().to_ascii_uppercase();
            if m.is_empty() { "POST".to_string() } else { m }
        };
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("HTTP 客户端构建失败: {e}")),
        };
        let parsed = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::POST);
        let mut req = client.request(parsed, &url);
        for (k, v) in &tool.headers {
            req = req.header(k, v);
        }
        if method == "POST" || method == "PUT" || method == "PATCH" {
            req = req.json(args);
        }
        match req.send().await {
            Ok(r) => {
                let status = r.status();
                let text = r.text().await.unwrap_or_default();
                if !status.is_success() {
                    return ToolResult::err(format!(
                        "HTTP {status}：{}",
                        text.chars().take(2000).collect::<String>()
                    ));
                }
                ToolResult::ok(text.chars().take(8000).collect::<String>())
            }
            Err(e) => ToolResult::err(format!("请求失败: {e}")),
        }
    }

    /// shell 类自定义工具：模板插值（值经白名单校验 + 引号包裹）→ 与 terminal 同一道审批闸门
    async fn run_custom_shell(
        &self,
        agent_id: &str,
        tool: &crate::config::CustomTool,
        vals: &std::collections::BTreeMap<String, String>,
    ) -> ToolResult {
        // 实参字符白名单：模板插值 + 引号包裹仍挡不住的 shell 元字符一律拒绝 —— 宁可失败不留注入面
        for (k, v) in vals {
            if let Some(c) = v.chars().find(|c| shell_unsafe_char(*c)) {
                return ToolResult::err(format!(
                    "自定义工具 {} 的参数 {k} 含不允许的字符 {c:?}（shell 类工具实参仅支持普通文本）",
                    tool.name
                ));
            }
        }
        let cmd = interpolate(&tool.target, vals, shell_quote);
        // 与 terminal 同一道闸门：声明式模板不是越权通道（risky 拦高危 / always 全拦）
        let lowered = cmd.to_lowercase();
        let mode = self.security.exec_approval.as_str();
        if mode == "risky" || mode == "always" {
            let allowlisted = self
                .security
                .exec_allowlist
                .iter()
                .any(|p| !p.trim().is_empty() && lowered.starts_with(p.trim().to_lowercase().as_str()));
            let gated = mode == "always" || is_destructive_command(&lowered);
            if gated && !allowlisted {
                return self.request_approval(agent_id, &cmd);
            }
        }
        let opts = self.shell_opts(&cmd);
        execute_shell_with(&self.effective_root(), &cmd, 120, &opts).await
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

    /// 路径安全解析：规范化后必须落在「组工作区」或「全局工作区」之内。
    /// 不存在的路径按「最近存在的父目录」规范化后再判定 —— 否则 `..` 拼接可绕过越界检查。
    fn resolve_safe(&self, raw: &str) -> anyhow::Result<PathBuf> {
        let raw_trim = raw.trim();
        if raw_trim.is_empty() {
            bail!("路径不能为空");
        }
        let group_root = self.effective_root();
        let candidate = if Path::new(raw_trim).is_absolute() {
            PathBuf::from(raw_trim)
        } else {
            group_root.join(raw_trim)
        };
        let group_canon = canonicalize_best(&group_root);
        let ws_canon = canonicalize_best(&self.workspace_root);
        let cand_canon = canonicalize_best(&candidate);
        if !cand_canon.starts_with(&group_canon) && !cand_canon.starts_with(&ws_canon) {
            bail!("路径越界（仅允许工作区内的路径）: {raw}");
        }
        Ok(candidate)
    }

    /// 写前快照（检查点）：原文件复制到 .exmachina/checkpoints/<日期>/<相对路径>。
    /// 同日同名已存在则跳过 —— 保存的是「当日首次改动前」的版本，供 `exm checkpoint restore` 回滚。
    fn checkpoint(&self, target: &Path) -> anyhow::Result<()> {
        if !target.is_file() {
            return Ok(());
        }
        let root = self.effective_root();
        let Ok(rel) = target.strip_prefix(&root) else {
            return Ok(()); // 组工作区在工作区外：不做快照（无回滚能力，但也不阻断写入）
        };
        let day: String = crate::types::now_iso().chars().take(10).collect();
        let dst = self
            .workspace_root
            .join(".exmachina")
            .join("checkpoints")
            .join(day)
            .join(rel);
        if dst.exists() {
            return Ok(());
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(target, &dst).context("写检查点失败")?;
        Ok(())
    }

    /// 读取文件或目录：文件按行带行号返回（offset/limit 分页），目录返回条目清单
    fn tool_read(&self, args: &serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(400).clamp(1, 5000) as usize;
        let max_chars = args.get("maxChars").and_then(|v| v.as_u64()).unwrap_or(24000) as usize;
        let p = match self.resolve_safe(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        if p.is_dir() {
            return self.list_dir(&p);
        }
        let bytes = match std::fs::read(&p) {
            Ok(b) => b,
            Err(e) => return ToolResult::err(format!("读取失败: {e}")),
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult::err(format!(
                    "不是 UTF-8 文本文件（{} 字节），无法按行读取；二进制处理请用 terminal",
                    e.as_bytes().len()
                ))
            }
        };
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        if total == 0 {
            return ToolResult::ok(format!("（空文件，0 行）{path}"));
        }
        let start = (offset - 1).min(total);
        let mut out = String::new();
        let mut used = 0usize;
        let mut end = start;
        for (i, line) in lines.iter().enumerate().skip(start) {
            if i - start >= limit {
                break;
            }
            let rendered = format!("{:>5}→{}\n", i + 1, line);
            if used + rendered.chars().count() > max_chars {
                if i == start {
                    out.push_str(&format!("{:>5}→{}\n", i + 1, line.chars().take(600).collect::<String>()));
                    end = i + 1;
                }
                break;
            }
            used += rendered.chars().count();
            out.push_str(&rendered);
            end = i + 1;
        }
        let shown = if end > start { format!("{}–{}", start + 1, end) } else { "无".to_string() };
        out.push_str(&format!("\n（{path}：共 {total} 行；已显示第 {shown} 行）"));
        if end < total {
            out.push_str(&format!("；续读 offset={}", end + 1));
        }
        ToolResult::ok(out)
    }

    /// 目录清单：目录在前，带大小与条目总数
    fn list_dir(&self, p: &Path) -> ToolResult {
        let entries = match std::fs::read_dir(p) {
            Ok(e) => e,
            Err(e) => return ToolResult::err(format!("列目录失败: {e}")),
        };
        let mut rows: Vec<(String, bool, u64)> = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let meta = e.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            rows.push((name, is_dir, size));
        }
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        let total = rows.len();
        let body = rows
            .iter()
            .take(200)
            .map(|(n, d, s)| {
                if *d {
                    format!("[dir]  {n}/")
                } else {
                    format!("[file] {n}  ({})", human_size(*s))
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if body.is_empty() {
            return ToolResult::ok("（空目录）");
        }
        ToolResult::ok(format!("{body}\n（共 {total} 项，已列 {}）", total.min(200)))
    }

    /// 精确编辑：old_string → new_string（唯一命中校验；支持 replace_all）
    fn tool_edit(&self, args: &serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let old = args.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
        let new = args.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
        let replace_all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
        if old.is_empty() {
            return ToolResult::err("old_string 不能为空；新建文件请用 filesystem write");
        }
        if old == new {
            return ToolResult::err("old_string 与 new_string 完全相同，无需编辑");
        }
        let p = match self.resolve_safe(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        if !p.is_file() {
            return ToolResult::err(format!("文件不存在或不是普通文件: {path}"));
        }
        let content = match std::fs::read_to_string(&p) {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("读取失败: {e}")),
        };
        let hits: Vec<usize> = content
            .match_indices(old)
            .map(|(i, _)| content[..i].matches('\n').count() + 1)
            .collect();
        if hits.is_empty() {
            return ToolResult::err(format!(
                "未找到匹配文本（{path}）。old_string 需与文件内容逐字符一致（含缩进与换行）——建议先用 read 读取该段再复制。"
            ));
        }
        if hits.len() > 1 && !replace_all {
            let at: Vec<String> = hits.iter().take(8).map(|l| l.to_string()).collect();
            return ToolResult::err(format!(
                "old_string 命中 {} 处（行 {}），无法判断改哪一处：请扩大上下文使其唯一，或置 replace_all=true 全部替换。",
                hits.len(),
                at.join(", ")
            ));
        }
        let (updated, count) = if replace_all {
            (content.replace(old, new), hits.len())
        } else {
            (content.replacen(old, new, 1), 1)
        };
        if let Err(e) = self.checkpoint(&p) {
            return ToolResult::err(format!("写前检查点失败，已中止编辑：{e}"));
        }
        if let Err(e) = std::fs::write(&p, &updated) {
            return ToolResult::err(format!("写入失败: {e}"));
        }
        let delta = updated.lines().count() as i64 - content.lines().count() as i64;
        let mut msg = format!("已替换 {count} 处（{path}，首处位于第 {} 行）", hits[0]);
        if delta != 0 {
            msg.push_str(&format!("；行数 {delta:+}"));
        }
        ToolResult::ok(msg)
    }

    /// 仓库检索：正则搜索文件内容（尊重 .gitignore），返回 文件:行号:内容
    fn tool_grep(&self, args: &serde_json::Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.trim().is_empty() {
            return ToolResult::err("pattern 不能为空");
        }
        let sub = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob = args.get("glob").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let ic = args.get("ignoreCase").and_then(|v| v.as_bool()).unwrap_or(false);
        let max_results = args.get("maxResults").and_then(|v| v.as_u64()).unwrap_or(100).clamp(1, 500) as usize;
        let root = match self.resolve_safe(sub) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        let re = match regex::RegexBuilder::new(pattern).case_insensitive(ic).build() {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("正则无效: {e}")),
        };
        let walker = build_walker(&root, &glob);
        let base = self.effective_root();
        let mut out: Vec<String> = Vec::new();
        let mut hit_files = 0usize;
        let mut truncated = false;
        'files: for entry in walker {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > 2 * 1024 * 1024 {
                continue; // 跳过 >2MB 的文件
            }
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            let rel = path.strip_prefix(&base).unwrap_or(path).display().to_string().replace('\\', "/");
            let mut file_hit = false;
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    if !file_hit {
                        file_hit = true;
                        hit_files += 1;
                    }
                    let trimmed: String = line.trim().chars().take(240).collect();
                    out.push(format!("{rel}:{}:{}", i + 1, trimmed));
                    if out.len() >= max_results {
                        truncated = true;
                        break 'files;
                    }
                }
            }
        }
        if out.is_empty() {
            return ToolResult::ok(format!("无匹配（pattern: {pattern}）"));
        }
        let mut body = out.join("\n");
        body.push_str(&format!(
            "\n（{} 条命中，涉及 {hit_files} 个文件{}）",
            out.len(),
            if truncated { "，已达结果上限——请收窄 pattern/path" } else { "" }
        ));
        ToolResult::ok(body)
    }

    /// 按 glob 模式列出文件路径
    fn tool_glob(&self, args: &serde_json::Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if pattern.is_empty() {
            return ToolResult::err("pattern 不能为空（如 **/*.rs）");
        }
        let sub = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = match self.resolve_safe(sub) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e.to_string()),
        };
        let walker = build_walker(&root, &pattern);
        let base = self.effective_root();
        let mut files: Vec<String> = Vec::new();
        for entry in walker {
            let Ok(entry) = entry else { continue };
            if entry.path().is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(&base)
                    .unwrap_or(entry.path())
                    .display()
                    .to_string()
                    .replace('\\', "/");
                files.push(rel);
                if files.len() >= 500 {
                    break;
                }
            }
        }
        files.sort();
        if files.is_empty() {
            return ToolResult::ok(format!("无匹配文件（pattern: {pattern}）"));
        }
        let n = files.len();
        ToolResult::ok(format!("{}\n（{n} 个文件）", files.join("\n")))
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
                if let Err(e) = self.checkpoint(&target) {
                    return ToolResult::err(format!("写前检查点失败，已中止写入：{e}"));
                }
                match std::fs::write(&target, content).context("写入失败") {
                    Ok(_) => ToolResult::ok(format!(
                        "written: {}（{} 行 / {} 字符）",
                        target.display(),
                        content.lines().count(),
                        content.chars().count()
                    )),
                    Err(e) => ToolResult::err(e.to_string()),
                }
            }
            "mkdir" => match std::fs::create_dir_all(&target) {
                Ok(_) => ToolResult::ok(format!("mkdir: {}", target.display())),
                Err(e) => ToolResult::err(e.to_string()),
            },
            "list" => {
                if target.is_dir() {
                    self.list_dir(&target)
                } else {
                    ToolResult::err(format!("不是目录: {path}"))
                }
            }
            other => ToolResult::err(format!(
                "未知 filesystem 操作: {other}（支持 write / mkdir / list；删除文件请用 terminal，受审批与审计约束）"
            )),
        }
    }

    async fn tool_terminal(&self, agent_id: &str, args: &serde_json::Value) -> ToolResult {
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("run");
        // 后台任务轮询 / 终止（不经过白名单与审批：对象是已批准过的任务）
        match op {
            "poll" => return self.bg_poll(args),
            "kill" => return self.bg_kill(args),
            _ => {}
        }
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if cmd.is_empty() {
            return ToolResult::err("command 不能为空");
        }
        // 单一命令约束：带 shell 连接符（&& | ; 重定向等）一律拒绝，防白名单绕过
        if contains_shell_chain(&cmd) {
            return ToolResult::err(
                "命令包含 shell 连接符（&& / | / ; / 重定向等），白名单只放行单一命令；请拆成多次调用。",
            );
        }
        let lowered = cmd.to_lowercase();
        if !terminal_allowed(&cmd) {
            return ToolResult::err(format!(
                "命令不在白名单: {cmd}（白名单按「命令 + 子命令」匹配，且不支持连接符）"
            ));
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
                return self.request_approval(agent_id, &cmd);
            }
        }
        // 沙箱 strict：联网类命令（安装/下载/克隆）默认需审批——把「外联」与「本地干活」分开
        if self.sandbox.strict() && !self.sandbox.allow_network && is_network_command(&lowered) {
            return self.request_approval(agent_id, &cmd);
        }
        // 后台模式：长任务（构建/测试/服务）不阻塞本轮，返回任务 id 供轮询
        if args.get("background").and_then(|v| v.as_bool()).unwrap_or(false) {
            return self.bg_start(&cmd);
        }
        let timeout = args
            .get("timeoutSecs")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.security.terminal_timeout_secs as u64)
            .clamp(5, self.security.terminal_timeout_secs.max(5) as u64);
        // strict 模式收紧前台命令时长：长活请显式用后台模式
        let timeout = if self.sandbox.strict() { timeout.min(60) } else { timeout };
        execute_shell_with(&self.effective_root(), &cmd, timeout, &self.shell_opts(&cmd)).await
    }

    /// 启动后台任务：输出累积到共享缓冲，任务终结记录退出码
    fn bg_start(&self, cmd: &str) -> ToolResult {
        const BG_OUTPUT_CAP: usize = 200_000;
        let id = format!("bg-{}", crate::types::new_id()[..8].to_string());
        let buf = Arc::new(parking_lot::Mutex::new(String::new()));
        let state = Arc::new(parking_lot::Mutex::new(None::<i32>));
        let cwd = self.effective_root();

        let opts = self.shell_opts(cmd);
        let mut process = tokio::process::Command::new(&opts.program);
        let effective: String = opts.cmd_override.clone().unwrap_or_else(|| cmd.to_string());
        if opts.args.is_empty() {
            #[cfg(target_os = "windows")]
            process.args(["/C", effective.as_str()]);
            #[cfg(not(target_os = "windows"))]
            process.args(["-c", effective.as_str()]);
        } else {
            process.args(&opts.args);
        }
        if opts.env_clear {
            process.env_clear();
        }
        for (k, v) in &opts.envs {
            process.env(k, v);
        }
        let spawned = process
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("后台任务启动失败: {e}")),
        };
        // Windows：作业对象随任务存活；任务被 abort 时 Drop → 关句柄 → 整树终止
        let job = attach_job(&child, &opts);
        let buf2 = buf.clone();
        let state2 = state.clone();
        let handle = tokio::spawn(async move {
            let _job = job;
            let mut out = child.stdout.take();
            let mut err = child.stderr.take();
            let b3 = buf2.clone();
            let t_out = out.take().map(|mut s| {
                let b = b3.clone();
                tokio::spawn(async move {
                    let mut chunk = [0u8; 8192];
                    loop {
                        match tokio::io::AsyncReadExt::read(&mut s, &mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let mut b = b.lock();
                                if b.len() < BG_OUTPUT_CAP {
                                    b.push_str(&String::from_utf8_lossy(&chunk[..n]));
                                }
                            }
                        }
                    }
                })
            });
            let b4 = buf2.clone();
            let t_err = err.take().map(|mut s| {
                let b = b4.clone();
                tokio::spawn(async move {
                    let mut chunk = [0u8; 4096];
                    loop {
                        match tokio::io::AsyncReadExt::read(&mut s, &mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let mut b = b.lock();
                                if b.len() < BG_OUTPUT_CAP {
                                    b.push_str(&String::from_utf8_lossy(&chunk[..n]));
                                }
                            }
                        }
                    }
                })
            });
            let status = child.wait().await;
            if let Some(t) = t_out {
                let _ = t.await;
            }
            if let Some(t) = t_err {
                let _ = t.await;
            }
            *state2.lock() = Some(status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1));
        });
        let task = BgTask { cmd: cmd.to_string(), started: Instant::now(), buf, state, abort: handle.abort_handle() };
        self.background.lock().insert(id.clone(), task);
        ToolResult::ok(format!(
            "后台任务已启动：taskId={id}\n命令：{cmd}\n用 terminal(op=\"poll\", taskId=\"{id}\") 查看输出；op=\"kill\" 终止。"
        ))
    }

    /// 轮询后台任务：返回状态 + 输出尾部
    fn bg_poll(&self, args: &serde_json::Value) -> ToolResult {
        let id = args.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
        let guard = self.background.lock();
        let Some(t) = guard.get(id) else {
            return ToolResult::err(format!("后台任务不存在: {id}"));
        };
        let state = *t.state.lock();
        let buf = t.buf.lock().clone();
        let elapsed = t.started.elapsed().as_secs();
        let tail: String = {
            let chars: Vec<char> = buf.chars().collect();
            let start = chars.len().saturating_sub(6000);
            chars[start..].iter().collect()
        };
        drop(guard);
        let status_line = match state {
            None => format!("运行中（已 {elapsed}s）"),
            Some(0) => format!("已完成（exit=0，耗时 {elapsed}s）"),
            Some(c) => format!("已结束（exit={c}，耗时 {elapsed}s）"),
        };
        ToolResult::ok(format!("{status_line}\n—— 输出（累计 {} 字符）——\n{tail}", buf.chars().count()))
    }

    /// 终止后台任务（中止读取任务 → kill_on_drop 连带结束子进程）
    fn bg_kill(&self, args: &serde_json::Value) -> ToolResult {
        let id = args.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
        let mut guard = self.background.lock();
        match guard.remove(id) {
            Some(t) => {
                t.abort.abort();
                ToolResult::ok(format!("后台任务已终止：{id}（{cmd}）", cmd = t.cmd))
            }
            None => ToolResult::err(format!("后台任务不存在: {id}")),
        }
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

    /// 联网搜索：调用配置的后端（tavily / brave / searxng / custom）。
    /// 未配置时该工具不会出现在下发 schema 里（此处仍兜底报错，防文本协议路径调用）。
    async fn tool_web_search(&self, args: &serde_json::Value) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if query.is_empty() {
            return ToolResult::err("query 不能为空");
        }
        if !self.search.ready() {
            return ToolResult::err(
                "联网搜索未配置：请到「设置 → 联网搜索」选择后端并填写凭据（tavily / brave / searxng / custom）。",
            );
        }
        let provider = self.search.provider.trim().to_lowercase();
        let max = args
            .get("maxResults")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 20) as usize)
            .unwrap_or(self.search.max_results.clamp(1, 20));
        let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build() {
            Ok(c) => c,
            Err(e) => return ToolResult::err(format!("HTTP 客户端构建失败: {e}")),
        };
        let (endpoint, method, payload, bearer) = match provider.as_str() {
            "tavily" => {
                let ep = if self.search.endpoint.trim().is_empty() {
                    "https://api.tavily.com/search".to_string()
                } else {
                    self.search.endpoint.trim().to_string()
                };
                (
                    ep,
                    "POST",
                    serde_json::json!({ "api_key": self.search.api_key.trim(), "query": query, "max_results": max }),
                    None,
                )
            }
            "brave" => {
                let ep = if self.search.endpoint.trim().is_empty() {
                    format!("https://api.search.brave.com/res/v1/web/search?q={}&count={}", urlencode(&query), max)
                } else {
                    format!("{}?q={}&count={}", self.search.endpoint.trim().trim_end_matches('/'), urlencode(&query), max)
                };
                (ep, "GET", serde_json::Value::Null, Some(self.search.api_key.trim().to_string()))
            }
            "searxng" => {
                let ep = format!(
                    "{}/search?q={}&format=json",
                    self.search.endpoint.trim().trim_end_matches('/'),
                    urlencode(&query)
                );
                (ep, "GET", serde_json::Value::Null, None)
            }
            _ => {
                // custom：POST {query, maxResults}，响应兼容多种字段名
                (
                    self.search.endpoint.trim().to_string(),
                    "POST",
                    serde_json::json!({ "query": query, "maxResults": max, "apiKey": self.search.api_key.trim() }),
                    None,
                )
            }
        };
        let mut req = if method == "POST" { client.post(&endpoint) } else { client.get(&endpoint) };
        if method == "POST" {
            req = req.json(&payload);
        }
        if let Some(tok) = bearer {
            if !tok.is_empty() {
                req = req.header("X-Subscription-Token", tok);
            }
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("搜索请求失败: {e}")),
        };
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return ToolResult::err(format!("搜索后端返回 HTTP {status}：{}", body.chars().take(300).collect::<String>()));
        }
        let parsed: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => return ToolResult::err(format!("搜索响应不是 JSON: {e}")),
        };
        let items = parsed
            .get("results")
            .or_else(|| parsed.get("web").and_then(|w| w.get("results")))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if items.is_empty() {
            return ToolResult::ok(format!("无搜索结果（query: {query}）"));
        }
        let mut lines: Vec<String> = Vec::new();
        for (i, it) in items.iter().take(max).enumerate() {
            let title = it
                .get("title")
                .or_else(|| it.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("（无标题）");
            let url = it.get("url").or_else(|| it.get("link")).and_then(|v| v.as_str()).unwrap_or("");
            let snippet = it
                .get("content")
                .or_else(|| it.get("snippet"))
                .or_else(|| it.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            lines.push(format!(
                "{}. {title}\n   {url}\n   {}",
                i + 1,
                snippet.replace('\n', " ").chars().take(300).collect::<String>()
            ));
        }
        ToolResult::ok(format!(
            "搜索「{query}」（{}）：\n{}",
            self.search.provider.trim(),
            lines.join("\n")
        ))
    }

    /// 浏览器自动化（headless Chrome + CDP）：懒启动常驻，串行化访问。
    /// 探测不到浏览器时该工具不会出现在下发 schema 里（此处兜底报错）。
    async fn tool_browser(&self, args: &serde_json::Value) -> ToolResult {
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("open");
        if op == "close" {
            let mut guard = self.browser.lock().await;
            return match guard.take() {
                Some(mut s) => {
                    s.shutdown();
                    ToolResult::ok("浏览器已关闭")
                }
                None => ToolResult::ok("浏览器未启动"),
            };
        }
        let mut guard = self.browser.lock().await;
        if guard.is_none() {
            match crate::browser::BrowserSession::launch(&self.browser_cfg, &self.workspace_root).await {
                Ok(s) => *guard = Some(s),
                Err(e) => return ToolResult::err(format!("启动浏览器失败：{e}")),
            }
        }
        let session = match guard.as_mut() {
            Some(s) => s,
            None => return ToolResult::err("浏览器会话不可用"),
        };
        let max_default = self.browser_cfg.max_chars;
        let max = args
            .get("maxChars")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(max_default);
        match op {
            "open" => {
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
                if !(url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("file://")
                    || url.starts_with("about:"))
                {
                    return ToolResult::err("url 需以 http(s):// 开头（本地文件用 file://）");
                }
                if let Err(e) = session.navigate(url).await {
                    return ToolResult::err(format!("导航失败：{e}"));
                }
                match session.page_text(max).await {
                    Ok((title, text)) => {
                        ToolResult::ok(format!("已加载：{title}\nURL：{url}\n\n{text}"))
                    }
                    Err(e) => ToolResult::err(format!("读取页面失败：{e}")),
                }
            }
            "text" => match session.page_text(max).await {
                Ok((title, text)) => ToolResult::ok(format!("{title}\n\n{text}")),
                Err(e) => ToolResult::err(format!("读取页面失败：{e}")),
            },
            "eval" => {
                let expr = args.get("expression").and_then(|v| v.as_str()).unwrap_or("");
                if expr.trim().is_empty() {
                    return ToolResult::err("eval 需要 expression");
                }
                match session.eval(expr).await {
                    Ok(v) => ToolResult::ok(v.chars().take(8000).collect::<String>()),
                    Err(e) => ToolResult::err(format!("脚本执行失败：{e}")),
                }
            }
            "screenshot" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                match session.screenshot(&self.workspace_root, name).await {
                    Ok(rel) => ToolResult::ok(format!("截图已保存：{rel}（可用 read 查看路径，或用 browser screenshot 再次覆盖）")),
                    Err(e) => ToolResult::err(format!("截图失败：{e}")),
                }
            }
            other => ToolResult::err(format!(
                "未知 browser 操作: {other}（支持 open / text / eval / screenshot / close）"
            )),
        }
    }

    /// 自主排程：创建/查看/删除定时与一次性任务（AI 自驱工作的时间维度）
    fn tool_schedule(&self, args: &serde_json::Value) -> ToolResult {
        let Some(cron) = &self.cron else {
            return ToolResult::err("排程存储未接入（该运行环境不支持 schedule）");
        };
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("list");
        match op {
            "create" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let expr = args.get("cron").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                let at = args.get("at").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if name.is_empty() || prompt.is_empty() {
                    return ToolResult::err("create 需要 name 与 prompt");
                }
                if expr.is_none() && at.is_none() {
                    return ToolResult::err("create 需要 cron（周期）或 at（一次性时间）之一");
                }
                if let Some(e) = &expr {
                    if e.split_whitespace().count() != 5 {
                        return ToolResult::err("cron 需为五段：分 时 日 月 周，如 0 9 * * *");
                    }
                }
                let job = crate::types::CronJob {
                    id: crate::types::new_id()[..8].to_string(),
                    name,
                    prompt,
                    cron: expr,
                    at,
                    group: Some(self.registry.active_group()),
                    session_title: None,
                    enabled: true,
                    last_run_at: None,
                    last_status: None,
                    last_run_minute: None,
                    created_at: crate::types::now_iso(),
                };
                match cron.upsert(job) {
                    Ok(j) => ToolResult::ok(format!(
                        "已排程：id={} 名称={} 触发={}（激活组 {}；到期由调度器唤醒本组执行）",
                        j.id,
                        j.name,
                        j.cron.clone().or(j.at.clone()).unwrap_or_default(),
                        self.registry.active_group()
                    )),
                    Err(e) => ToolResult::err(format!("排程失败: {e}")),
                }
            }
            "list" => match cron.list() {
                Ok(jobs) => {
                    let filter = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let rows: Vec<String> = jobs
                        .iter()
                        .filter(|j| filter.is_empty() || j.id == filter)
                        .map(|j| {
                            format!(
                                "- {} [{}] {} 触发={} 启用={}{}",
                                j.name,
                                j.id,
                                if j.prompt.chars().count() > 40 {
                                    format!("{}…", j.prompt.chars().take(40).collect::<String>())
                                } else {
                                    j.prompt.clone()
                                },
                                j.cron.clone().or(j.at.clone()).unwrap_or_default(),
                                if j.enabled { "是" } else { "否" },
                                j.last_status
                                    .as_ref()
                                    .map(|s| format!(" 上次={s}"))
                                    .unwrap_or_default()
                            )
                        })
                        .collect();
                    if rows.is_empty() {
                        ToolResult::ok("（无排程任务）")
                    } else {
                        ToolResult::ok(rows.join("\n"))
                    }
                }
                Err(e) => ToolResult::err(format!("读取排程失败: {e}")),
            },
            "remove" | "enable" | "disable" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if id.is_empty() {
                    return ToolResult::err(format!("{op} 需要 id"));
                }
                if op == "remove" {
                    return match cron.remove(&id) {
                        Ok(true) => ToolResult::ok(format!("已删除排程：{id}")),
                        Ok(false) => ToolResult::err(format!("排程不存在：{id}")),
                        Err(e) => ToolResult::err(format!("删除失败: {e}")),
                    };
                }
                match cron.get(&id) {
                    Ok(Some(mut job)) => {
                        job.enabled = op == "enable";
                        match cron.upsert(job) {
                            Ok(_) => ToolResult::ok(format!("已{}排程：{id}", if op == "enable" { "启用" } else { "停用" })),
                            Err(e) => ToolResult::err(format!("更新失败: {e}")),
                        }
                    }
                    Ok(None) => ToolResult::err(format!("排程不存在：{id}")),
                    Err(e) => ToolResult::err(format!("读取失败: {e}")),
                }
            }
            other => ToolResult::err(format!("未知 schedule 操作: {other}（支持 create/list/remove/enable/disable）")),
        }
    }
}

// ---------------------------------------------------------------- 自由函数

/// 去掉 Windows canonicalize 产生的 `\\?\` 前缀（便于跨路径比较）
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p
    }
}

/// 词法规范化：消解 `.` 与 `..`（不解析符号链接）
fn normalize_lexically(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 尽可能规范化：路径存在则 canonicalize；不存在时向父目录回溯，
/// 拼接规范化后的祖先 + 剩余部分（词法消解 `..`）——保证越界判定用的是真实路径。
fn canonicalize_best(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return strip_verbatim(c);
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = p.to_path_buf();
    loop {
        if let Ok(base) = cur.canonicalize() {
            let mut joined = strip_verbatim(base);
            for t in tail.iter().rev() {
                joined.push(t);
            }
            return normalize_lexically(&joined);
        }
        let Some(name) = cur.file_name().map(|s| s.to_os_string()) else {
            return normalize_lexically(p);
        };
        tail.push(name);
        if !cur.pop() {
            return normalize_lexically(p);
        }
    }
}

/// 人类可读大小
fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1048576.0)
    }
}

/// 构造遍历器：尊重 .gitignore / .ignore（隐藏文件也纳入，便于读 .github 等目录）；
/// glob 非空时只保留匹配项（`*.rs` 这类裸模式自动补 `**/` 前缀）。
fn build_walker(root: &Path, glob: &str) -> ignore::Walk {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false).git_ignore(true).git_global(true).max_filesize(Some(4 * 1024 * 1024));
    if !glob.trim().is_empty() {
        let mut ov = ignore::overrides::OverrideBuilder::new(root);
        let pat = glob.trim();
        let normalized = if pat.contains('/') || pat.starts_with("**") {
            pat.to_string()
        } else {
            format!("**/{pat}")
        };
        if ov.add(&normalized).is_ok() {
            if let Ok(set) = ov.build() {
                builder.overrides(set);
            }
        }
    }
    builder.build()
}

/// 极简 URL 编码（查询串用）
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------- 自定义工具插值

/// 模板里的占位名清单（`{name}`；不完整的 `{` 忽略）
fn placeholder_names(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        match rest[i + 1..].find('}') {
            Some(j) => {
                let name = rest[i + 1..i + 1 + j].trim().to_string();
                if !name.is_empty() && !out.contains(&name) {
                    out.push(name);
                }
                rest = &rest[i + j + 2..];
            }
            None => break,
        }
    }
    out
}

/// 单趟插值：`{name}` → 处理后的值；未提供的占位（已在调用方兜底为空串）替换为空
fn interpolate(template: &str, vals: &std::collections::BTreeMap<String, String>, f: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        match rest[i + 1..].find('}') {
            Some(j) => {
                let name = rest[i + 1..i + 1 + j].trim();
                out.push_str(&f(vals.get(name).map(String::as_str).unwrap_or("")));
                rest = &rest[i + j + 2..];
            }
            None => {
                out.push('{');
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// shell 实参不允许出现的元字符（模板插值 + 引号包裹之外的最后防线）
fn shell_unsafe_char(c: char) -> bool {
    matches!(
        c,
        '"' | '\'' | '`' | '$' | '&' | '|' | ';' | '<' | '>' | '(' | ')' | '%' | '!' | '^' | '\\' | '\n' | '\r' | '\t'
    )
}

/// 实参包裹：Windows 双引号 / POSIX 单引号（调用方已先行做字符白名单校验）
fn shell_quote(v: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        format!("\"{v}\"")
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("'{v}'")
    }
}
