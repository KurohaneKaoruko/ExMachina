//! 本地网关侧车管理:拉起捆绑的 exm-gateway、健康等待、生命周期收口。
//!
//! - Windows 下以 CREATE_NO_WINDOW 拉起,不弹终端
//! - 数据目录:应用数据根/ExMachina(与旧 tauri 壳一致,升级无缝)
//! - 首次运行播种:资源目录里的 entities/skills/default-soul.md 拷入空白数据目录

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub struct Gateway {
    pub base_url: String,
    child: Child,
}

impl Gateway {
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 拉起本地网关(root = 应用数据根/ExMachina;resources = 随应用分发的资源目录)
pub fn spawn(app_exe: &std::path::Path) -> anyhow::Result<Gateway> {
    let gateway = ["exm-gateway.exe", "exm-gateway"]
        .iter()
        .map(|n| app_exe.parent().unwrap_or(app_exe).join(n))
        .find(|p| p.is_file())
        .or_else(|| {
            // 开发兜底:EXM_GATEWAY_BIN 指定
            std::env::var("EXM_GATEWAY_BIN").ok().map(std::path::PathBuf::from)
        })
        .filter(|p| p.is_file())
        .ok_or_else(|| anyhow::anyhow!("未找到捆绑的 exm-gateway(检查应用目录或设置 EXM_GATEWAY_BIN)"))?;

    // 安装目录布局:<install>/data(应用数据) + <install>/resources(随包资源)
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let root = exe_dir.join("data");
    let resources = exe_dir.join("resources");
    seed_if_empty(&root, &resources)?;

    let port = free_port();
    let mut cmd = Command::new(&gateway);
    cmd.arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .env("EXM_DATA_DIR", root.join("data"))
        .env("EXM_WEBUI_DIST", resources.join("webui").join("dist"))
        .env("EXM_AGENTS_DIR", root.join("entities"))
        .env("EXM_LANG", "zh")
        .current_dir(&root);

    #[cfg(windows)]
    {
        // 不弹终端窗口
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd.spawn().map_err(|e| anyhow::anyhow!("网关启动失败: {e}"))?;

    // 就绪等待:TCP 可连即就绪(30s 上限);提前退出立即报错
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut child = child;
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            anyhow::bail!("exm-gateway 进程提前退出(多为端口被占或数据目录异常,详见日志)");
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(Gateway { base_url: format!("http://127.0.0.1:{port}"), child });
        }
        if Instant::now() > deadline {
            child.kill().ok();
            anyhow::bail!("网关 30s 内未就绪");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// 首次运行播种:数据根为空时,把随应用分发的 entities/skills/default-soul.md 拷入,
/// 让开箱即用的 Machina + 智能连结组立即可用(与应用内一键更新/模板机制同源)。
fn seed_if_empty(root: &std::path::Path, resources: &std::path::Path) -> anyhow::Result<()> {
    let entities = root.join("entities");
    if entities.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(root)?;
    let src_entities = resources.join("entities");
    if src_entities.is_dir() {
        copy_dir(&src_entities, &entities)?;
    }
    let src_skills = resources.join("skills");
    if src_skills.is_dir() {
        copy_dir(&src_skills, &root.join("skills"))?;
    }
    for f in ["default-soul.md"] {
        let src = resources.join(f);
        if src.is_file() {
            std::fs::copy(&src, root.join(f))?;
        }
    }
    Ok(())
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let e = entry?;
        let ty = e.file_type()?;
        let target = dst.join(e.file_name());
        if ty.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(4173)
}
