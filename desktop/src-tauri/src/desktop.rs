//! 桌面端配置与事件映射（组 11）
//!
//! - `DesktopConfig`：托盘驻留 / 全局快捷键 / 通知类别开关（`<数据目录>/ExMachina/desktop.json`）
//! - `map_event`：网关 WS 事件 → 原生通知载荷的纯函数映射（事件映射单测覆盖，D11 零新事件通路）

use serde::{Deserialize, Serialize};

/// 桌面端配置：关窗行为 / 全局快捷键 / 通知类别（默认值 = spec 口径）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopConfig {
    /// 关闭主窗口 = 隐藏并驻留托盘（false）；true = 直接完全退出
    pub exit_on_close: bool,
    /// 全局快捷键（唤起 / 收起主窗口；冲突时提示并引导改绑）
    pub shortcut: String,
    /// 通知类别开关
    pub notify_approval: bool,
    pub notify_cron: bool,
    pub notify_task_done: bool,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        DesktopConfig {
            exit_on_close: false, // 关窗驻留默认开（spec）
            shortcut: "Alt+Shift+E".into(),
            notify_approval: true,
            notify_cron: true,
            notify_task_done: true,
        }
    }
}

impl DesktopConfig {
    fn path() -> std::path::PathBuf {
        data_dir().join("ExMachina").join("desktop.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
}

/// 数据目录：跨平台取用户数据区（不引额外依赖）
pub fn data_dir() -> std::path::PathBuf {
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

// ---------------------------------------------------------------- 事件映射（11.2）

/// 原生通知载荷（由网关事件映射而来）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notif {
    /// 通知标题（含类别前缀，用户可辨识）
    pub title: String,
    /// 通知正文
    pub body: String,
    /// 类别：approval | cron | task_done（点击定位与类别开关的键）
    pub category: &'static str,
}

/// 网关 WS 事件 → 原生通知（类别关闭 = None；无关事件 = None）。
/// 覆盖三类：审批请求（approval.required）、定时任务推送（cron.finished）、任务完成（run.finished）。
pub fn map_event(kind: &str, payload: &serde_json::Value, cfg: &DesktopConfig) -> Option<Notif> {
    let s = |k: &str| payload.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match kind {
        "approval.required" => {
            if !cfg.notify_approval {
                return None;
            }
            let agent = s("agentId");
            let command = s("command");
            Some(Notif {
                title: "审批请求待处理".into(),
                body: if command.is_empty() {
                    format!("{agent} 发起了一个需要批准的命令")
                } else {
                    format!("{agent}: {command}")
                },
                category: "approval",
            })
        }
        "cron.finished" => {
            if !cfg.notify_cron {
                return None;
            }
            let name = s("jobName");
            let status = s("status");
            let summary = s("summary");
            let mark = if status == "done" { "完成" } else { "失败" };
            Some(Notif {
                title: format!("定时任务{mark}：{name}"),
                body: if summary.is_empty() { status } else { summary },
                category: "cron",
            })
        }
        "run.finished" => {
            if !cfg.notify_task_done {
                return None;
            }
            // 收束陈述首条（截断，防长正文撑爆系统通知）
            let text = payload
                .get("statements")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|st| st.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("本轮任务已收束")
                .chars()
                .take(120)
                .collect::<String>();
            Some(Notif {
                title: "任务完成".into(),
                body: text,
                category: "task_done",
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 审批事件映射为审批通知() {
        let cfg = DesktopConfig::default();
        let n = map_event("approval.required", &json!({ "agentId": "coding-agent", "command": "cargo publish" }), &cfg).unwrap();
        assert_eq!(n.category, "approval");
        assert!(n.body.contains("coding-agent") && n.body.contains("cargo publish"));
        // 类别开关关闭 → 不通知
        let off = DesktopConfig { notify_approval: false, ..Default::default() };
        assert!(map_event("approval.required", &json!({}), &off).is_none(), "类别开关生效");
    }

    #[test]
    fn 定时任务与任务完成映射() {
        let cfg = DesktopConfig::default();
        let n = map_event("cron.finished", &json!({ "jobName": "每日报表", "status": "failed", "summary": "模型未配置" }), &cfg).unwrap();
        assert_eq!(n.category, "cron");
        assert!(n.title.contains("每日报表") && n.title.contains("失败"));
        assert!(n.body.contains("模型未配置"));

        let n = map_event(
            "run.finished",
            &json!({ "statements": [{ "tag": "报告", "text": "重构完成，全部测试通过" }] }),
            &cfg,
        )
        .unwrap();
        assert_eq!(n.category, "task_done");
        assert!(n.body.contains("全部测试通过"));

        // 任务完成开关关闭 → 不通知；审批不受影响
        let off_done = DesktopConfig { notify_task_done: false, ..Default::default() };
        assert!(map_event("run.finished", &json!({ "statements": [] }), &off_done).is_none());
        assert!(map_event("approval.required", &json!({}), &off_done).is_some(), "类别开关互不影响");
    }

    #[test]
    fn 无关事件与超长正文() {
        let cfg = DesktopConfig::default();
        assert!(map_event("graph.updated", &json!({}), &cfg).is_none(), "无关事件不映射");
        assert!(map_event("tool.call", &json!({}), &cfg).is_none());
        let long: String = "长".repeat(500);
        let n = map_event("run.finished", &json!({ "statements": [{ "text": long }] }), &cfg).unwrap();
        assert!(n.body.chars().count() <= 120, "正文封顶: {}", n.body.chars().count());
    }
}
