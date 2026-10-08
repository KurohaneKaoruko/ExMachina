//! 主界面:对话优先的纯原生 UI(egui/eframe,无 webview)。
//! 顶栏 = 交互目标 + 设置入口;左栏 = 会话列表;中央 = 消息流 + 输入;
//! 设置页收敛 LLM 配置 / 桌面行为 / 高级控制台入口。

use crate::{api, desktop, gateway, ws};
use egui::ScrollArea;

pub struct ViewSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

pub struct App {
    pub api: api::Api,
    /// 持有网关子进程(App 析构即级联终止)
    #[allow(dead_code)]
    pub gateway: Option<gateway::Gateway>,
    pub stream: std::sync::Arc<ws::StreamHandle>,
    pub cfg: desktop::DesktopConfig,

    pub is_settings: bool,
    pub target: Option<api::Target>,
    pub sessions: Vec<api::SessionInfo>,
    pub active: Option<String>,
    pub msgs: Vec<api::Msg>,
    pub input: String,
    pub busy: bool,

    pub view_settings: ViewSettings,
    pub saved_at: Option<std::time::Instant>,
    pub status: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, gateway: Option<gateway::Gateway>, base: String, key: String) -> Self {
        install_cjk_fonts(&cc.egui_ctx);
        let api = api::Api::new(base.clone(), key.clone());
        let stream: std::sync::Arc<ws::StreamHandle> = std::sync::Arc::new(ws::StreamHandle::default());
        stream.spawn(cc.egui_ctx.clone(), key);

        let llm = api.llm_config().unwrap_or_default();
        let mut app = App {
            api,
            gateway,
            stream,
            cfg: desktop::DesktopConfig::load(),
            is_settings: false,
            target: None,
            sessions: Vec::new(),
            active: None,
            msgs: Vec::new(),
            input: String::new(),
            busy: false,
            view_settings: ViewSettings { base_url: llm.0, api_key: llm.1, model: llm.2 },
            saved_at: None,
            status: format!("网关 {base}"),
        };
        app.refresh_all();
        app
    }

    pub fn refresh_all(&mut self) {
        self.target = self.api.target().ok();
        if let Ok(list) = self.api.sessions() {
            self.active = self.active.clone().or_else(|| list.first().map(|s| s.id.clone()));
            self.sessions = list;
        }
        self.refresh_msgs();
    }

    pub fn refresh_msgs(&mut self) {
        if let Some(id) = &self.active {
            if let Ok(m) = self.api.messages(id) {
                self.msgs = m;
            }
        }
    }

    fn send(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.busy {
            return;
        }
        self.input.clear();
        if self.active.is_none() {
            match self.api.create_session(&text.chars().take(20).collect::<String>()) {
                Ok(id) => {
                    self.active = Some(id);
                    if let Ok(list) = self.api.sessions() {
                        self.sessions = list;
                    }
                }
                Err(e) => {
                    self.status = format!("创建会话失败: {e}");
                    return;
                }
            }
        }
        let sid = self.active.clone().unwrap();
        let ws_url = format!("{}/ws?sessionId={sid}", self.api.base);
        self.stream.set_target(Some(ws_url));
        self.busy = true;
        self.status = "执行中…".into();
        if let Err(e) = self.api.chat(&sid, &text) {
            self.busy = false;
            self.status = format!("发送失败: {e}");
        }
    }

    fn open_console(&mut self) {
        let url = format!("{}/", self.api.base);
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(&url).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        self.status = format!("高级控制台已在浏览器打开:{url}");
    }

    fn pump_stream(&mut self, ctx: &egui::Context) {
        let shared = self.stream.shared();
        let (finished, error) = {
            let st = shared.lock().unwrap();
            (st.finished, st.error.clone())
        };
        if finished {
            shared.lock().unwrap().finished = false;
            self.busy = false;
            self.status = "就绪".into();
            self.refresh_msgs();
            ctx.request_repaint();
        } else if let Some(err) = error {
            shared.lock().unwrap().error = None;
            self.busy = false;
            self.status = format!("本轮出错:{err}");
            self.refresh_msgs();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 关窗驻留:拦截关闭,仅隐藏窗口(可配置完全退出)
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.cfg.exit_on_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.pump_stream(ctx);

        egui::TopBottomPanel::top("topbar").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("EXMACHINA").strong().size(15.0));
                ui.separator();
                if let Some(t) = &self.target {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {}",
                            t.name,
                            if t.mode == "single" { "单体" } else { "智能连结" }
                        ))
                        .weak(),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙ 设置").clicked() {
                        self.is_settings = !self.is_settings;
                    }
                    if ui.button("🌐 控制台").on_hover_text("浏览器打开高级控制台").clicked() {
                        self.open_console();
                    }
                    if ui.button("⟳").on_hover_text("刷新").clicked() {
                        self.refresh_all();
                    }
                    let connected = self.stream.shared().lock().unwrap().connected;
                    let (dot, color) = if connected {
                        ("●", egui::Color32::from_rgb(80, 200, 120))
                    } else {
                        ("○", egui::Color32::from_rgb(160, 160, 160))
                    };
                    ui.label(egui::RichText::new(dot).color(color));
                });
            });
            ui.add_space(4.0);
        });

        if self.is_settings {
            self.ui_settings(ctx);
        } else {
            self.ui_chat(ctx);
        }

        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.status).weak().small());
            });
        });
    }

}

impl App {
    fn ui_chat(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sessions").resizable(false).default_width(210.0).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("会话").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("+ 新会话").clicked() {
                        self.active = None;
                        self.msgs.clear();
                        self.stream.set_target(None);
                    }
                });
            });
            ui.separator();
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let sessions = self.sessions.clone();
                for s in &sessions {
                    let selected = self.active.as_deref() == Some(s.id.as_str());
                    let label = if s.preview.is_empty() {
                        s.title.clone()
                    } else {
                        format!("{}\n{}", s.title, s.preview)
                    };
                    if ui.selectable_label(selected, egui::RichText::new(label).small()).clicked() {
                        self.active = Some(s.id.clone());
                        self.refresh_msgs();
                        let ws_url = format!("{}/ws?sessionId={}", self.api.base, s.id);
                        self.stream.set_target(Some(ws_url));
                    }
                    ui.add_space(2.0);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let shared = self.stream.shared();
            let (stream_text, stream_thinking, error, connected) = {
                let st = shared.lock().unwrap();
                (st.text.clone(), st.thinking.clone(), st.error.clone(), st.connected)
            };
            let _ = connected;

            ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
                ui.add_space(6.0);
                if self.msgs.is_empty() && stream_text.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(90.0);
                        ui.label(egui::RichText::new("EXMACHINA").size(30.0).weak());
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("开始你的第一条消息").weak());
                        ui.add_space(90.0);
                    });
                    return;
                }
                for m in &self.msgs {
                    let is_user = m.role == "user";
                    ui.add_space(4.0);
                    egui::Frame::default()
                        .fill(if is_user {
                            egui::Color32::from_rgb(38, 46, 58)
                        } else {
                            egui::Color32::from_rgb(30, 34, 40)
                        })
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .rounding(6.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width() - 8.0);
                            ui.label(
                                egui::RichText::new(if is_user { "你" } else { "EXMACHINA" })
                                    .small()
                                    .strong()
                                    .color(if is_user {
                                        egui::Color32::from_rgb(140, 180, 240)
                                    } else {
                                        egui::Color32::from_rgb(120, 200, 150)
                                    }),
                            );
                            if let Some(t) = &m.thinking {
                                if !t.trim().is_empty() {
                                    ui.collapsing(egui::RichText::new("▸ 思考过程").small().weak(), |ui| {
                                        ui.label(egui::RichText::new(t).small().weak());
                                    });
                                }
                            }
                            if m.tool_calls > 0 {
                                ui.label(
                                    egui::RichText::new(format!("▸ 已执行 {} 次工具调用", m.tool_calls))
                                        .small()
                                        .weak(),
                                );
                            }
                            ui.label(&m.text);
                        });
                }
                if !stream_text.is_empty() {
                    ui.add_space(4.0);
                    egui::Frame::default()
                        .fill(egui::Color32::from_rgb(30, 34, 40))
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .rounding(6.0)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("EXMACHINA")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::from_rgb(120, 200, 150)),
                            );
                            if !stream_thinking.trim().is_empty() {
                                ui.collapsing(egui::RichText::new("▸ 思考过程").small().weak(), |ui| {
                                    ui.label(egui::RichText::new(&stream_thinking).small().weak());
                                });
                            }
                            ui.label(&stream_text);
                        });
                }
                if self.busy && stream_text.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("思考中…").weak());
                }
            });

            if let Some(err) = &error {
                ui.separator();
                ui.label(egui::RichText::new(format!("⚠ {err}")).color(egui::Color32::from_rgb(240, 140, 120)));
            }

            ui.separator();
            ui.horizontal(|ui| {
                let enter_sent = {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .hint_text("输入消息,Enter 发送")
                            .desired_width(ui.available_width() - 130.0),
                    );
                    resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter))
                };
                if enter_sent {
                    self.send();
                }
                if ui.add_enabled(!self.busy, egui::Button::new("发送")).clicked() {
                    self.send();
                }
                if ui.add_enabled(self.busy, egui::Button::new("■ 停止")).clicked() {
                    if let Some(id) = &self.active {
                        let _ = self.api.stop_session(id);
                        self.busy = false;
                        self.status = "已请求停止".into();
                    }
                }
            });
        });
    }

    fn ui_settings(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading("设置");
            ui.add_space(6.0);

            ui.group(|ui| {
                ui.label(egui::RichText::new("模型通道(LLM)").strong());
                ui.add_space(4.0);
                egui::Grid::new("llm-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Base URL(OpenAI 兼容)");
                    ui.add(egui::TextEdit::singleline(&mut self.view_settings.base_url).desired_width(380.0));
                    ui.end_row();
                    ui.label("API Key");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.view_settings.api_key)
                            .password(true)
                            .desired_width(380.0),
                    );
                    ui.end_row();
                    ui.label("默认模型");
                    ui.add(egui::TextEdit::singleline(&mut self.view_settings.model).desired_width(380.0));
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("保存模型配置").clicked() {
                        match self.api.save_llm(
                            &self.view_settings.base_url,
                            &self.view_settings.api_key,
                            &self.view_settings.model,
                        ) {
                            Ok(()) => {
                                self.saved_at = Some(std::time::Instant::now());
                                self.status = "模型配置已保存".into();
                            }
                            Err(e) => self.status = format!("保存失败: {e}"),
                        }
                    }
                    if ui.button("浏览器打开高级控制台").clicked() {
                        self.open_console();
                    }
                });
                if let Some(t) = self.saved_at {
                    if t.elapsed().as_secs() < 3 {
                        ui.label(egui::RichText::new("✓ 已保存").color(egui::Color32::from_rgb(120, 200, 150)).small());
                    }
                }
            });

            ui.add_space(10.0);
            ui.group(|ui| {
                ui.label(egui::RichText::new("桌面行为").strong());
                ui.add_space(4.0);
                ui.checkbox(&mut self.cfg.exit_on_close, "关闭窗口即完全退出(否则驻留后台,网关持续运行)");
                if ui.button("保存桌面行为").clicked() {
                    match self.cfg.save() {
                        Ok(()) => self.status = "桌面行为已保存".into(),
                        Err(e) => self.status = format!("保存失败: {e}"),
                    }
                }
            });

            ui.add_space(10.0);
            ui.group(|ui| {
                ui.label(egui::RichText::new("关于").strong());
                ui.add_space(4.0);
                ui.label(format!("EXMACHINA 桌面端 v{}", env!("CARGO_PKG_VERSION")));
                ui.label(
                    egui::RichText::new(format!("数据目录:{}", desktop::data_dir().display())).weak().small(),
                );
            });
        });
    }
}

/// 安装 CJK 字体(运行时发现系统字体,避免 40MB 字体捆绑)
fn install_cjk_fonts(ctx: &egui::Context) {
    let candidates: &[&str] = &[
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJKsc-Regular.otf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "cjk".into(),
                egui::FontData::from_owned(bytes).into(),
            );
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.push("cjk".into());
            }
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                family.push("cjk".into());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
    // 兜底:fc-list 找任意 CJK 字体(Linux 极简系统)
    if let Ok(out) = std::process::Command::new("fc-list").args([":lang=zh", "file"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = text.lines().next() {
            let path = line.split(':').next().unwrap_or_default().trim().to_string();
            if let Ok(bytes) = std::fs::read(&path) {
                let mut fonts = egui::FontDefinitions::default();
                fonts.font_data.insert("cjk".into(), egui::FontData::from_owned(bytes).into());
                if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                    family.push("cjk".into());
                }
                ctx.set_fonts(fonts);
            }
        }
    }
}
