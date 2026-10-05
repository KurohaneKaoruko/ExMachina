//! EXMACHINA 桌面端（Tauri v2 壳）
//!
//! 与 OpenCode 同款交付形态：**桌面端自带本地服务器**（捆绑的 exm-gateway 随应用启动，
//! WebUI 与 API 同端口），同时支持 `--remote <url>` 直连远程网关（服务器上的后台服务）。
//! 移动端无需单独 App：手机浏览器 / PWA 打开网关地址即为同源客户端。
//!
//! 启动形态：
//! - `exmachina-desktop`                                          —— 拉起捆绑网关并打开窗口
//! - `exmachina-desktop --remote http://host:4173 --key <authKey>` —— 直连远程网关
//!
//! 密钥经 Tauri 初始化脚本写入 localStorage（exm.key），复用 WebUI 自身的登录门与鉴权层。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::TcpListener;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Manager;

/// 应用状态：随应用存活的网关子进程（退出时终止）
struct GatewayChild(Mutex<Option<Child>>);

fn main() {
    // 命令行解析：--remote <url>（远程模式）；--key <authKey>（访问密钥，可选）
    let mut remote: Option<String> = None;
    let mut key: Option<String> = None;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--remote" => remote = args.get(i + 1).cloned(),
            "--key" => key = args.get(i + 1).cloned(),
            _ => {}
        }
        i += 2;
    }

    tauri::Builder::default()
        .manage(GatewayChild(Mutex::new(None)))
        .setup(move |app| {
            let url: String = match remote {
                Some(r) => r.trim_end_matches('/').to_string(),
                None => spawn_local_gateway(app.handle())?,
            };
            let mut win = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url.parse().map_err(|e| format!("无效的网关地址: {e}"))?),
            )
            .title("EXMACHINA · 多智能体系统")
            .inner_size(1320.0, 860.0)
            .min_inner_size(420.0, 360.0);
            // 密钥注入：document start 阶段写 localStorage（WebUI 的 api 层读取 exm.key）
            if let Some(k) = key.filter(|k| !k.is_empty()) {
                let script = format!(
                    "try {{ localStorage.setItem('exm.key', '{}'); }} catch (e) {{}}",
                    k.replace('\'', "")
                );
                win = win.initialization_script(&script);
            }
            win.build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri 启动失败")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // 退出即终止网关子进程
                if let Some(child) = app.state::<GatewayChild>().0.lock().unwrap().as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        });
}

/// 拉起捆绑的 exm-gateway：空闲端口 + 独立数据目录，就绪后返回 UI 地址（REST / WS / WebUI 同端口）
fn spawn_local_gateway(app: &tauri::AppHandle) -> Result<String, Box<dyn std::error::Error>> {
    let bin_dir = app.path().resource_dir()?.join("binaries");
    let gateway = ["exm-gateway.exe", "exm-gateway"]
        .iter()
        .map(|n| bin_dir.join(n))
        .find(|p| p.is_file())
        .ok_or("未找到捆绑的 exm-gateway（检查 src-tauri/binaries/，见 desktop/README.md）")?;

    let port = free_port();
    // 数据目录：Windows %LOCALAPPDATA%/ExMachina；类 Unix XDG_DATA_HOME/ExMachina
    let data_dir = data_dir().join("ExMachina");
    std::fs::create_dir_all(data_dir.join("data"))?;

    let mut child = Command::new(&gateway)
        .arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .env("EXM_DATA_DIR", data_dir.join("data"))
        .spawn()
        .map_err(|e| format!("网关启动失败: {e}"))?;

    // 就绪等待：TCP 可连即视为就绪（最多 30s；提前退出立即报错）
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            let _ = child.kill();
            return Err("exm-gateway 进程提前退出（多为模型通道未配置或端口被占，详见数据目录日志）".into());
        }
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            *app.state::<GatewayChild>().0.lock().unwrap() = Some(child);
            return Ok(format!("http://127.0.0.1:{port}"));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("网关 30s 内未就绪".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(4173)
}

/// 数据目录：跨平台取用户数据区（不引额外依赖）
fn data_dir() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|_| {
                std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".local").join("share"))
            })
            .unwrap_or_else(|_| std::env::temp_dir())
    }
}
