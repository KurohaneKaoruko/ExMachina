//! 通道适配器共用小件（channel_util.rs）。
//!
//! 两块内容，都是六个适配器（telegram / qqbot / napcat / discord / slack / matrix）
//! 需要同一套语义的地方——各写一遍必然漂移（命令文案、去重窗口、归档命名不一致）：
//!
//! ① 内置命令：`/new` 开新会话、`/status` 查状态。`/stop` 已在 `platform::admit`
//!    最前面统一处理，这里补齐其余两条。调用时机固定为「入站闸门 Allow 之后」：
//!    未绑定 / 被拒用户不可用命令（/new 会建会话，属资源操作，不能绕过身份管控）。
//! ② 消息去重：Discord / QQ 机器人断线 Resume 会重放最近事件；Slack 对同一消息
//!    可能同时投 `message` 与 `app_mention` 双事件。按平台消息 id 做首见放行。

use crate::platform::{status_of, Channel};
use exm_core::Core;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::OnceLock;

// ---------------------------------------------------------------- 内置命令

/// 内置命令识别结果（纯数据，便于单测）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinCmd {
    /// /new：开新会话
    New,
    /// /status：查通道与会话状态
    Status,
}

/// 文本 → 内置命令（纯函数）：只认整词命令（前后可有空白），不认带参数变体——
/// IM 输入法容易在命令后带空格/全角空格，trim 后整词比对最不容易误触发。
pub(crate) fn parse_command(text: &str) -> Option<BuiltinCmd> {
    match text.trim() {
        "/new" => Some(BuiltinCmd::New),
        "/status" => Some(BuiltinCmd::Status),
        _ => None,
    }
}

/// 尝试执行内置命令：命中返回回复文本（由调用方按平台发回），未命中返回 None 走正常消息流。
pub(crate) async fn builtin_command(core: &Arc<Core>, ch: &Channel, chat_key: &str, text: &str) -> Option<String> {
    match parse_command(text) {
        Some(BuiltinCmd::New) => Some(cmd_new(core, ch, chat_key).await),
        Some(BuiltinCmd::Status) => Some(cmd_status(core, ch, chat_key)),
        None => None,
    }
}

/// 通道会话的规范标题（与 `ChannelRun::begin` 同口径——两边必须一字不差，
/// 否则 /new 建的新会话不会被下一轮消息复用）
fn channel_session_title(ch_id: &str, chat_key: &str) -> String {
    format!("channel:{ch_id}:{chat_key}")
}

/// /new：开新会话。
/// 做法：旧会话改名让出规范标题（历史保留、可在控制台回看），随即以规范标题建全新会话，
/// 下一轮入站消息自动落进新会话。与 `ChannelRun::begin` 同口径的组切换/还原。
async fn cmd_new(core: &Arc<Core>, ch: &Channel, chat_key: &str) -> String {
    let prev = core.active_group();
    let mut switched = false;
    if let Some(g) = &ch.group {
        if *g != prev && core.group_meta(g).is_some() {
            switched = core.registry().set_active_group(g).is_ok();
        }
    }
    let title = channel_session_title(&ch.id, chat_key);
    // 旧会话归档：改名（带时间戳防重名），失败则不动旧会话直接报错
    let mut archived: Option<(String, String)> = None;
    let existing = core
        .list_sessions_in_group(&core.active_group())
        .ok()
        .and_then(|list| list.into_iter().find(|s| s.title == title));
    if let Some(old) = existing {
        let stamp = chrono::Utc::now().format("%m%d-%H%M");
        let new_title = format!("{title}·归档{stamp}");
        match core.store.update_session_title(&old.id, &new_title) {
            Ok(_) => archived = Some((old.id, new_title)),
            Err(e) => {
                if switched {
                    let _ = core.registry().set_active_group(&prev);
                }
                return format!("⚠️ 开新会话失败：旧会话改名出错（{e}）。可直接重试。");
            }
        }
    }
    let created = core.create_session(&title);
    if switched {
        let _ = core.registry().set_active_group(&prev);
    }
    match created {
        Ok(_) => match archived {
            Some((_, old_title)) => format!("🆕 新会话已开启。\n旧会话已归档为「{old_title}」，历史仍可在控制台回看。"),
            None => "🆕 新会话已开启。".to_string(),
        },
        Err(e) => {
            // 归档成功但建新失败：把旧会话名改回去，避免规范标题悬空（下一轮消息找不到会话会另建，历史就"断"了）
            if let Some((old_id, _)) = &archived {
                let _ = core.store.update_session_title(old_id, &title);
            }
            format!("⚠️ 开新会话失败：{e}。旧会话未受影响，可直接重试。")
        }
    }
}

/// /status：通道身份、连接状态（适配器上报）、当前会话、白名单口径。
fn cmd_status(core: &Arc<Core>, ch: &Channel, chat_key: &str) -> String {
    let gid = ch.group.clone().unwrap_or_else(|| core.active_group());
    let conn = match status_of(&ch.id) {
        Some(s) => {
            let mark = if s.state == "ok" { "✅" } else { "❌" };
            format!("{mark} {}（{}）", if s.state == "ok" { "已连接" } else { "异常" }, s.detail)
        }
        // webhook/qq/wechat 桥接类没有运行时，无状态上报属正常
        None => "⚪ 无长连接（桥接类通道属正常）".to_string(),
    };
    let title = channel_session_title(&ch.id, chat_key);
    let sess = core
        .list_sessions_in_group(&gid)
        .ok()
        .and_then(|list| list.into_iter().find(|s| s.title == title));
    let sess_line = match sess {
        Some(s) => format!("· 会话：{}（状态 {}）", &s.id[..s.id.len().min(12)], s.status),
        None => "· 会话：尚未创建（发消息即建）".to_string(),
    };
    let wl = if ch.allowed_chats.is_empty() {
        "不限（空 = 所有会话）".to_string()
    } else {
        format!("{} 个会话", ch.allowed_chats.len())
    };
    format!(
        "📡 通道状态\n· 通道：{}（{}{}）\n· 绑定组：{}\n· 连接：{}\n{}\n· 白名单：{}",
        ch.id,
        ch.kind,
        ch.account.as_deref().map(|a| format!(" · {a}")).unwrap_or_default(),
        gid,
        conn,
        sess_line,
        wl,
    )
}

// ---------------------------------------------------------------- 消息去重

/// 去重表：平台 → (集合, 环形队列)。环形容量 1024：
/// 正常流量下是几分钟的窗口，足够覆盖 Resume 重放（重放只发生在断线瞬间附近），
/// 又不会无限吃内存。
fn seen() -> &'static Mutex<HashMap<String, (HashSet<String>, VecDeque<String>)>> {
    static SEEN: OnceLock<Mutex<HashMap<String, (HashSet<String>, VecDeque<String>)>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

const DEDUP_CAP: usize = 1024;

/// 首见返回 true（放行）；重复返回 false（丢弃）。key 为空视为不可去重（放行）。
pub(crate) fn first_seen(scope: &str, key: &str) -> bool {
    let key = key.trim();
    if key.is_empty() {
        return true;
    }
    let mut m = seen().lock();
    let (set, q) = m.entry(scope.to_string()).or_default();
    if !set.insert(key.to_string()) {
        return false;
    }
    q.push_back(key.to_string());
    if q.len() > DEDUP_CAP {
        if let Some(old) = q.pop_front() {
            set.remove(&old);
        }
    }
    true
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 命令解析_整词识别与误触防护() {
        assert_eq!(parse_command("/new"), Some(BuiltinCmd::New));
        assert_eq!(parse_command("  /new  "), Some(BuiltinCmd::New));
        assert_eq!(parse_command("/status"), Some(BuiltinCmd::Status));
        assert_eq!(parse_command("/status 后面跟话"), None, "带后缀不算命令");
        assert_eq!(parse_command("/NEW"), None, "命令区分大小写（不误触普通聊天）");
        assert_eq!(parse_command("你好 /new"), None, "混在句中不算命令");
        assert_eq!(parse_command("/stop"), None, "/stop 归 admit 管，这里不认");
        assert_eq!(parse_command(""), None);
    }

    #[test]
    fn 去重_首见放行重复丢弃() {
        assert!(first_seen("ut", "m1"), "首见放行");
        assert!(!first_seen("ut", "m1"), "重复丢弃");
        assert!(first_seen("ut", "m2"));
        // 不同平台作用域互不影响
        assert!(first_seen("ut2", "m1"));
        // 空 key 放行（平台没给 id 时不去重）
        assert!(first_seen("ut", ""));
        assert!(first_seen("ut", "  "));
    }

    #[test]
    fn 去重_环形容量淘汰最旧() {
        for i in 0..(DEDUP_CAP + 8) {
            first_seen("ut-cap", &format!("k{i}"));
        }
        // 最早的 k0 已被挤出环：再次出现视为首见
        assert!(first_seen("ut-cap", "k0"));
        // 环内的 k(DEDUP_CAP+4) 仍是重复
        assert!(!first_seen("ut-cap", &format!("k{}", DEDUP_CAP + 4)));
    }

    fn test_channel() -> Channel {
        Channel {
            id: "t1".into(),
            kind: "telegram".into(),
            enabled: true,
            group: None,
            allowed_chats: vec![],
            account: None,
            secret: None,
            reply_webhook: None,
            token: None,
            config: Default::default(),
            group_gate: None,
            created_at: String::new(),
        }
    }

    /// /new 端到端（替身 Core）：首轮直接建新；再开时旧会话归档、规范标题顶上新会话
    #[tokio::test]
    async fn 命令_开新会话归档旧会话() {
        let dir = std::env::temp_dir().join(format!("exm-chutil-{}", exm_core::types::new_id()));
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        let core = Arc::new({
            let mut cfg = exm_core::config::ExmConfig::load(&dir);
            cfg.use_mock = true;
            exm_core::Core::with_config(cfg).expect("创建 Core 失败")
        });
        let ch = test_channel();
        let gid = core.active_group();
        let title = "channel:t1:100";

        // 首轮：无旧会话 → 直接建新
        let r1 = builtin_command(&core, &ch, "100", "/new").await.unwrap();
        assert!(r1.contains("新会话已开启"), "{r1}");
        let first = core
            .list_sessions_in_group(&gid)
            .unwrap()
            .into_iter()
            .find(|s| s.title == title)
            .expect("首轮应建出规范标题会话");

        // 第二轮：旧会话归档（改名），规范标题上是全新会话
        let r2 = builtin_command(&core, &ch, "100", "/new").await.unwrap();
        assert!(r2.contains("归档"), "应提示归档去向：{r2}");
        let sessions = core.list_sessions_in_group(&gid).unwrap();
        let canonical = sessions.iter().find(|s| s.title == title).expect("归档后规范标题应有新会话");
        assert_ne!(canonical.id, first.id, "规范标题应顶上全新会话");
        let archived = sessions.iter().find(|s| s.id == first.id).expect("旧会话不删只改名");
        assert!(archived.title.starts_with(title) && archived.title.contains("归档"), "{}", archived.title);
    }

    /// /status：能报出通道与连接口径（无运行时上报 = 桥接类属正常）
    #[tokio::test]
    async fn 命令_状态回执可读() {
        let dir = std::env::temp_dir().join(format!("exm-chutil-s-{}", exm_core::types::new_id()));
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        let core = Arc::new({
            let mut cfg = exm_core::config::ExmConfig::load(&dir);
            cfg.use_mock = true;
            exm_core::Core::with_config(cfg).expect("创建 Core 失败")
        });
        let ch = test_channel();
        let r = builtin_command(&core, &ch, "100", "/status").await.unwrap();
        assert!(r.contains("t1"), "含通道 id：{r}");
        assert!(r.contains("telegram"), "含平台：{r}");
        assert!(r.contains("尚未创建"), "未发过消息时会话未建：{r}");
        assert!(r.contains("白名单"), "{r}");
    }

    /// 非命令文本不拦截
    #[tokio::test]
    async fn 非命令文本放行() {
        let dir = std::env::temp_dir().join(format!("exm-chutil-n-{}", exm_core::types::new_id()));
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        let core = Arc::new({
            let mut cfg = exm_core::config::ExmConfig::load(&dir);
            cfg.use_mock = true;
            exm_core::Core::with_config(cfg).expect("创建 Core 失败")
        });
        let ch = test_channel();
        assert!(builtin_command(&core, &ch, "1", "帮我查一下天气").await.is_none());
    }
}
