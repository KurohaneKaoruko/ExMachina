//! Matrix 适配器（matrix.rs）：Client-Server API 长轮询 `/sync`（任何 homeserver 通用）。
//! 凭证：`config.homeserver`（如 https://matrix.org）+ 顶层 token（access token，
//! 从 Element「设置 → 帮助与关于 → 访问令牌」获取，或应用服务签发）。
//! 自检：GET /account/whoami 取自己的 user_id（同时用于过滤自己发的消息）。
//! 首次 sync 只取 `next_batch` 游标并丢弃历史事件，其后逐批处理 join 房间里的
//! `m.room.message` 文本事件（时间线倒序不需要，按批处理即可）。
//! 回复：PUT /rooms/{roomId}/send/m.room.message/{txnId}（txn 自增，幂等去重）。
//! token 失效（M_UNKNOWN_TOKEN）上报错误后指数退避重试。
//! 监督循环每 5 秒对账：新增账号拉起轮询，删除/停用/凭证变更的账号回收任务。

use crate::channel_util::{builtin_command, first_seen, media_placeholder};
use crate::platform::{admit, caps, report_status, spawn_reply, GateDecision, Channel, ChannelRun, InboundCtx, MediaItem};
use exm_core::Core;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::task::JoinHandle;

/// 消息正文上限（Matrix 事件上限 64KB，取收敛陈述的合理截断）
const MAX_CHARS: usize = 3800;

struct Poller {
    handle: JoinHandle<()>,
    /// 凭证指纹（homeserver + token）：变更即重启轮询
    fingerprint: String,
}

fn pollers() -> &'static Mutex<HashMap<String, Poller>> {
    static POLLERS: OnceLock<Mutex<HashMap<String, Poller>>> = OnceLock::new();
    POLLERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint(ch: &Channel) -> String {
    format!("{}|{}", ch.cfg("homeserver").unwrap_or_default(), ch.token.clone().unwrap_or_default())
}

pub fn spawn_supervisor(core: Arc<Core>) {
    tokio::spawn(async move {
        loop {
            reconcile(&core).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn reconcile(core: &Arc<Core>) {
    let desired: Vec<Channel> = crate::platform::load_channels(core)
        .into_iter()
        .filter(|c| {
            c.kind == "matrix"
                && c.enabled
                && c.cfg("homeserver").is_some()
                && c.token.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false)
        })
        .collect();

    let mut guards = pollers().lock();
    let stale: Vec<String> = guards
        .iter()
        .filter(|(id, p)| match desired.iter().find(|d| &d.id == *id) {
            Some(d) => p.fingerprint != fingerprint(d) || p.handle.is_finished(),
            None => true,
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in &stale {
        if let Some(p) = guards.remove(id) {
            p.handle.abort();
        }
    }
    for ch in &desired {
        if guards.contains_key(&ch.id) {
            continue;
        }
        let core = core.clone();
        let ch_task = ch.clone();
        let fp = fingerprint(ch);
        let handle = tokio::spawn(async move { poll_loop(core, ch_task).await });
        guards.insert(ch.id.clone(), Poller { handle, fingerprint: fp });
    }
}

/// 长轮询超时 25s，客户端超时放宽到 60s
fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder().timeout(Duration::from_secs(60)).build().ok()
}

async fn poll_loop(core: Arc<Core>, ch: Channel) {
    let hs = ch.cfg("homeserver").unwrap_or_default();
    let hs = hs.trim_end_matches('/').to_string();
    let token = ch.token.clone().unwrap_or_default();
    let Some(client) = client() else {
        eprintln!("[matrix:{}] HTTP 客户端构建失败", ch.id);
        return;
    };

    // 自检：access token → user_id（同时用于过滤自己发的消息）
    let mut self_user = String::new();
    match client
        .get(format!("{hs}/_matrix/client/v3/account/whoami"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            if let Ok(v) = r.json::<Value>().await {
                let uid = v.get("user_id").and_then(|x| x.as_str()).unwrap_or("?");
                self_user = uid.to_string();
                println!("[matrix:{}] 账号已连结：{uid}（{hs}）", ch.id);
                report_status(&ch.id, "ok", format!("账号 {uid}"));
            }
        }
        Ok(r) => {
            let msg = format!("access token 校验失败：HTTP {}（检查 homeserver 与令牌）", r.status());
            eprintln!("[matrix:{}] {msg}", ch.id);
            report_status(&ch.id, "error", msg);
        }
        Err(e) => {
            let msg = format!("网络错误：{e}");
            eprintln!("[matrix:{}] token 校验{msg}", ch.id);
            report_status(&ch.id, "error", msg);
        }
    }

    // since 游标：None = 首次同步（只取游标，丢弃历史消息）
    let mut since: Option<String> = None;
    let mut backoff = 15u64;
    loop {
        let url = match &since {
            // 首次同步不等：立刻返回当前游标
            None => format!("{hs}/_matrix/client/v3/sync?timeout=0"),
            Some(s) => format!(
                "{hs}/_matrix/client/v3/sync?timeout=25000&since={}",
                url_encode(s)
            ),
        };
        match client.get(&url).header("Authorization", format!("Bearer {token}")).send().await {
            Ok(r) if r.status().is_success() => {
                let body: Value = match r.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        let msg = format!("sync 响应解析失败：{e}");
                        eprintln!("[matrix:{}] {msg}", ch.id);
                        report_status(&ch.id, "error", msg);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };
                let next = body.get("next_batch").and_then(|x| x.as_str()).map(|x| x.to_string());
                match &since {
                    // 首轮：只记游标，历史消息不入队（避免上线即回复陈年旧话）
                    None => {
                        since = next;
                        // 首轮顺手把 whoami 没拿到的情况补一次（token 有效但 whoami 网络抖动）
                        report_status(
                            &ch.id,
                            "ok",
                            if self_user.is_empty() { "轮询中".into() } else { format!("账号 {self_user}") },
                        );
                        continue;
                    }
                    Some(_) => {
                        report_status(
                            &ch.id,
                            "ok",
                            if self_user.is_empty() { "轮询中".into() } else { format!("账号 {self_user}") },
                        );
                        for (room, sender, event_id, mentioned, raw_text) in parse_events(&body, &self_user) {
                            // 消息去重（防御性）：/sync 游标本身保证不重放，这里兜底
                            // 「游标丢失（如长期断线后重置）导致同批事件重见」的场景
                            if !first_seen(&format!("matrix:{}", ch.id), &event_id) {
                                continue;
                            }
                            // 入站媒体（组 6.4）：标记行解析 → mxc 下载 → 注入
                            let mut saved: Vec<(String, String, String)> = Vec::new();
                            let text: String = if raw_text.starts_with(US) {
                                let parts: Vec<&str> = raw_text.split(US).collect();
                                if parts.len() == 3 {
                                    let (kind, mxc, _room) = (parts[0], parts[1], parts[2]);
                                    let dl = format!(
                                        "{}/_matrix/media/v3/download/{}",
                                        hs,
                                        url_encode(mxc.trim_start_matches("mxc://"))
                                    );
                                    // 纯媒体消息占位正文：曾给空串导致整条被下方空文本检查丢弃——
                                    // 「只发一张图」在 matrix 通道完全石沉大海
                                    let mut text = media_placeholder(&[kind]);
                                    match client.get(dl).bearer_auth(&token).send().await {
                                        Ok(r) if r.status().is_success() => {
                                            let ext = if kind == "image" { "png" } else if kind == "voice" { "ogg" } else { "bin" };
                                            let name = format!("matrix-{}.{ext}", exm_core::types::now_ms());
                                            if let Some(bytes) = r.bytes().await.ok().map(|b| b.to_vec()) {
                                                if let Some(p) = crate::platform::save_inbound_media(core.as_ref(), &ch, &name, bytes).await {
                                                    saved.push((kind.to_string(), p, name));
                                                }
                                            }
                                        }
                                        _ => {
                                            // 下载失败不硬塞空路径条目（会注出「已保存：<空>」的假话），
                                            // 改成显式失败占位，用户与模型都知道附件没收到
                                            let kind_cn = if kind == "image" { "图片" } else if kind == "voice" { "语音" } else { "文件" };
                                            text = format!("（用户发来一个{kind_cn}附件，但下载失败，未能读取内容）");
                                        }
                                    }
                                    text
                                } else {
                                    raw_text
                                }
                            } else {
                                raw_text
                            };
                            if text.trim().is_empty() {
                                continue;
                            }
                            if !ch.allowed_chats.is_empty() && !ch.allowed_chats.iter().any(|a| a == &room) {
                                eprintln!("[matrix:{}] {room} 不在白名单，已忽略", ch.id);
                                continue;
                            }
                            // 房间即群：门控按群聊语义执行（self_user 非空时提及才唤醒）
                            let gate_ctx = InboundCtx {
                                text: &text,
                                external_id: &sender,
                                display_name: &sender,
                                is_group: true,
                                mentioned: mentioned || self_user.is_empty(),
                                chat_key: &room,
                            };
                            match admit(core.as_ref(), &ch, &gate_ctx).await {
                                GateDecision::Allow => {}
                                GateDecision::Ignore => continue,
                                GateDecision::Deny(reply) => {
                                    let client2 = client.clone();
                                    let hs2 = hs.clone();
                                    let token2 = token.clone();
                                    let room2 = room.clone();
                                    tokio::spawn(async move { send_message(&client2, &hs2, &token2, &room2, &reply).await });
                                    continue;
                                }
                            }
                            // 内置命令（/new /status）：闸门放行后、进入会话执行前拦截
                            if let Some(reply) = builtin_command(&core, &ch, &room, &text).await {
                                let client2 = client.clone();
                                let hs2 = hs.clone();
                                let token2 = token.clone();
                                let room2 = room.clone();
                                tokio::spawn(async move { send_message(&client2, &hs2, &token2, &room2, &reply).await });
                                continue;
                            }
                            let core = core.clone();
                            let ch2 = ch.clone();
                            let hs2 = hs.clone();
                            let token2 = token.clone();
                            let client2 = client.clone();
                            let sender2 = sender.clone();
                            tokio::spawn(async move {
                                handle_message(&core, &ch2, &client2, &hs2, &token2, &room, &sender2, saved, &text).await;
                            });
                        }
                        since = next.or(since);
                    }
                }
                backoff = 15;
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                let msg = if status.as_u16() == 401 {
                    format!("access token 失效（HTTP 401）：{}", body.chars().take(120).collect::<String>())
                } else {
                    format!("sync HTTP {status}：{}", body.chars().take(120).collect::<String>())
                };
                eprintln!("[matrix:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(300);
            }
            Err(e) => {
                let msg = format!("sync 网络错误：{e}");
                eprintln!("[matrix:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(300);
            }
        }
    }
}

const US: char = '\u{1}'; // 入站媒体标记行分隔符（媒体消息正文形如 \u{1}kind\u{1}mxc\u{1}room）

/// /sync 响应 → [(房间 id, 发送者, 事件 id, 是否提及, 正文)]。自己发的消息与非文本消息一律忽略。
fn parse_events(body: &Value, self_user: &str) -> Vec<(String, String, String, bool, String)> {
    let mut out = Vec::new();
    let Some(join) = body.pointer("/rooms/join").and_then(|v| v.as_object()) else {
        return out;
    };
    for (room_id, room) in join {
        let Some(events) = room.pointer("/timeline/events").and_then(|v| v.as_array()) else {
            continue;
        };
        for e in events {
            if e.get("type").and_then(|x| x.as_str()) != Some("m.room.message") {
                continue;
            }
            let content = e.get("content").cloned().unwrap_or(Value::Null);
            let sender = e.get("sender").and_then(|x| x.as_str()).unwrap_or("");
            if !self_user.is_empty() && sender == self_user {
                continue;
            }
            let event_id = e.get("event_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            // 入站媒体（组 6.4）：m.image / m.file / m.audio → (kind, mxc, 文件名)
            let msgtype = content.get("msgtype").and_then(|x| x.as_str()).unwrap_or("");
            if matches!(msgtype, "m.image" | "m.file" | "m.audio") {
                let url = content.get("url").and_then(|x| x.as_str()).unwrap_or("");
                if !url.is_empty() {
                    let kind = match msgtype {
                        "m.image" => "image",
                        "m.audio" => "voice",
                        _ => "file",
                    };
                    out.push((
                        room_id.clone(),
                        sender.to_string(),
                        event_id,
                        false,
                        format!("{US}media{US}{kind}{US}{url}{US}{room_id}"),
                    ));
                    continue;
                }
            }
            if msgtype != "m.text" {
                continue;
            }
            let text = content.get("body").and_then(|x| x.as_str()).unwrap_or("");
            // m.mentions.user_ids 含本用户即提及；部分客户端以 body 前缀 @名 提及（宽松兜底）
            let mentioned = content
                .pointer("/m.mentions/user_ids")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|u| u.as_str() == Some(self_user)))
                .unwrap_or(false)
                || (!self_user.is_empty() && text.to_lowercase().contains(&format!("@{}", self_user.to_lowercase())));
            out.push((room_id.clone(), sender.to_string(), event_id, mentioned, text.to_string()));
        }
    }
    out
}

/// 最小百分号编码：只放行 unreserved 字符（since 游标是不透明串，可能含 `+ / =`）
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 跨模块复用（cron 结果推送等出站路径的 URL 编码）
pub(crate) fn url_encode_pub(s: &str) -> String {
    url_encode(s)
}

/// 一条用户消息：组绑定 → 会话复用 → 订阅回复 → 执行 → 房间回帖
#[allow(clippy::too_many_arguments)]
async fn handle_message(
    core: &Arc<Core>,
    ch: &Channel,
    client: &reqwest::Client,
    hs: &str,
    token: &str,
    room: &str,
    sender: &str,
    media: Vec<(String, String, String)>,
    text: &str,
) {
    // 会话键用房间 id，房间内多人共享同一会话（Matrix 房间即群）
    let Some((run, rx)) = ChannelRun::begin(core, ch, room).await else { return };
    core.stamp_session_origin(&run.session_id, sender, core.identity_of(&ch.id, sender).map(|i| i.id).unwrap_or_else(|| format!("ch:{}:{}", ch.id, sender)).as_str());
    // 入站媒体注入（组 6.4）：图片进多模态暂存，文件/语音附路径说明——
    // 曾下载保存后忘了传入执行上下文，附件等于白收
    let note = crate::platform::stage_inbound_media(core, &run.session_id, &media);
    let text = if note.is_empty() { text.to_string() } else { format!("{text}{note}") };
    let client2 = client.clone();
    let hs2 = hs.to_string();
    let token2 = token.to_string();
    let room2 = room.to_string();
    let m_client = client.clone();
    let m_hs = hs.to_string();
    let m_token = token.to_string();
    let m_room = room.to_string();
    let reply = spawn_reply(
        &run,
        rx,
        MAX_CHARS,
        move |text| {
            let client2 = client2.clone();
            let hs2 = hs2.clone();
            let token2 = token2.clone();
            let room2 = room2.clone();
            async move {
                send_message(&client2, &hs2, &token2, &room2, &text).await;
            }
        },
        move |item: MediaItem| {
            let client2 = m_client.clone();
            let hs2 = m_hs.clone();
            let token2 = m_token.clone();
            let room2 = m_room.clone();
            async move { mx_send_media(&client2, &hs2, &token2, &room2, &item).await }
        },
    );
    run.run(&text).await;
    let _ = reply.await;
}

/// matrix 媒体直发（组 6.3）：POST /media/v3/upload → content_uri → m.image / m.file
async fn mx_send_media(
    client: &reqwest::Client,
    hs: &str,
    token: &str,
    room: &str,
    item: &MediaItem,
) -> bool {
    let Ok(bytes) = tokio::fs::read(&item.path).await else {
        return false;
    };
    let mime = match item.kind.as_str() {
        "voice" => "audio/ogg",
        "file" => "application/octet-stream",
        _ => "image/png",
    };
    let name = item
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".into());
    static TXN: AtomicU64 = AtomicU64::new(0);
    let n = TXN.fetch_add(1, Ordering::Relaxed);
    let txn = format!("exm-media-{}-{n}", exm_core::types::now_ms());
    let upload = format!("{hs}/_matrix/media/v3/upload?filename={}", url_encode(&name));
    let resp = match client
        .post(upload)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", mime)
        .body(bytes)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return false,
    };
    if !resp.status().is_success() {
        return false;
    }
    let uri: String = match resp.json::<Value>().await {
        Ok(v) => v
            .get("content_uri")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        Err(_) => return false,
    };
    if uri.is_empty() {
        return false;
    }
    static MTXN: AtomicU64 = AtomicU64::new(0);
    let mn = MTXN.fetch_add(1, Ordering::Relaxed);
    let mtxn = format!("exm-msg-{}-{mn}", exm_core::types::now_ms());
    let msgtype = if item.kind == "voice" { "m.audio" } else if item.kind == "file" { "m.file" } else { "m.image" };
    let send = format!("{hs}/_matrix/client/v3/rooms/{}/send/m.room.message/{}", url_encode(room), url_encode(&mtxn));
    let body = json!({
        "msgtype": msgtype,
        "body": if item.caption.is_empty() { name.clone() } else { item.caption.clone() },
        "url": uri,
        "info": { "mimetype": mime },
    });
    client
        .put(send)
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn send_message(client: &reqwest::Client, hs: &str, token: &str, room: &str, text: &str) {
    if token.trim().is_empty() {
        return;
    }
    static TXN: AtomicU64 = AtomicU64::new(0);
    let n = TXN.fetch_add(1, Ordering::Relaxed);
    let txn = format!("exm{}-{}", exm_core::types::now_iso().replace([':', '.', '-', 'T', 'Z'], ""), n);
    match client
        .put(format!(
            "{hs}/_matrix/client/v3/rooms/{}/send/m.room.message/{txn}",
            url_encode(room)
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "msgtype": "m.text", "body": text }))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!("[matrix] 回复失败：HTTP {status} {body}");
        }
        Err(e) => eprintln!("[matrix] 回复网络错误：{e}"),
    }
}

// ---------------------------------------------------------------- 测试（纯函数部分）

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// /sync 事件解析：文本提及、自身过滤、媒体消息打标记行、非文本事件忽略
    #[test]
    fn 事件解析_提及与自身过滤与媒体标记() {
        let body = json!({
            "rooms": { "join": {
                "!r1:x": { "timeline": { "events": [
                    // 他人文本 + m.mentions 命中
                    { "type": "m.room.message", "sender": "@a:x", "event_id": "$e1",
                      "content": { "msgtype": "m.text", "body": "帮我看看", "m.mentions": { "user_ids": ["@me:x"] } } },
                    // 自己发的：忽略
                    { "type": "m.room.message", "sender": "@me:x", "event_id": "$e2",
                      "content": { "msgtype": "m.text", "body": "我自己说的" } },
                    // 图片消息：转标记行
                    { "type": "m.room.message", "sender": "@b:x", "event_id": "$e3",
                      "content": { "msgtype": "m.image", "body": "shot.png", "url": "mxc://x/abc" } },
                    // 非文本（如 m.notice）：忽略
                    { "type": "m.room.message", "sender": "@b:x", "event_id": "$e4",
                      "content": { "msgtype": "m.notice", "body": "bot 广播" } },
                    // 其他事件类型：忽略
                    { "type": "m.room.member", "sender": "@b:x", "event_id": "$e5", "content": {} },
                ] } }
            } }
        });
        let out = parse_events(&body, "@me:x");
        assert_eq!(out.len(), 2, "自身/notice/非消息事件不收：{out:?}");
        // 文本事件：提及命中、event_id 作去重键
        assert_eq!(out[0].0, "!r1:x");
        assert_eq!(out[0].2, "$e1");
        assert!(out[0].3, "m.mentions 命中应算提及");
        assert_eq!(out[0].4, "帮我看看");
        // 图片消息：标记行 \u{1}kind\u{1}mxc\u{1}room，未提及
        assert_eq!(out[1].2, "$e3");
        assert!(!out[1].3);
        assert_eq!(out[1].4, format!("{US}media{US}image{US}mxc://x/abc{US}!r1:x"));
        // 无 join 房间（初始 sync）返回空
        assert!(parse_events(&json!({}), "@me:x").is_empty());
    }

    /// URL 编码：只放行 unreserved 字符（since 游标 / 房间 id 含保留字符也不跑偏）
    #[test]
    fn url编码_游标与房间id安全() {
        assert_eq!(url_encode("abc123"), "abc123");
        assert_eq!(url_encode("a b/c+d"), "a%20b%2Fc%2Bd");
        assert_eq!(url_encode("!room:x.org"), "%21room%3Ax.org");
        assert_eq!(url_encode("-_.~"), "-_.~", "unreserved 字符不编码");
        // 非 ASCII 按 UTF-8 字节展开
        assert_eq!(url_encode("房"), "%E6%88%BF");
    }
}
