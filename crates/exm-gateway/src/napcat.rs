//! OneBot 11 平台适配器（napcat.rs）：适配 NapCat / Lagrange 等_onebot11_ 实现。
//! 接法：正向 WebSocket —— 连接 OneBot 实现的 WS 服务端（config.url，可选 config.token 访问令牌），
//! 收 `post_type=message` 事件 → 绑定组内执行 → `send_group_msg` / `send_private_msg` 回复。
//! 监督循环每 5 秒对账：新增账号拉起连接，删除/停用/地址或令牌变更的账号回收任务。
//!
//! 收发解耦：入站读取不阻塞（消息处理全部 spawn），动作经 mpsc 通道交给专职写任务，
//! 避免长运行期间无法应答 WS 层 Ping 被服务端断开。

use crate::channel_util::{builtin_command, first_seen, media_placeholder};
use crate::platform::{admit, report_status, spawn_reply, GateDecision, Channel, ChannelRun, InboundCtx, MediaItem};
use exm_core::Core;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::connect_async;

struct Conn {
    handle: JoinHandle<()>,
    /// 连接指纹（url+token）：变更即重连
    fingerprint: String,
}

fn conns() -> &'static Mutex<HashMap<String, Conn>> {
    static CONNS: OnceLock<Mutex<HashMap<String, Conn>>> = OnceLock::new();
    CONNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint(ch: &Channel) -> String {
    format!("{}|{}", ch.cfg("url").unwrap_or_default(), ch.cfg("token").unwrap_or_default())
}

pub fn spawn_supervisor(core: Arc<Core>) {
    tokio::spawn(async move {
        loop {
            reconcile(&core).await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

async fn reconcile(core: &Arc<Core>) {
    let desired: Vec<Channel> = crate::platform::load_channels(core)
        .into_iter()
        .filter(|c| c.kind == "napcat" && c.enabled && c.cfg("url").is_some())
        .collect();

    let mut guards = conns().lock().unwrap();
    let stale: Vec<String> = guards
        .iter()
        .filter(|(id, c)| {
            match desired.iter().find(|d| &d.id == *id) {
                Some(d) => c.fingerprint != fingerprint(d) || c.handle.is_finished(),
                None => true,
            }
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in &stale {
        if let Some(c) = guards.remove(id) {
            c.handle.abort();
        }
    }
    for ch in &desired {
        if guards.contains_key(&ch.id) {
            continue;
        }
        let core = core.clone();
        let ch_task = ch.clone();
        let fp = fingerprint(ch);
        let handle = tokio::spawn(async move { connect_loop(core, ch_task).await });
        guards.insert(ch.id.clone(), Conn { handle, fingerprint: fp });
    }
}

async fn connect_loop(core: Arc<Core>, ch: Channel) {
    let url = ch.cfg("url").unwrap_or_default();
    let token = ch.cfg("token").unwrap_or_default();
    // 断线退避：连续失败按 5→10→20…（封顶 300s）指数递增；连接成功即复位。
    // NapCat 停机维护时不该每 5 秒重试一次敲日志。
    let mut fail_streak: u32 = 0;
    loop {
        // 带鉴权头的握手请求（token 为空则不带）
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("X-Client-Role", "ExMachina-Gateway")
            .body(());
        let ws = match request {
            Ok(req) => match connect_async(req).await {
                Ok((ws, _)) => {
                    println!("[napcat:{}] 已连结 {url}", ch.id);
                    report_status(&ch.id, "ok", format!("已连结 {url}"));
                    // 复位退避：连上即视为恢复（连上后立刻断开属对端不稳，5s 基线重试可接受）
                    fail_streak = 0;
                    ws
                }
                Err(e) => {
                    let msg = format!("连接失败：{e}");
                    eprintln!("[napcat:{}] {msg}", ch.id);
                    report_status(&ch.id, "error", msg);
                    let wait = std::time::Duration::from_secs((5u64 << fail_streak.min(7)).min(300));
                    fail_streak = fail_streak.saturating_add(1);
                    tokio::time::sleep(wait).await;
                    continue;
                }
            },
            Err(e) => {
                let msg = format!("地址非法（{url}）：{e}");
                eprintln!("[napcat:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            }
        };

        let (mut write, mut read) = ws.split();
        // 动作通道：事件处理任务 → 专职写任务（保持读循环常开，WS 层 Ping 及时应答）
        let (tx, mut rx) = mpsc::unbounded_channel::<serde_json::Value>();
        let writer = tokio::spawn(async move {
            while let Some(v) = rx.recv().await {
                if write.send(Message::text(v.to_string())).await.is_err() {
                    break;
                }
            }
        });
        // 自检：拿机器人身份（回包在读取循环里识别）
        let _ = tx.send(serde_json::json!({ "action": "get_login_info", "echo": "exm-login" }));

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(s)) => on_text(&core, &ch, &tx, &s).await,
                Ok(Message::Close(c)) => {
                    eprintln!("[napcat:{}] 服务端关闭：{c:?}", ch.id);
                    report_status(&ch.id, "error", "服务端关闭连接");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    let msg = format!("连接错误：{e}");
                    eprintln!("[napcat:{}] {msg}", ch.id);
                    report_status(&ch.id, "error", msg);
                    break;
                }
            }
        }
        drop(tx);
        let _ = writer.await;
        // 断线退避：指数递增（连接成功时已复位）
        let wait = std::time::Duration::from_secs((5u64 << fail_streak.min(7)).min(300));
        fail_streak = fail_streak.saturating_add(1);
        tokio::time::sleep(wait).await;
    }
}

async fn on_text(core: &Arc<Core>, ch: &Channel, tx: &mpsc::UnboundedSender<serde_json::Value>, s: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else { return };
    // API 回包（带 echo 的响应）：自检回包打日志，其余忽略
    if let Some(echo) = v.get("echo").and_then(|x| x.as_str()) {
        if echo == "exm-login" {
            let nick = v.pointer("/data/nickname").and_then(|x| x.as_str()).unwrap_or("?");
            let uid = v.pointer("/data/user_id").and_then(|x| x.as_i64()).unwrap_or(0);
            println!("[napcat:{}] 机器人已连结：{nick}（{uid}）", ch.id);
            report_status(&ch.id, "ok", format!("机器人 {nick}（{uid}）"));
        }
        return;
    }
    let post = v.get("post_type").and_then(|x| x.as_str()).unwrap_or("");
    if post != "message" {
        return; // meta_event / request / notice 忽略
    }
    let group_id = v.get("group_id").and_then(|x| x.as_i64());
    let user_id = v.get("user_id").and_then(|x| x.as_i64());
    // 机器人自己的消息不回环处理（NapCat 开启 reportSelfMessage 时会上报自己说的话）
    if user_id.is_some() && v.get("self_id").and_then(|x| x.as_i64()) == user_id {
        return;
    }
    // 消息去重：OneBot 实现断线重连可能重放最近事件，按 message_id 首见放行
    let msg_id = v.get("message_id").map(|x| x.to_string()).unwrap_or_default();
    if !first_seen(&format!("napcat:{}", ch.id), &msg_id) {
        return;
    }
    let mut text = extract_text(v.get("message").unwrap_or(&serde_json::Value::Null));
    // 入站媒体段（组 6.4）：先盘类型，纯媒体消息给占位正文——
    // 曾在文本为空时直接 return，「只发一张图」被整体丢弃
    let media = collect_media(v.get("message").unwrap_or(&serde_json::Value::Null));
    if text.trim().is_empty() {
        let kinds: Vec<&str> = media.iter().map(|(k, _)| k.as_str()).collect();
        text = media_placeholder(&kinds);
    }
    if text.trim().is_empty() {
        return;
    }
    // 会话白名单：非空时仅放行清单内的群号 / QQ 号
    let peer = group_id.map(|g| g.to_string()).or(user_id.map(|u| u.to_string()));
    if !ch.allowed_chats.is_empty() {
        let ok = peer.as_deref().map(|p| ch.allowed_chats.iter().any(|a| a == p)).unwrap_or(false);
        if !ok {
            eprintln!("[napcat:{}] {peer:?} 不在白名单，已忽略", ch.id);
            return;
        }
    }
    let core = core.clone();
    let ch = ch.clone();
    let tx = tx.clone();
    let raw = v.clone();
    tokio::spawn(async move {
        let Some(external_id) = napcat_gate(&core, &ch, &tx, group_id, user_id, &raw, &text).await else {
            return; // 闸门拦截（忽略 / 已回复）
        };
        // 入站媒体（组 6.4）：下载或本地 file:// 复制 → 注入（段清单已在闸门前盘好）；
        // 语音段（record/voice）顺带转写为文本注记——telegram 同款能力，
        // QQ 语音是高频输入形态，只存路径等于让用户对着机器人说了一堆「已保存的噪音」
        let mut saved: Vec<(String, String, String)> = Vec::new();
        let mut transcript_note = String::new();
        for (kind, src) in media {
            let name = format!("napcat-{}.{}", exm_core::types::now_ms(), if kind == "image" { "png" } else if kind == "voice" { "ogg" } else { "bin" });
            let bytes = if let Some(local) = src.strip_prefix("file://") {
                std::fs::read(local).ok()
            } else {
                crate::platform::download_bytes(&src).await
            };
            if let Some(b) = bytes {
                if let Some(p) = crate::platform::save_inbound_media(&core, &ch, &name, b.clone()).await {
                    saved.push((kind.clone(), p, name.clone()));
                }
                if kind == "voice" {
                    transcript_note.push_str(&crate::channel_util::voice_transcript_note(&core, b, &name).await);
                }
            }
        }
        let text = if transcript_note.is_empty() { text } else { format!("{text}{transcript_note}") };
        // 内置命令（/new /status）：闸门放行后、进入会话执行前拦截
        // 会话键与 handle_message 同口径：群号优先，私聊为用户 id
        let peer = group_id
            .map(|g| g.to_string())
            .or_else(|| user_id.map(|u| u.to_string()))
            .unwrap_or_default();
        if let Some(reply) = builtin_command(&core, &ch, &peer, &text).await {
            let action = match group_id {
                Some(g) => serde_json::json!({
                    "action": "send_group_msg",
                    "params": { "group_id": g, "message": [{ "type": "text", "data": { "text": reply } }] },
                }),
                None => serde_json::json!({
                    "action": "send_private_msg",
                    "params": { "user_id": user_id.unwrap_or(0), "message": [{ "type": "text", "data": { "text": reply } }] },
                }),
            };
            let _ = tx.send(action);
            return;
        }
        handle_message(&core, &ch, &tx, group_id, user_id, &external_id, saved, &text).await;
    });
}

/// OneBot 消息体 → 附件段清单 (kind, 来源)：kind ∈ image | file | voice（record 归一为 voice）；
/// 来源取 data.url，缺省回退 data.file（可能是本地 file:// 路径，由调用方分支处理）。
fn collect_media(m: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(arr) = m.as_array() else {
        return out; // CQ 码字符串形态不带结构化附件段（NapCat 默认数组段）
    };
    for seg in arr {
        let t = seg.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let kind = match t {
            "image" => "image",
            "file" => "file",
            // 语音段（record / voice 两种写法都见过）；视频体积大且无消费方，不采
            "record" | "voice" => "voice",
            _ => continue,
        };
        let url = seg.pointer("/data/url").and_then(|x| x.as_str()).unwrap_or("");
        let file = seg.pointer("/data/file").and_then(|x| x.as_str()).unwrap_or("");
        let src = if !url.is_empty() { url.to_string() } else { file.to_string() };
        if src.is_empty() {
            continue;
        }
        out.push((kind.to_string(), src));
    }
    out
}

/// OneBot 消息的入站闸门（群聊唤醒 + 身份管控）：在 spawn 的任务内执行
async fn napcat_gate(
    core: &Core,
    ch: &Channel,
    tx: &tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    group_id: Option<i64>,
    user_id: Option<i64>,
    raw: &serde_json::Value,
    text: &str,
) -> Option<String> {
    let is_group = group_id.is_some();
    // 提及归一化：原始消息含 @ 段（CQ:at）即视为提及（QQ 群内 @ 机器人是主语义）
    let raw_text = raw.get("message").map(|m| m.to_string()).unwrap_or_default();
    let mentioned = raw_text.contains("CQ:at");
    let external_id = user_id.map(|u| u.to_string()).unwrap_or_default();
    // 会话键（与 ChannelRun::begin 同口径）：群号优先，私聊为用户 id
    let peer = group_id
        .map(|g| g.to_string())
        .or_else(|| user_id.map(|u| u.to_string()))
        .unwrap_or_default();
    let ctx = InboundCtx { text, external_id: &external_id, display_name: &external_id, is_group, mentioned, chat_key: &peer };
    match admit(core, ch, &ctx).await {
        GateDecision::Allow => Some(external_id),
        GateDecision::Ignore => None,
        GateDecision::Deny(reply) => {
            let action = match group_id {
                Some(g) => serde_json::json!({
                    "action": "send_group_msg",
                    "params": { "group_id": g, "message": [{ "type": "text", "data": { "text": reply } }] },
                }),
                None => serde_json::json!({
                    "action": "send_private_msg",
                    "params": { "user_id": user_id.unwrap_or(0), "message": [{ "type": "text", "data": { "text": reply } }] },
                }),
            };
            let _ = tx.send(action);
            None
        }
    }
}

/// OneBot 消息体 → 纯文本：数组段取 text；CQ 码字符串剥掉非文本段。
fn extract_text(m: &serde_json::Value) -> String {
    if let Some(arr) = m.as_array() {
        arr.iter()
            .filter(|seg| seg.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|seg| seg.pointer("/data/text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("")
    } else if let Some(s) = m.as_str() {
        strip_cq(s)
    } else {
        String::new()
    }
}

fn strip_cq(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("[CQ:") {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                // 找不到收尾 ] ：原样保留标记与后面正文，曾把前缀重复拼一遍
                out.push_str(&rest[i..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 一条用户消息：组绑定 → 会话复用 → 订阅回复 → 执行 → send_group_msg / send_private_msg
async fn handle_message(
    core: &Arc<Core>,
    ch: &Channel,
    tx: &mpsc::UnboundedSender<serde_json::Value>,
    group_id: Option<i64>,
    user_id: Option<i64>,
    external_id: &str,
    media: Vec<(String, String, String)>,
    text: &str,
) {
    // 会话键：群聊优先，无群号走私聊
    let peer = group_id.map(|g| g.to_string()).or(user_id.map(|u| u.to_string())).unwrap_or_default();
    let Some((run, rx)) = ChannelRun::begin(core, ch, &peer).await else {
        return;
    };
    core.stamp_session_origin(&run.session_id, external_id, core.identity_of(&ch.id, external_id).map(|i| i.id).unwrap_or_else(|| format!("ch:{}:{}", ch.id, external_id)).as_str());
    // 入站媒体注入（组 6.4）：图片进多模态暂存，文件附路径说明
    let note = crate::platform::stage_inbound_media(core, &run.session_id, &media);
    let text = if note.is_empty() { text.to_string() } else { format!("{text}{note}") };
    let tx = tx.clone();
    let media_tx = tx.clone();
    let reply = spawn_reply(
        &run,
        rx,
        3500,
        move |text| {
            let tx = tx.clone();
            async move {
                let action = match group_id {
                    Some(g) => serde_json::json!({
                        "action": "send_group_msg",
                        "params": { "group_id": g, "message": [{ "type": "text", "data": { "text": text } }] },
                    }),
                    None => serde_json::json!({
                        "action": "send_private_msg",
                        "params": { "user_id": user_id.unwrap_or(0), "message": [{ "type": "text", "data": { "text": text } }] },
                    }),
                };
                let _ = tx.send(action);
            }
        },
        move |item: MediaItem| {
            let tx = media_tx.clone();
            async move {
                // OneBot 11：base64:// 数据段（远程 NapCat 同样可用，免文件系统共享）
                let (seg_type, mime) = match item.kind.as_str() {
                    "voice" => ("record", "audio/ogg"),
                    "file" => ("file", "application/octet-stream"),
                    _ => ("image", "image/png"),
                };
                let Ok(bytes) = tokio::fs::read(&item.path).await else {
                    return false;
                };
                use base64::Engine as _;
                let data = format!("base64://{}", base64::engine::general_purpose::STANDARD.encode(bytes));
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "media".into());
                let seg = match seg_type {
                    "file" => serde_json::json!({ "type": "file", "data": { "file": data, "name": name } }),
                    _ => serde_json::json!({ "type": seg_type, "data": { "file": data } }),
                };
                let _ = mime;
                let action = match group_id {
                    Some(g) => serde_json::json!({
                        "action": "send_group_msg",
                        "params": { "group_id": g, "message": [seg] },
                    }),
                    None => serde_json::json!({
                        "action": "send_private_msg",
                        "params": { "user_id": user_id.unwrap_or(0), "message": [seg] },
                    }),
                };
                let _ = tx.send(action);
                true
            }
        },
    );
    run.run(&text).await;
    let _ = reply.await;
}

// ---------------------------------------------------------------- 测试（纯函数部分）

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 文本提取：数组段拼接 text，其余类型（图/文件/AT）不混入正文
    #[test]
    fn 文本提取_数组段拼接() {
        let m = json!([
            { "type": "text", "data": { "text": "帮我" } },
            { "type": "image", "data": { "url": "https://x/y.png" } },
            { "type": "text", "data": { "text": "看这张图" } },
            { "type": "at", "data": { "qq": "10086" } },
        ]);
        assert_eq!(extract_text(&m), "帮我看这张图");
        // 非数组（CQ 码字符串）走剥离路径
        assert_eq!(extract_text(&json!("看[CQ:image,id=1]这张")), "看这张");
    }

    /// CQ 码剥离：完整码剥掉；未闭合码不吞后面正文
    #[test]
    fn cq码剥离_未闭合不吞正文() {
        assert_eq!(strip_cq("[CQ:at,qq=1]你好"), "你好");
        assert_eq!(strip_cq("a[CQ:x]b[CQ:y]c"), "abc");
        assert_eq!(strip_cq("没有码"), "没有码");
        assert_eq!(strip_cq("未闭合[CQ:at"), "未闭合[CQ:at");
    }

    /// 附件段盘型：image / file / record(→voice) 分类，缺来源的段跳过，非数组段返回空
    #[test]
    fn 附件段盘型_语音归一与来源回退() {
        let m = json!([
            { "type": "image", "data": { "url": "https://x/a.png", "file": "a.png" } },
            { "type": "record", "data": { "file": "file:///tmp/b.mp3" } },
            { "type": "file", "data": {} },
            { "type": "at", "data": { "qq": "1" } },
        ]);
        let media = collect_media(&m);
        assert_eq!(media.len(), 2, "缺来源的 file 段与 at 段不收：{media:?}");
        assert_eq!(media[0], ("image".to_string(), "https://x/a.png".to_string()), "url 优先");
        assert_eq!(media[1], ("voice".to_string(), "file:///tmp/b.mp3".to_string()), "record 归一为 voice");
        // CQ 码字符串形态：无结构化段 → 空（不至于误采）
        assert!(collect_media(&json!("[CQ:image,url=x]")).is_empty());
    }
}
