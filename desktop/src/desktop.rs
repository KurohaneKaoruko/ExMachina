//! 桌面配置与数据目录(自 tauri 壳移植,行为兼容)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 桌面行为配置(desktop.json,随应用数据目录存放)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopConfig {
    /// 关窗驻留:关闭主窗口仅隐藏(网关持续运行);true = 关窗即完全退出
    #[serde(default)]
    pub exit_on_close: bool,
    /// 通知开关:审批待办
    #[serde(default = "default_true")]
    pub notify_approval: bool,
    /// 通知开关:定时任务推送
    #[serde(default = "default_true")]
    pub notify_cron: bool,
    /// 通知开关:任务完成
    #[serde(default = "default_true")]
    pub notify_task_done: bool,
}

fn default_true() -> bool {
    true
}

impl Default for DesktopConfig {
    fn default() -> Self {
        DesktopConfig {
            exit_on_close: false,
            notify_approval: true,
            notify_cron: true,
            notify_task_done: true,
        }
    }
}

impl DesktopConfig {
    fn path() -> PathBuf {
        data_dir().join("ExMachina").join("desktop.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }
}

/// 应用数据根:**安装目录/data**(与程序同目录,自包含、可整目录备份迁移)。
/// 安装目录不可写(如 macOS 装进 /Applications)时回退系统数据目录。
pub fn data_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let portable = exe.parent().map(|p| p.join("data")).unwrap_or_else(|| PathBuf::from("data"));
    std::fs::create_dir_all(&portable).ok();
    if writable(&portable) {
        return portable;
    }
    fallback_data_dir()
}

fn writable(dir: &PathBuf) -> bool {
    let probe = dir.join(".write-probe");
    std::fs::write(&probe, b"ok").is_ok_and(|_| std::fs::remove_file(&probe).is_ok())
}

fn fallback_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var("LOCALAPPDATA").map(|v| PathBuf::from(v).join("ExMachina")).unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:".into());
            PathBuf::from(home).join("AppData").join("Local").join("ExMachina")
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            return PathBuf::from(xdg).join("ExMachina");
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".local").join("share").join("ExMachina")
    }
}
