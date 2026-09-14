//! `exm install` —— 首启安装向导（docs/07 §3）
//! 安装向导：一条命令完成 环境检查 → 交互式配置 → 连接自检 → 落盘 → 下一步指引

use crate::render::*;
use exm_core::config::{InstallChannels, InstallFeatures, InstallManifest, LlmConfig, ExmConfig};
use owo_colors::OwoColorize;
use std::io::{BufRead, Write};

fn ask(prompt: &str, default: &str) -> String {
    print!("{} {}", prompt.yellow(), dim(format!("[{default}]")));
    std::io::stdout().flush().ok();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    let t = line.trim();
    if t.is_empty() {
        default.to_string()
    } else {
        t.to_string()
    }
}

fn ask_yes(prompt: &str, default_yes: bool) -> bool {
    let d = if default_yes { "Y/n" } else { "y/N" };
    let s = ask(prompt, d);
    match s.to_lowercase().as_str() {
        "y" | "yes" | "是" => true,
        "n" | "no" | "否" => false,
        "y/n" => default_yes,
        _ => default_yes,
    }
}

pub fn run_install(workspace_root: &std::path::Path, quick: bool) -> anyhow::Result<()> {
    println!("{}", "EXMACHINA 安装向导".on_blue().white().to_string());
    println!("{}", dim("本向导将完成：环境检查 → 模型接入 → 运行时参数 → 落盘与自检"));
    println!();

    // 1) 环境检查
    let mut notes: Vec<String> = Vec::new();
    let agents_dir = workspace_root.join("agents");
    if agents_dir.join("definitions").exists() {
        let n = std::fs::read_dir(agents_dir.join("definitions"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .count();
        println!("{} 编成数据：{} 份个体定义（{}）", ok("✓"), n, agents_dir.display());
    } else {
        println!("{} 未找到 agents/definitions（将无法装载个体，请检查工作目录）", err("✗"));
        notes.push(format!("缺少编成目录：{}", agents_dir.display()));
    }

    // 2) 模型接入
    let mut cfg = ExmConfig::load(workspace_root);
    let base_url = ask("LLM Base URL（OpenAI 兼容）", &cfg.llm.base_url);
    let api_key = ask("API Key（可留空，稍后在「提供商」页补配）", "");
    let orch_model = ask("指挥体模型", &cfg.llm.orch_model);
    let unit_model = ask("子个体模型", &cfg.llm.unit_model);
    let concurrency = ask("子个体并发数", &cfg.max_concurrency.to_string())
        .parse::<usize>()
        .unwrap_or(cfg.max_concurrency);
    let memory = ask_yes("启用记忆系统（可检索深层记忆 + memory.md 基础记忆）", true);

    cfg.llm = LlmConfig {
            api_keys: Vec::new(),
        api_format: String::new(), base_url, api_key, orch_model, unit_model };
    cfg.max_concurrency = concurrency.clamp(1, 32);
    cfg.memory_enabled = memory;
    cfg.use_mock = cfg.use_mock || exm_core::config::mock_enabled_from_env();

    // 3) 落盘
    cfg.save()?;
    let manifest = InstallManifest {
        profile: if quick { "quick".into() } else { "custom".into() },
        channels: InstallChannels { cli: true, gateway: true, gateway_port: 4173, webui: true },
        features: InstallFeatures { memory, tools: true },
        notes: notes.clone(),
        ..Default::default()
    };
    cfg.save_manifest(&manifest)?;

    // 4) 初始化记忆库与基础记忆文件
    let core = exm_core::Core::with_config(cfg.clone())?;
    core.render_memory_md()?;

    // 5) 连接自检（配置了密钥才发探针；未配置则明确提示去配置）
    let configured = !cfg.llm.api_key.trim().is_empty() || !cfg.llm.api_keys.is_empty();
    let mut probe = "跳过（未配置模型）".to_string();
    if configured && !cfg.use_mock {
        probe = match probe_llm(&cfg) {
            Ok(msg) => ok(format!("通过：{msg}")),
            Err(e) => warn(format!("未通过（{e}）—— 可在「提供商」页或 config.json 修正")),
        };
    }

    println!();
    println!("{}", "安装完成".green().bold().to_string());
    println!("  {} {}", dim("运行配置"), cfg.config_path.display());
    println!("  {} {}", dim("安装清单"), cfg.install_manifest_path().display());
    println!("  {} {}", dim("数据目录"), cfg.data_dir.display());
    println!("  {} {}", dim("基础记忆"), cfg.memory_md_path.display());
    println!("  {} {}", dim("LLM 通道"), if configured { cfg.llm.base_url.clone() } else { warn("未配置（对话前需先配置模型端点与密钥）") });
    println!("  {} {}", dim("连接自检"), probe);
    if !configured {
        println!();
        println!("{}", warn("注意：尚未配置模型——对话前请先完成模型接入（下列任一步）"));
        println!("  {}  {}", dim("·"), "exm model add <id> --base-url <url> --api-key <key> --orch-model <m> --unit-model <m>".cyan());
        println!("  {}  {}", dim("·"), "exm serve  # 打开「提供商」页图形化配置".cyan());
    }
    println!();
    println!("{}", "下一步".bold().to_string());
    println!("  {}  {}{}", dim("1."), "exm chat \"分析当前项目的架构风险\"".cyan(), dim("   # 终端直接对话"));
    println!("  {}  {}{}", dim("2."), "exm serve".cyan(), dim("                        # 起网关（WebUI/第三方渠道接入）"));
    println!("  {}  {}{}", dim("3."), "exm doctor".cyan(), dim("                       # 体检：配置/编成/记忆/端口"));
    println!("{}", dim("提示：`exm` 与 `exmachina` 是同一命令的两个入口名，行为完全一致（docs/07 §2）"));
    Ok(())
}

/// 极短 LLM 连通性探针：curl 探测 /models（401/403 也说明端点可达）
pub fn probe_llm(cfg: &ExmConfig) -> Result<String, String> {
    let url = format!("{}/models", cfg.llm.base_url.trim_end_matches('/'));
    let out = std::process::Command::new("curl")
        .args(["-s", "-o", if cfg!(windows) { "NUL" } else { "/dev/null" }, "-w", "%{http_code}", "--max-time", "8", &url])
        .output()
        .map_err(|e| format!("无 curl 可用（{e}）"))?;
    let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if code.starts_with('2') || code == "401" || code == "403" {
        Ok(format!("可达（HTTP {code}）"))
    } else {
        Err(format!("HTTP {code}"))
    }
}
