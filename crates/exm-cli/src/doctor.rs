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
        match std::fs::read_dir(cfg.agents_dir.join("definitions")) {
            Ok(entries) => {
                let n = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                    .count();
                if n >= 13 {
                    Ok(format!("{n} 份个体定义"))
                } else {
                    Err(format!("仅 {n} 份定义，期望 ≥42"))
                }
            }
            Err(_) => Err(format!("缺少目录 {}", cfg.agents_dir.join("definitions").display())),
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
                "未配置模型提供商 —— 请在「模型提供商」页填写端点与 API Key{profile_note}"
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
