fn main() {
    // Windows 可执行文件嵌入图标(资源管理器 / 任务栏 / 快捷方式)
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        res.compile().expect("嵌入 Windows 图标失败");
    }
}
