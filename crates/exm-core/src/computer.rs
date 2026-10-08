//! Computer Use —— 截屏 + 鼠标 / 键盘控制本机桌面（docs/协议与契约.md 工具面）。
//!
//! 设计立场：
//! - **默认关闭**：把整机输入权交给模型必须是显式决定（`computer.enabled` / env `EXM_COMPUTER_USE=1`）。
//! - 本模块只做**机械层**：截屏、移动、点击、键入、滚动、热键。审批闸门、动作间隔、
//!   截图落盘与回灌都在 `tools.rs` 的工具管线里——安全策略集中一处。
//! - 输入用 enigo（Rust 原生，无 C 依赖），截屏用 xcap；截图以 PNG 落盘
//!   `.exmachina/screenshots/`，由 runtime 在目标模型具备视觉能力时作为图像输入回灌。
//! - 坐标口径：**物理像素**（与截屏一致）；系统缩放时用户在截图上看到的坐标即物理像素
//!   坐标，模型换算无需 DPI 补偿（macOS Retina 为点阵坐标，见 docs 已知边界）。

use anyhow::Context;
use std::time::Instant;

/// 一次截屏的产物：落盘路径 + 描述信息
pub struct Screenshot {
    /// 落盘的 PNG 绝对路径（相对化由调用方处理）
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// 显示器序号与名称（人读）
    pub monitor: String,
}

/// 机械执行器：持动作节流时间戳。
/// enigo 句柄不持久化——macOS 后端含非线程安全的 CGEventSource 句柄,
/// 持有它会让整个 ToolGateway 不满足 Send;改为每次操作就地创建(交互操作开销可忽略)。
pub struct ComputerSession {
    last_action: Option<Instant>,
}

impl ComputerSession {
    pub fn new() -> anyhow::Result<Self> {
        Ok(ComputerSession { last_action: None })
    }

    /// 就地创建 enigo 句柄(操作内用完即弃)
    fn enigo() -> anyhow::Result<enigo::Enigo> {
        enigo::Enigo::new(&enigo::Settings::default())
            .context("初始化键鼠控制失败（无交互桌面会话？）")
    }

    /// 动作节流：距上次输入动作不足 `interval_ms` 时等待补齐（防失控连点）
    pub fn throttle(&mut self, interval_ms: u64) {
        if interval_ms == 0 {
            return;
        }
        if let Some(t) = self.last_action {
            let elapsed = t.elapsed().as_millis() as u64;
            if elapsed < interval_ms {
                std::thread::sleep(std::time::Duration::from_millis(interval_ms - elapsed));
            }
        }
        self.last_action = Some(Instant::now());
    }

    /// 全屏截取指定显示器（`index` 从 0），必要时等比缩放后落盘 PNG
    pub fn screenshot(
        &mut self,
        index: usize,
        max_edge: u32,
        out_dir: &std::path::Path,
        name: &str,
    ) -> anyhow::Result<Screenshot> {
        let monitors = xcap::Monitor::all().context("枚举显示器失败")?;
        anyhow::ensure!(!monitors.is_empty(), "未检测到可用显示器");
        let idx = index.min(monitors.len() - 1);
        let mon = &monitors[idx];
        let img = mon.capture_image().context("截屏失败（无交互桌面会话？）")?;
        let (w, h) = (img.width(), img.height());

        // 等比缩放：视觉模型侧控制 token 消耗
        let scaled = if max_edge > 0 && w.max(h) > max_edge {
            let ratio = max_edge as f64 / w.max(h) as f64;
            let nw = ((w as f64) * ratio).round().max(1.0) as u32;
            let nh = ((h as f64) * ratio).round().max(1.0) as u32;
            image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
        } else {
            img
        };

        let _ = std::fs::create_dir_all(out_dir);
        let file = out_dir.join(format!("{name}.png"));
        scaled.save_with_format(&file, image::ImageFormat::Png).context("截图写盘失败")?;
        Ok(Screenshot {
            path: file.display().to_string(),
            width: scaled.width(),
            height: scaled.height(),
            monitor: format!("#{} {}（原始 {}x{}）", idx, mon.name().unwrap_or_default(), w, h),
        })
    }

    /// 移动鼠标到物理像素坐标（绝对坐标）
    pub fn move_mouse(&mut self, x: i32, y: i32) -> anyhow::Result<()> {
        use enigo::{Coordinate, Mouse as _};
        let x = x.clamp(-9_000_000, 9_000_000);
        let y = y.clamp(-9_000_000, 9_000_000);
        let mut enigo = Self::enigo()?;
        enigo.move_mouse(x, y, Coordinate::Abs)?;
        Ok(())
    }

    /// 点击：button = left | right | middle；double = 双击（两次快速单击）
    pub fn click(&mut self, button: &str, double: bool) -> anyhow::Result<()> {
        use enigo::{Direction, Mouse as _};
        let btn = match button {
            "right" => enigo::Button::Right,
            "middle" => enigo::Button::Middle,
            _ => enigo::Button::Left,
        };
        let mut enigo = Self::enigo()?;
        enigo.button(btn, Direction::Click)?;
        if double {
            enigo.button(btn, Direction::Click)?;
        }
        Ok(())
    }

    /// 滚轮：dy 正 = 向上、负 = 向下；dx 正 = 向右、负 = 向左（格数）
    pub fn scroll(&mut self, dx: i32, dy: i32) -> anyhow::Result<()> {
        use enigo::{Axis, Mouse as _};
        let mut enigo = Self::enigo()?;
        if dy != 0 {
            enigo.scroll(dy.clamp(-50, 50), Axis::Vertical)?;
        }
        if dx != 0 {
            enigo.scroll(dx.clamp(-50, 50), Axis::Horizontal)?;
        }
        Ok(())
    }

    /// 键入文本（含 Unicode）
    pub fn type_text(&mut self, text: &str) -> anyhow::Result<()> {
        use enigo::Keyboard as _;
        anyhow::ensure!(!text.is_empty(), "text 不能为空");
        Self::enigo()?.text(text)?;
        Ok(())
    }

    /// 按键 / 热键："enter"、"ctrl+c"、"ctrl+shift+t"、"f5"、"win"
    pub fn hotkey(&mut self, combo: &str) -> anyhow::Result<()> {
        use enigo::{Direction, Keyboard as _};
        let parts: Vec<String> = combo
            .split('+')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        anyhow::ensure!(!parts.is_empty(), "按键组合不能为空");
        anyhow::ensure!(parts.len() <= 4, "按键组合过长（≤4 键）：{combo}");

        let key_of = |tok: &str| -> anyhow::Result<enigo::Key> {
            Ok(match tok {
                "ctrl" | "control" | "ctrl_l" => enigo::Key::Control,
                "alt" | "option" => enigo::Key::Alt,
                "shift" => enigo::Key::Shift,
                "win" | "super" | "cmd" | "meta" | "command" => enigo::Key::Meta,
                "enter" | "return" => enigo::Key::Return,
                "tab" => enigo::Key::Tab,
                "esc" | "escape" => enigo::Key::Escape,
                "space" => enigo::Key::Space,
                "backspace" => enigo::Key::Backspace,
                "delete" | "del" => enigo::Key::Delete,
                "up" | "uparrow" => enigo::Key::UpArrow,
                "down" | "downarrow" => enigo::Key::DownArrow,
                "left" | "leftarrow" => enigo::Key::LeftArrow,
                "right" | "rightarrow" => enigo::Key::RightArrow,
                "home" => enigo::Key::Home,
                "end" => enigo::Key::End,
                "pageup" => enigo::Key::PageUp,
                "pagedown" => enigo::Key::PageDown,
                "insert" => {
                    #[cfg(target_os = "macos")]
                    {
                        anyhow::bail!("macOS 平台不支持 insert 键（enigo 未提供该枚举）");
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        enigo::Key::Insert
                    }
                }
                f if f.len() >= 2 && f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => {
                    let n: u8 = f[1..].parse().context("无效功能键：{f}")?;
                    anyhow::ensure!((1..=12).contains(&n), "仅支持 F1-F12：{f}");
                    match n {
                        1 => enigo::Key::F1,
                        2 => enigo::Key::F2,
                        3 => enigo::Key::F3,
                        4 => enigo::Key::F4,
                        5 => enigo::Key::F5,
                        6 => enigo::Key::F6,
                        7 => enigo::Key::F7,
                        8 => enigo::Key::F8,
                        9 => enigo::Key::F9,
                        10 => enigo::Key::F10,
                        11 => enigo::Key::F11,
                        _ => enigo::Key::F12,
                    }
                }
                s if s.chars().count() == 1 => {
                    enigo::Key::Unicode(s.chars().next().unwrap().to_ascii_lowercase())
                }
                other => anyhow::bail!("无法识别的按键名：{other}"),
            })
        };

        // 组合键：修饰键按住 → 末键单击 → 修饰键释放；单键即直接单击
        let (modifiers, last) = if parts.len() > 1 {
            (parts[..parts.len() - 1].to_vec(), parts[parts.len() - 1].clone())
        } else {
            (vec![], parts[0].clone())
        };
        let mut enigo = Self::enigo()?;
        let mut held: Vec<enigo::Key> = Vec::new();
        for m in &modifiers {
            let k = key_of(m)?;
            enigo.key(k.clone(), Direction::Press)?;
            held.push(k);
        }
        let last_key = key_of(&last)?;
        enigo.key(last_key, Direction::Click)?;
        for k in held.into_iter().rev() {
            enigo.key(k, Direction::Release)?;
        }
        Ok(())
    }
}

/// 极简 base64 编码（截图 data URL；与 browser.rs 的解码器同风格，避免引入新依赖）
pub fn b64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}
