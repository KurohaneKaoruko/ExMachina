//! EXMACHINA 桌面端:纯原生对话窗口(egui/eframe,无 webview)。
//! 主界面即对话;配置收敛到「设置」页;高级控制台经浏览器打开网关 WebUI。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod desktop;
mod gateway;
mod ws;

fn main() -> eframe::Result {
    // 命令行:--remote <url> --key <key>(直连远程网关);默认拉起本地侧车
    let args: Vec<String> = std::env::args().collect();
    let mut remote: Option<String> = None;
    let mut key = String::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--remote" => remote = args.get(i + 1).cloned(),
            "--key" => key = args.get(i + 1).cloned().unwrap_or_default(),
            _ => {}
        }
        i += 1;
    }

    let (base, gateway) = if let Some(r) = remote {
        (r.trim_end_matches('/').to_string(), None)
    } else {
        let exe = std::env::current_exe().unwrap_or_default();
        let gw = gateway::spawn(&exe).unwrap_or_else(|e| {
            eprintln!("[desktop] {e}");
            std::process::exit(1);
        });
        (gw.base_url.clone(), Some(gw))
    };

    let icon = load_icon(include_bytes!("../icons/icon.png"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([720.0, 480.0])
            .with_icon(std::sync::Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native("EXMACHINA", options, Box::new(|cc| {
        Ok(Box::new(app::App::new(cc, gateway, base, key)))
    }))
}

fn load_icon(png: &[u8]) -> egui::IconData {
    let img = image::load_from_memory(png).expect("内置图标解码失败").to_rgba8();
    let (w, h) = img.dimensions();
    egui::IconData { width: w, height: h, rgba: img.into_raw() }
}
