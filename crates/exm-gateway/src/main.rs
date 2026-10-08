//! Gateway 独立进程入口：exm-gateway [--port 4173] [--host 0.0.0.0]
//! 监听地址可通过 `--host` 指定；未指定时读 `EXM_HOST` 环境变量；
//! 仍未指定则默认 `0.0.0.0`（公网可访问）。需要 loopback 时传 `--host 127.0.0.1`。
use exm_core::{config::ExmConfig, Core};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut port: u16 = std::env::var("EXM_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(4173);
    let mut host: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--port" || a == "-p" {
            if let Some(v) = args.next() {
                port = v.parse().unwrap_or(port);
            }
        } else if a == "--host" || a == "-H" {
            if let Some(v) = args.next() {
                host = Some(v);
            }
        }
    }
    let cfg = ExmConfig::load(std::env::current_dir()?);
    let core = Arc::new(Core::with_config(cfg)?);
    exm_gateway::serve(core, port, host.as_deref()).await
}
