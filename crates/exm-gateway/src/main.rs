//! Gateway 独立进程入口：exm-gateway [--port 4173]
use exm_core::{config::ExmConfig, Core};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut port: u16 = std::env::var("EXM_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(4173);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--port" || a == "-p" {
            if let Some(v) = args.next() {
                port = v.parse().unwrap_or(port);
            }
        }
    }
    let cfg = ExmConfig::load(std::env::current_dir()?);
    let core = Arc::new(Core::with_config(cfg)?);
    exm_gateway::serve(core, port).await
}
