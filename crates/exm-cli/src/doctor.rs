//! `exm doctor` —— 体检：配置 / 编成 / 记忆 / 通道 / WebUI

use crate::render::*;
use exm_core::config::{config_schema, ExmConfig, CONFIG_VERSION};
use exm_core::Core;
use owo_colors::OwoColorize;
use std::path::Path;

struct Doctor {
    pass: usize,
    warn: usize,
    fail: usize,
}

impl Doctor {
    fn new() -> Self {
        Doctor { pass: 0, warn: 0, fail: 0 }
    }
    fn check(&mut self, name: &str, result: Result<String, String>) {
        match result {
            Ok(msg) => {
                self.pass += 1;
                println!("{} {} {}", ok("✓"), name, dim(msg));
            }
            Err(msg) => {
                self.fail += 1;
                println!("{} {} {}", err("✗"), name, msg);
            }
        }
    }
    fn soft(&mut self, name: &str, result: Result<String, String>) {
        match result {
            Ok(msg) => {
                self.pass += 1;
                println!("{} {} {}", ok("✓"), name, dim(msg));
            }
            Err(msg) => {
                self.warn += 1;
                println!("{} {} {}", warn("!"), name, msg);
            }
        }
    }
}

/// 开/关展示口径
fn on_off(on: bool) -> &'static str {
    if on { "开" } else { "关" }
}

pub fn run_doctor(workspace_root: &Path) -> anyhow::Result<()> {
    println!("{}", "EXMACHINA 体检".on_blue().white().to_string());
    let mut d = Doctor::new();
    let cfg = ExmConfig::load(workspace_root);

    d.check(
        "运行配置",
        if cfg.config_path.exists() {
            Ok(format!("{}（configVersion {}）", cfg.config_path.display(), cfg.config_version))
        } else {
            Err("未找到 config.json —— 请先运行 `exm install`".into())
        },
    );

    d.soft(
        "配置结构版本",
        if cfg.config_version == CONFIG_VERSION {
            Ok(format!("v{}（最新）", cfg.config_version))
        } else {
            Err(format!(
                "v{} 低于期望 v{}（下次加载自动迁移）",
                cfg.config_version, CONFIG_VERSION
            ))
        },
    );

    d.check(
        "编成数据",
        match std::fs::read_dir(cfg.agents_dir.join("groups")) {
            Ok(entries) => {
                // 组目录制：groups/<gid>/agents/*.json（默认组 + 自定义组）
                let n = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().join("group.json").is_file())
                    .map(|e| {
                        std::fs::read_dir(e.path().join("agents"))
                            .map(|list| {
                                list.filter_map(|x| x.ok())
                                    .filter(|x| x.path().extension().map(|y| y == "json").unwrap_or(false))
                                    .count()
                            })
                            .unwrap_or(0)
                    })
                    .sum::<usize>();
                if n >= 13 {
                    Ok(format!("{n} 份成员定义"))
                } else {
                    Err(format!("仅 {n} 份成员定义，期望 ≥42"))
                }
            }
            Err(_) => Err(format!("缺少目录 {}", cfg.agents_dir.join("groups").display())),
        },
    );

    let core_result = Core::with_config(cfg.clone());
    match &core_result {
        Ok(core) => d.check(
            "运行时装配",
            Ok(format!(
                "1 指挥体 + {} 子个体｜链路模板 {} 份",
                core.registry.units().len(),
                core.playbooks().map(|p| p.len()).unwrap_or(0)
            )),
        ),
        Err(e) => d.check("运行时装配", Err(format!("失败：{e}"))),
    }

    match &core_result {
        Ok(core) => match core.memory_stats() {
            Ok(s) => d.check(
                "深层记忆库",
                Ok(format!("{} 条记忆 / {} 词项（{}）", s["total"], s["terms"], cfg.data_dir.display())),
            ),
            Err(e) => d.check("深层记忆库", Err(format!("查询失败：{e}"))),
        },
        Err(e) => d.check("深层记忆库", Err(format!("无法打开：{e}"))),
    }

    d.soft(
        "基础记忆文件",
        if cfg.memory_md_path.exists() {
            Ok(format!("{}", cfg.memory_md_path.display()))
        } else {
            Err("尚未生成（`exm memory render` 或任一任务后自动生成）".into())
        },
    );

    match &core_result {
        Ok(core) => match core.memory_recall("记忆 偏好 决策", Some(3)) {
            Ok(hits) => d.soft("记忆召回自检", Ok(format!("召回 {} 条", hits.len()))),
            Err(e) => d.soft("记忆召回自检", Err(format!("不可用：{e}"))),
        },
        Err(e) => d.soft("记忆召回自检", Err(format!("不可用：{e}"))),
    }

    let profile_note = format!(
        "（模型档案 {} 个，生效 {}）",
        cfg.llm_profiles.len(),
        cfg.active_profile
    );
    let configured = !cfg.llm.api_key.trim().is_empty()
        || !cfg.llm.api_keys.is_empty()
        || cfg.llm_profiles.iter().any(|p| !p.api_key.trim().is_empty() || !p.api_keys.is_empty());
    d.soft(
        "LLM 通道",
        if cfg.use_mock {
            Err(format!("测试替身通道（EXM_LLM_MOCK=1）{profile_note}"))
        } else if !configured {
            Err(format!(
                "未配置提供商 —— 请在「提供商」页填写端点与 API Key{profile_note}"
            ))
        } else {
            match crate::install::probe_llm(&cfg) {
                Ok(msg) => Ok(msg),
                Err(e) => Err(format!("端点不可达：{e}")),
            }
        },
    );

    d.soft(
        "WebUI 构建产物",
        if cfg.webui_dist.join("index.html").exists() {
            Ok(format!("{}", cfg.webui_dist.display()))
        } else {
            Err("未构建（网关仍提供 API；可选执行 npm run build:webui）".into())
        },
    );

    d.soft(
        "配置 Schema",
        Ok(format!(
            "{} 个分组（CLI 向导与 WebUI 设置页共用）",
            config_schema()["groups"].as_array().map(|a| a.len()).unwrap_or(0)
        )),
    );

    // ---- 新增配置段诊断（组 12.2：异常配置可检出） ----

    // 群聊唤醒门控：升级迁移默认关（保持旧行为）——提示口径，不判失败
    d.soft(
        "群聊唤醒门控",
        if cfg.identity.group_gate_default {
            Ok("新通道群内仅 @提及 / 回复触发（新装默认）".to_string())
        } else {
            Err("已关闭（升级迁移保持旧行为：群内任意消息触发；如需门控请到设置页开启）".to_string())
        },
    );

    // 事件触发器：去抖窗口与 webhook 密钥口径
    d.check(
        "事件触发器",
        if cfg.triggers.file_watch.enabled && cfg.triggers.file_watch.debounce_ms < 50 {
            Err(format!("去抖窗口过小（{}ms，下限 50）：易触发风暴", cfg.triggers.file_watch.debounce_ms))
        } else if cfg.triggers.event_webhook.enabled && cfg.triggers.event_webhook.secret.trim().is_empty() {
            Err("事件 webhook 已启用但密钥为空：入口不可用，请配置签名密钥或关闭".to_string())
        } else {
            let fw = cfg.triggers.file_watch.enabled as usize;
            let ew = cfg.triggers.event_webhook.enabled as usize;
            Ok(format!(
                "文件监听 {} / 事件 webhook {}（排除 {}）",
                on_off(fw == 1),
                on_off(ew == 1),
                cfg.triggers.file_watch.exclude.join(", ")
            ))
        },
    );

    // 心跳：目标个体必须存在（per-agent 粒度）
    match &core_result {
        Ok(core) => match cfg.automation.heartbeat_agent.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(agent) => {
                let known = core
                    .registry()
                    .list_groups()
                    .into_iter()
                    .any(|g| core.registry().agents_in_group(&g.id).iter().any(|a| a.identifier == agent));
                d.check(
                    "心跳目标个体",
                    if known {
                        Ok(format!("{agent}（以其所属组巡检，产出进其组记忆）"))
                    } else {
                        Err(format!("个体 {agent} 不在任何组：将回落全局心跳，请修正 automation.heartbeatAgent"))
                    },
                );
            }
            None => d.check("心跳目标个体", Ok("未配置（全局心跳，激活组执行）".into())),
        },
        Err(_) => d.soft("心跳目标个体", Err("运行时未装配，跳过".into())),
    }

    // 用量治理：窗口与配额口径
    d.check(
        "用量限制",
        if !cfg.limits.enabled {
            Ok("未启用（外部入口不限流）".into())
        } else if cfg.limits.enabled && cfg.limits.max_requests == 0 && cfg.limits.quota_tokens == 0 && cfg.limits.quota_requests == 0 {
            Err("已启用但窗口请求数与周期配额均为 0（不限）：等于未生效，请调整阈值或关闭".to_string())
        } else if !matches!(cfg.limits.quota_period.as_str(), "daily" | "weekly" | "monthly") {
            Err(format!("配额周期非法：{}（daily | weekly | monthly）", cfg.limits.quota_period))
        } else if !matches!(cfg.limits.quota_running_policy.as_str(), "finish" | "interrupt") {
            Err(format!("配额耗尽策略非法：{}（finish | interrupt）", cfg.limits.quota_running_policy))
        } else {
            Ok(format!(
                "窗口 {}s / {} 次，周期 {}（token {}、请求 {}）",
                cfg.limits.window_secs, cfg.limits.max_requests, cfg.limits.quota_period, cfg.limits.quota_tokens, cfg.limits.quota_requests
            ))
        },
    );

    // MCP 服务端：暴露面提示（默认关闭；开启且 ACL 全空 = 全量暴露）
    d.soft(
        "MCP 服务端",
        if !cfg.mcp_serve.enabled {
            Ok("未启用（工具面不对外暴露）".into())
        } else if cfg.mcp_serve.allowed_groups.is_empty() && cfg.mcp_serve.allowed_tools.is_empty() {
            Err(format!(
                "已启用且未设 ACL：全部组、全部启用工具对外可见（路径 {}）——建议收窄 allowedGroups / allowedTools",
                cfg.mcp_serve.http_path
            ))
        } else {
            Ok(format!(
                "已启用（路径 {}，组限定 {} 个，工具限定 {} 个）",
                cfg.mcp_serve.http_path,
                cfg.mcp_serve.allowed_groups.len(),
                cfg.mcp_serve.allowed_tools.len()
            ))
        },
    );

    // 结果推送：总开关与摘要长度
    d.check(
        "定时任务结果推送",
        if cfg.notify.max_chars < 50 {
            Err(format!("推送摘要长度过小（{} < 50）：摘要将被过度截断", cfg.notify.max_chars))
        } else {
            Ok(format!("总开关 {}，摘要上限 {} 字符（逐任务在 notifyChannels 订阅）", on_off(cfg.notify.enabled), cfg.notify.max_chars))
        },
    );

    println!();
    println!(
        "{} 通过 {}｜警告 {}｜失败 {}",
        "体检汇总".bold(),
        d.pass.to_string().green(),
        d.warn.to_string().yellow(),
        d.fail.to_string().bright_red()
    );
    if d.fail > 0 {
        anyhow::bail!("存在 {} 项失败，请先修复", d.fail);
    }
    Ok(())
}
