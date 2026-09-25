//! 浏览器自动化 —— `browser` 工具：headless Chrome/Chromium + CDP（Chrome DevTools Protocol）。
//!
//! 设计约束：
//! - **不引第三方驱动**：目标列表走 HTTP（reqwest），CDP 走已有依赖 tokio-tungstenite；
//! - **懒启动常驻**：首次调用拉起浏览器进程，后续调用复用；`close` 或进程退出即回收；
//! - **截图落工作区**：PNG 写到 `.exmachina/screenshots/`，回填相对路径供人查看；
//! - 正文提取在页面内用 JS `innerText` 完成（渲染后的真实文本，比抓 HTML 更干净）。

use crate::config::BrowserConfig;
use anyhow::{bail, Context};
use futures_util::{SinkExt, StreamExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

/// 自动探测浏览器可执行文件（配置为空时使用）
pub fn find_browser(hint: &str) -> Option<PathBuf> {
    let h = hint.trim();
    if !h.is_empty() {
        let p = PathBuf::from(h);
        return p.is_file().then_some(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "windows")]
    {
        let mut push_base = |base: &str, rest: &[&str]| {
            let mut p = PathBuf::from(base);
            for r in rest {
                p.push(r);
            }
            candidates.push(p);
        };
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                push_base(&base, &["Google", "Chrome", "Application", "chrome.exe"]);
                push_base(&base, &["Microsoft", "Edge", "Application", "msedge.exe"]);
                push_base(&base, &["Chromium", "Application", "chrome.exe"]);
            }
        }
        // Playwright / puppeteer 缓存（本机常用）
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let cache = PathBuf::from(&local).join("ms-playwright");
            if let Ok(entries) = std::fs::read_dir(&cache) {
                let mut dirs: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().starts_with("chromium"))
                            .unwrap_or(false)
                    })
                    .collect();
                dirs.sort();
                for d in dirs.into_iter().rev() {
                    candidates.push(d.join("chrome-win64").join("chrome.exe"));
                    candidates.push(d.join("chrome-win").join("chrome.exe"));
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        for c in [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ] {
            candidates.push(PathBuf::from(c));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// 极简 base64 解码（截图是 data URL 的 base64 段；避免引入新依赖）
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else {
            if c.is_ascii_whitespace() {
                continue;
            }
            return None;
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9222)
}

/// 浏览器会话：进程 + 浏览器级 CDP 连接（页面会话经 Target.attachToTarget 展平）
pub struct BrowserSession {
    child: Child,
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    next_id: u64,
    timeout_secs: u64,
    user_data_dir: PathBuf,
    /// 页面会话 id（flatten 模式：所有页面命令都带它）
    session_id: Option<String>,
}

impl BrowserSession {
    /// 拉起浏览器并连上第一个页面目标
    pub async fn launch(cfg: &BrowserConfig, workspace_root: &Path) -> anyhow::Result<Self> {
        let exe = find_browser(&cfg.executable).context(
            "未找到 Chrome/Chromium/Edge 可执行文件：请在「设置 → 浏览器自动化」填写 browser.executable",
        )?;
        let port = free_port();
        let user_data_dir = workspace_root.join(".exmachina").join("browser");
        std::fs::create_dir_all(&user_data_dir)?;
        let mut cmd = Command::new(&exe);
        if cfg.headless {
            cmd.arg("--headless=new");
        }
        cmd.args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-gpu",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            "--hide-scrollbars",
            &format!("--remote-debugging-port={port}"),
            &format!("--user-data-dir={}", user_data_dir.display()),
            "about:blank",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        let child = cmd.spawn().context("启动浏览器进程失败")?;

        // 等待调试端口就绪（拿到浏览器级 WS 地址）
        let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?;
        let base = format!("http://127.0.0.1:{port}");
        let mut browser_ws: Option<String> = None;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Ok(r) = client.get(format!("{base}/json/version")).send().await {
                if r.status().is_success() {
                    let v: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
                    if let Some(u) = v.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) {
                        browser_ws = Some(u.to_string());
                        break;
                    }
                }
            }
        }
        let Some(browser_ws) = browser_ws else {
            let mut child = child;
            let _ = child.kill();
            bail!("浏览器调试端口未就绪（{base}/json/version）");
        };
        let (ws, _) = tokio_tungstenite::connect_async(&browser_ws)
            .await
            .context("连接 CDP WebSocket 失败")?;
        let mut session = BrowserSession {
            child,
            ws,
            next_id: 1,
            timeout_secs: cfg.timeout_secs.max(5),
            user_data_dir,
            session_id: None,
        };
        // 建页面目标并展平附加（现代 CDP 方式；旧的 /json/new 已被新版 Chrome 移除）
        let created = session
            .command("Target.createTarget", serde_json::json!({ "url": "about:blank" }))
            .await
            .context("创建页面目标失败")?;
        let target_id = created
            .get("targetId")
            .and_then(|x| x.as_str())
            .context("创建页面目标失败：无 targetId")?
            .to_string();
        let attached = session
            .command(
                "Target.attachToTarget",
                serde_json::json!({ "targetId": target_id, "flatten": true }),
            )
            .await
            .context("附加页面会话失败")?;
        let sid = attached
            .get("sessionId")
            .and_then(|x| x.as_str())
            .context("附加页面会话失败：无 sessionId")?
            .to_string();
        session.session_id = Some(sid);
        let _ = session.command("Page.enable", serde_json::json!({})).await;
        Ok(session)
    }

    /// 发送 CDP 命令并等待同 id 响应（忽略中间事件）
    pub async fn command(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut payload = serde_json::json!({ "id": id, "method": method, "params": params });
        // flatten 模式：页面命令带 sessionId；未附加页面时走浏览器级（Target.*）
        if let Some(sid) = &self.session_id {
            if !method.starts_with("Target.") {
                payload["sessionId"] = serde_json::json!(sid);
            }
        }
        self.ws
            .send(Message::Text(payload.to_string()))
            .await
            .context("CDP 发送失败")?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(self.timeout_secs);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                bail!("CDP 命令超时（{method}，{}s）", self.timeout_secs);
            }
            let msg = tokio::time::timeout(remaining, self.ws.next())
                .await
                .map_err(|_| anyhow::anyhow!("CDP 命令超时（{method}，{}s）", self.timeout_secs))?;
            let Some(msg) = msg else { bail!("CDP 连接已断开") };
            let msg = msg.context("CDP 读取失败")?;
            let Message::Text(t) = msg else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
            if v.get("id").and_then(|x| x.as_u64()) != Some(id) {
                continue; // 事件或旧响应
            }
            if let Some(err) = v.get("error") {
                bail!("CDP 错误（{method}）：{err}");
            }
            return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
        }
    }

    /// 导航并等待加载完成（readyState=complete，超时则返回当前状态）
    pub async fn navigate(&mut self, url: &str) -> anyhow::Result<()> {
        self.command("Page.navigate", serde_json::json!({ "url": url })).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(self.timeout_secs);
        loop {
            if tokio::time::Instant::now() > deadline {
                return Ok(()); // 超时也返回：页面多数已可读
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            match self.eval("document.readyState").await {
                Ok(s) if s.contains("complete") => return Ok(()),
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// 执行 JS：**表达式或 IIFE**（返回其求值结果，序列化为字符串）。
    /// 注意：多语句脚本请自行包成 `(function(){ … return x; })()` —— 我们不做隐式包装，
    /// 否则「表达式本身是 IIFE」的常见写法会被再包一层而丢掉返回值。
    pub async fn eval(&mut self, expr: &str) -> anyhow::Result<String> {
        let r = self
            .command(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": expr,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(exc) = r.pointer("/exceptionDetails") {
            bail!(
                "页面脚本异常：{}",
                exc.get("text").and_then(|t| t.as_str()).unwrap_or("未知")
            );
        }
        Ok(match r.pointer("/result/value") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        })
    }

    /// 当前页标题 + 正文（JS innerText，渲染后的真实文本）
    pub async fn page_text(&mut self, max_chars: usize) -> anyhow::Result<(String, String)> {
        let title = self.eval("document.title").await.unwrap_or_default();
        let text = self
            .eval(
                "(function(){ var b = document.body; if(!b) return ''; \
                 var t = b.innerText || ''; return t.replace(/\\n{3,}/g,'\\n\\n'); })()",
            )
            .await?;
        let trimmed: String = text.trim().chars().take(max_chars).collect();
        Ok((title, trimmed))
    }

    /// 截图落盘：返回工作区相对路径
    pub async fn screenshot(&mut self, workspace_root: &Path, name: &str) -> anyhow::Result<String> {
        let r = self
            .command("Page.captureScreenshot", serde_json::json!({ "format": "png" }))
            .await?;
        let b64 = r.get("data").and_then(|d| d.as_str()).context("截图数据缺失")?;
        let bytes = b64_decode(b64).context("截图 base64 解码失败")?;
        let dir = workspace_root.join(".exmachina").join("screenshots");
        std::fs::create_dir_all(&dir)?;
        let stem: String = if name.trim().is_empty() {
            crate::types::now_iso().replace(':', "").replace('-', "")
        } else {
            name.trim().chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect()
        };
        let file = dir.join(format!("{stem}.png"));
        std::fs::write(&file, bytes)?;
        Ok(file
            .strip_prefix(workspace_root)
            .map(|p| p.display().to_string().replace('\\', "/"))
            .unwrap_or_else(|_| file.display().to_string()))
    }

    /// 关闭浏览器进程（并清理临时 profile）
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.user_data_dir);
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
