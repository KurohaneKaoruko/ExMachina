//! `exmachina` 入口 —— 与 `exm` 同一实现、同一行为（docs/07 §2）
fn main() {
    if let Err(e) = exm_cli::run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
