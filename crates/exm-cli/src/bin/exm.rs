//! `exm` 入口 —— 日常简洁调用（与 `exmachina` 同实现）
fn main() {
    if let Err(e) = exm_cli::run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
