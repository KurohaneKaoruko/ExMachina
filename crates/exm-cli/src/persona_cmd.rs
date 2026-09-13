//! `exm persona` —— 智能体人设（说话风格）管理

use crate::render::*;
use exm_core::Core;
use owo_colors::OwoColorize;
use std::path::PathBuf;

pub fn run_get(core: &Core, identifier: &str) -> anyhow::Result<()> {
    let persona = core.persona(identifier)?;
    let custom = core.persona_is_custom(identifier)?;
    let tag = if custom {
        format!("({})", "自定义人设").yellow().to_string()
    } else {
        format!("({})", "默认智械体风格").bright_black().to_string()
    };
    println!("{} {}", identifier.cyan(), tag);
    println!("{}", persona);
    Ok(())
}

pub fn run_set(
    core: &Core,
    identifier: &str,
    text: Option<String>,
    file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let content = match (text, file) {
        (Some(t), _) => t,
        (None, Some(f)) => std::fs::read_to_string(&f)?,
        (None, None) => anyhow::bail!("请用 --text <内容> 或 --file <路径> 提供人设内容"),
    };
    core.set_persona(identifier, &content)?;
    println!("{} 个体 {identifier} 的人设已更新（下一次派发热生效）", ok("完成"));
    println!("{}", dim("查看：exm persona get <identifier>｜恢复默认：exm persona reset <identifier>"));
    Ok(())
}

pub fn run_reset(core: &Core, identifier: &str) -> anyhow::Result<()> {
    let removed = core.reset_persona(identifier)?;
    if removed {
        println!("{} 已重置为默认智械体风格", ok("完成"));
    } else {
        println!("{}", dim("该个体本就是默认人设，无需重置"));
    }
    Ok(())
}
