//! Discord 适配器（discord.rs）：Gateway WebSocket 长连接（API v10）。
//! 自检：GET /users/@me（bot token 校验 + 机器人身份）；
//! 网关：GET /gateway/bot → `wss://…/?v=10&encoding=json` → HELLO(op10) → Identify(op2)。
//! intents = GUILDS | GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT；
//! 其中 **MESSAGE_CONTENT 是特权 intent**，须在开发者门户 → Bot 页勾选，
//! 否则服务器频道里的消息正文为空（私聊与 @ 提及不受此限）。
//! 回复：POST /channels/{channel_id}/messages（单条限 2000 字符 → 截 1800）。
//! 断线优先 Resume(op6，补发漏掉的事件)；op9 Invalid Session 回退重新 Identify。
//! 监督循环每 5 秒对账：新增账号拉起会话，删除/停用/token 变更的账号回收任务。

use crate::channel_util::{builtin_command, first_seen, media_placeholder};
use crate::platform::{admit, caps, report_status, spawn_reply, GateDecision, Channel, ChannelRun, InboundCtx, MediaItem};
use exm_core::Core;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::connect_async;

const API: &str = "https://discord.com/api/v10";
/// GUILDS(1<<0) | GUILD_MESSAGES(1<<9) | DIRECT_MESSAGES(1<<12) | MESSAGE_CONTENT(1<<15)
const INTENTS: i64 = (1 << 0) | (1 << 9) | (1 << 12) | (1 << 15);
/// 单条消息上限 2000 字符，留出余量
const MAX_CHARS: usize = 1800;

struct Sess {
    handle: JoinHandle<()>,
    /// token 指纹：变更即重开会话
    fingerprint: String,
}

fn sessions() -> &'static Mutex<HashMap<String, Sess>> {
    static SESS: OnceLock<Mutex<HashMap<String, Sess>>> = OnceLock::new();
    SESS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint(ch: &Channel) -> String {
    ch.token.clone().unwrap_or_default()
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
            c.kind == "discord"
                && c.enabled
                && c.token.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false)
        })
        .collect();

    let mut guards = sessions().lock();
    let stale: Vec<String> = guards
        .iter()
        .filter(|(id, s)| match desired.iter().find(|d| &d.id == *id) {
            Some(d) => s.fingerprint != fingerprint(d) || s.handle.is_finished(),
            None => true,
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in &stale {
        if let Some(s) = guards.remove(id) {
            s.handle.abort();
        }
    }
    for ch in &desired {
        if guards.contains_key(&ch.id) {
            continue;
        }
        let core = core.clone();
        let ch_task = ch.clone();
        let fp = fingerprint(ch);
        let handle = tokio::spawn(async move { session_loop(core, ch_task).await });
        guards.insert(ch.id.clone(), Sess { handle, fingerprint: fp });
    }
}

async fn session_loop(core: Arc<Core>, ch: Channel) {
    let token = ch.token.clone().unwrap_or_default();
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(30)).build() else {
        eprintln!("[discord:{}] HTTP 客户端构建失败", ch.id);
        return;
    };
    let auth = format!("Bot {token}");
    // 自检：token 有效性 + 机器人身份
    match client.get(format!("{API}/users/@me")).header("Authorization", &auth).send().await {
        Ok(r) if r.status().is_success() => {
            if let Ok(v) = r.json::<Value>().await {
                let name = v.get("username").and_then(|x| x.as_str()).unwrap_or("?");
                println!("[discord:{}] 机器人已连结：{name}", ch.id);
                report_status(&ch.id, "ok", format!("机器人 {name}"));
            }
        }
        Ok(r) => {
            let msg = format!("token 校验失败：HTTP {}（检查 bot token）", r.status());
            eprintln!("[discord:{}] {msg}", ch.id);
            report_status(&ch.id, "error", msg);
        }
        Err(e) => {
            let msg = format!("网络错误：{e}");
            eprintln!("[discord:{}] token 校验{msg}", ch.id);
            report_status(&ch.id, "error", msg);
        }
    }

    // (session_id, seq)：断线优先 Resume 补发
    let mut session: Option<(String, i64)> = None;
    let mut bot_id = String::new(); // READY 时捕获本 bot 用户 id（提及归一化用）
    // 断线退避：连续失败按 3→6→12…（封顶 300s）指数递增；连结成功（READY/RESUMED）即复位。
    // 否则 token 失效或网关维护期间会每 3 秒重连一次。
    let mut fail_streak: u32 = 0;
    loop {
        let gw = match client.get(format!("{API}/gateway/bot")).header("Authorization", &auth).send().await {
            Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                Ok(v) => v.get("url").and_then(|u| u.as_str()).map(|u| u.to_string()),
                _ => None,
            },
            Ok(r) => {
                let msg = format!("网关接口 HTTP {}（token 或权限问题）", r.status());
                eprintln!("[discord:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            Err(e) => {
                let msg = format!("网关接口网络错误：{e}");
                eprintln!("[discord:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(15)).await;
                continue;
            }
        };
        let Some(gw) = gw else {
            eprintln!("[discord:{}] 网关响应缺少 url", ch.id);
            report_status(&ch.id, "error", "网关响应缺少 url");
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        };
        let url = format!("{}/?v=10&encoding=json", gw.trim_end_matches('/'));
        let (ws, _) = match connect_async(&url).await {
            Ok(x) => x,
            Err(e) => {
                let msg = format!("WS 连接失败：{e}");
                eprintln!("[discord:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };
        let (mut write, mut read) = ws.split();

        // HELLO(op10) → 心跳周期
        let hb_ms = match read.next().await {
            Some(Ok(Message::Text(s))) => serde_json::from_str::<Value>(&s)
                .ok()
                .and_then(|v| v.pointer("/d/heartbeat_interval").and_then(|x| x.as_u64()))
                .unwrap_or(41_250),
            _ => 41_250,
        };
        let hb_ms = hb_ms.saturating_sub(3_000).max(5_000);

        // 鉴权：有 session 走 Resume 补发，否则 Identify
        let auth_msg = match &session {
            Some((sid, seq)) => json!({ "op": 6, "d": { "token": token, "session_id": sid, "seq": seq } }),
            None => json!({
                "op": 2,
                "d": {
                    "token": token,
                    "intents": INTENTS,
                    "properties": {
                        "os": std::env::consts::OS,
                        "browser": "exmachina",
                        "device": "exmachina-gateway",
                    },
                }
            }),
        };
        if write.send(Message::text(auth_msg.to_string())).await.is_err() {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let mut hb = tokio::time::interval(Duration::from_millis(hb_ms));
        hb.tick().await;
        loop {
            tokio::select! {
                maybe = read.next() => match maybe {
                    Some(Ok(Message::Text(s))) => {
                        let Ok(v) = serde_json::from_str::<Value>(&s) else { continue };
                        match v.get("op").and_then(|x| x.as_i64()) {
                            Some(11) => {} // 心跳 ACK
                            Some(1) => { // 服务端要求立即心跳
                                let d = session.as_ref().map(|(_, s)| json!(s)).unwrap_or(Value::Null);
                                let _ = write.send(Message::text(json!({ "op": 1, "d": d }).to_string())).await;
                            }
                            Some(9) => { // Invalid Session：凭证或 intents 有误
                                let msg = "鉴权失效（检查 bot token 与 intents 权限）";
                                eprintln!("[discord:{}] {msg}", ch.id);
                                report_status(&ch.id, "error", msg);
                                session = None;
                                break;
                            }
                            Some(7) => break, // 服务端要求重连：保留 session 走 Resume
                            Some(0) => {
                                let seq = v.get("s").and_then(|x| x.as_i64());
                                if let (Some(n), Some(sess)) = (seq, session.as_mut()) {
                                    sess.1 = n;
                                }
                                let t = v.get("t").and_then(|x| x.as_str()).unwrap_or("");
                                let d = v.get("d").cloned().unwrap_or(Value::Null);
                                match t {
                                    "READY" => {
                                        let sid = d.get("session_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        session = Some((sid, seq.unwrap_or(0)));
                                        bot_id = d.pointer("/user/id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        let user = d.pointer("/user/username").and_then(|x| x.as_str()).unwrap_or("?");
                                        println!("[discord:{}] 机器人已连结：{user}", ch.id);
                                        report_status(&ch.id, "ok", format!("机器人 {user}"));
                                        fail_streak = 0;
                                    }
                                    "RESUMED" => {
                                        report_status(&ch.id, "ok", "会话已恢复");
                                        fail_streak = 0;
                                    }
                                    "MESSAGE_CREATE" => {
                                        let Some((channel_id, content)) = parse_message(&d) else { continue };
                                        // 消息去重：断线 Resume 会重放漏收事件，按平台消息 id 首见放行
                                        let msg_id = d.get("id").and_then(|x| x.as_str()).unwrap_or("");
                                        if !first_seen(&format!("discord:{}", ch.id), msg_id) {
                                            continue;
                                        }
                                        // 入站媒体（组 6.4）：附件下载（图片与文件同走 CDN 直链）
                                        let atts = parse_attachments(&d);
                                        // 纯附件消息（无文字）：占位正文兜底，曾直接丢弃——用户「只发一张图」得不到响应
                                        let text = if content.trim().is_empty() {
                                            let kinds: Vec<&str> = atts.iter().map(|(k, _, _)| k.as_str()).collect();
                                            media_placeholder(&kinds)
                                        } else {
                                            content
                                        };
                                        if text.trim().is_empty() {
                                            continue;
                                        }
                                        if !ch.allowed_chats.is_empty()
                                            && !ch.allowed_chats.iter().any(|a| a == &channel_id)
                                        {
                                            eprintln!("[discord:{}] {channel_id} 不在白名单，已忽略", ch.id);
                                            continue;
                                        }
                                        // 提及归一化：mentions 数组命中本 bot；群组上下文 = 有 guild_id
                                        let is_group = d.get("guild_id").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
                                        let mut mentioned = false;
                                        if let Some(list) = d.get("mentions").and_then(|x| x.as_array()) {
                                            mentioned = if bot_id.is_empty() {
                                                !list.is_empty()
                                            } else {
                                                list.iter().any(|m| m.get("id").and_then(|x| x.as_str()) == Some(bot_id.as_str()))
                                            };
                                        }
                                        let external_id = d.pointer("/author/id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        let display = d.pointer("/author/global_name").or_else(|| d.pointer("/author/username")).and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        let gate_ctx = InboundCtx { text: &text, external_id: &external_id, display_name: &display, is_group, mentioned, chat_key: channel_id.as_str() };
                                        match admit(core.as_ref(), &ch, &gate_ctx).await {
                                            GateDecision::Allow => {}
                                            GateDecision::Ignore => continue,
                                            GateDecision::Deny(reply) => {
                                                let client2 = client.clone();
                                                let token2 = token.clone();
                                                let cid = channel_id.clone();
                                                tokio::spawn(async move { send_message(&client2, &token2, &cid, &reply).await });
                                                continue;
                                            }
                                        }
                                        // 内置命令（/new /status）：闸门放行后、进入会话执行前拦截
                                        if let Some(reply) = builtin_command(&core, &ch, &channel_id, &text).await {
                                            let client2 = client.clone();
                                            let token2 = token.clone();
                                            let cid = channel_id.clone();
                                            tokio::spawn(async move { send_message(&client2, &token2, &cid, &reply).await });
                                            continue;
                                        }
                                        let core = core.clone();
                                        let ch2 = ch.clone();
                                        let client2 = client.clone();
                                        let token2 = token.clone();
                                        tokio::spawn(async move {
                                            // 附件下载落 inbox（闸门已过，不浪费白名单外流量）
                                            let mut saved: Vec<(String, String, String)> = Vec::new();
                                            for (kind, url, name) in atts {
                                                if let Some(bytes) = crate::platform::download_bytes(&url).await {
                                                    if let Some(p) = crate::platform::save_inbound_media(&core, &ch2, &name, bytes).await {
                                                        saved.push((kind, p, name));
                                                    }
                                                }
                                            }
                                            handle_message(&core, &ch2, &client2, &token2, &channel_id, &external_id, saved, &text).await;
                                        });
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        let msg = format!("WS 错误：{e}");
                        eprintln!("[discord:{}] {msg}", ch.id);
                        report_status(&ch.id, "error", msg);
                        break;
                    }
                    None => {
                        eprintln!("[discord:{}] WS 断开", ch.id);
                        report_status(&ch.id, "error", "连接断开");
                        break;
                    }
                },
                _ = hb.tick() => {
                    let d = session.as_ref().map(|(_, s)| json!(s)).unwrap_or(Value::Null);
                    if write.send(Message::text(json!({ "op": 1, "d": d }).to_string())).await.is_err() {
                        break;
                    }
                }
            }
        }
        // 断线退避：指数递增（连结成功时已在 READY/RESUMED 处复位）
        let wait = Duration::from_secs((3u64 << fail_streak.min(7)).min(300));
        fail_streak = fail_streak.saturating_add(1);
        tokio::time::sleep(wait).await;
    }
}

/// MESSAGE_CREATE → (频道 id, 正文)。机器人自己的消息一律忽略。
fn parse_message(d: &Value) -> Option<(String, String)> {
    if d.pointer("/author/bot").and_then(|x| x.as_bool()).unwrap_or(false) {
        return None;
    }
    let channel_id = d.get("channel_id").and_then(|x| x.as_str())?.to_string();
    let content = d.get("content").and_then(|x| x.as_str()).unwrap_or("");
    Some((channel_id, strip_mentions(content)))
}

/// MESSAGE_CREATE 附件 → (kind, url, 文件名)：图片按 content_type 判定，其余归文件。
/// CDN 直链公开可下载，无需鉴权头。
fn parse_attachments(d: &Value) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    if let Some(list) = d.get("attachments").and_then(|x| x.as_array()) {
        for a in list {
            let url = a.get("url").and_then(|x| x.as_str()).unwrap_or("");
            let name = a.get("filename").and_then(|x| x.as_str()).unwrap_or("attachment.bin");
            if url.is_empty() {
                continue;
            }
            let kind = if a
                .get("content_type")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .starts_with("image/")
            {
                "image"
            } else {
                "file"
            };
            out.push((kind.to_string(), url.to_string(), name.to_string()));
        }
    }
    out
}

/// 剥掉 `<@…>` 提及片段；未闭合片段保留原样（曾把前缀重复拼一遍）
fn strip_mentions(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<@") {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                // 找不到收尾 > ：把标记连同后面正文原样保留，宁可带杂质也不吞正文
                out.push_str(&rest[i..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// 一条用户消息：组绑定 → 会话复用 → 订阅回复 → 执行 → 频道回帖
async fn handle_message(
    core: &Arc<Core>,
    ch: &Channel,
    client: &reqwest::Client,
    token: &str,
    channel_id: &str,
    external_id: &str,
    media: Vec<(String, String, String)>,
    text: &str,
) {
    // typing 指示（组 6.5）：触发输入中状态
    if caps("discord").typing {
        let _ = client
            .post(format!("{API}/channels/{channel_id}/typing"))
            .header("Authorization", format!("Bot {token}"))
            .send()
            .await;
    }
    let Some((run, rx)) = ChannelRun::begin(core, ch, channel_id).await else { return };
    // 会话来源登记：与其他适配器同口径（绑定身份用 id，未绑定回退通道主体键）——
    // 曾登记成 role 且缺省空串，导致 token 配额等按来源计的治理对该通道失明
    core.stamp_session_origin(&run.session_id, external_id, core.identity_of(&ch.id, external_id).map(|i| i.id).unwrap_or_else(|| format!("ch:{}:{}", ch.id, external_id)).as_str());
    // 入站媒体注入（组 6.4）：图片进多模态暂存，文件附路径说明
    let note = crate::platform::stage_inbound_media(core, &run.session_id, &media);
    let text = if note.is_empty() { text.to_string() } else { format!("{text}{note}") };
    let client2 = client.clone();
    let token2 = token.to_string();
    let cid = channel_id.to_string();
    let media_client = client.clone();
    let media_token = token.to_string();
    let media_cid = channel_id.to_string();
    let reply = spawn_reply(
        &run,
        rx,
        MAX_CHARS,
        move |text| {
            let client2 = client2.clone();
            let token2 = token2.clone();
            let cid = cid.clone();
            async move {
                send_message(&client2, &token2, &cid, &text).await;
            }
        },
        move |item| {
            let client2 = media_client.clone();
            let token2 = media_token.clone();
            let cid = media_cid.clone();
            async move { dc_send_media(&client2, &token2, &cid, item).await }
        },
    );
    run.run(&text).await;
    let _ = reply.await;
}

/// discord 媒体直发（组 6.2）：multipart file[0] + caption；图片与文档同端点
async fn dc_send_media(
    client: &reqwest::Client,
    token: &str,
    channel_id: &str,
    item: MediaItem,
) -> bool {
    let Ok(bytes) = tokio::fs::read(&item.path).await else {
        return false;
    };
    let name = item
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".into());
    let form = reqwest::multipart::Form::new()
        .text("payload_json", json!({ "content": item.caption }).to_string())
        .part(
            "files[0]",
            reqwest::multipart::Part::bytes(bytes).file_name(name),
        );
    client
        .post(format!("{API}/channels/{channel_id}/messages"))
        .header("Authorization", format!("Bot {token}"))
        .multipart(form)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn send_message(client: &reqwest::Client, token: &str, channel_id: &str, text: &str) {    if token.trim().is_empty() {
        return;
    }
    match client
        .post(format!("{API}/channels/{channel_id}/messages"))
        .header("Authorization", format!("Bot {token}"))
        .json(&json!({ "content": text }))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!("[discord] 回复失败：HTTP {status} {body}");
        }
        Err(e) => eprintln!("[discord] 回复网络错误：{e}"),
    }
}

// ---------------------------------------------------------------- 测试（纯函数部分）

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 机器人自己的消息不解析（防回环）；普通消息剥提及取频道
    #[test]
    fn 消息解析_机器人自消息忽略() {
        let bot = json!({ "author": { "bot": true, "id": "9" }, "channel_id": "C1", "content": "我自己说的" });
        assert!(parse_message(&bot).is_none(), "bot 消息一律忽略");
        let human = json!({ "author": { "id": "1" }, "channel_id": "C1", "content": "<@99> 部署一下" });
        let (cid, text) = parse_message(&human).expect("人类消息应解析");
        assert_eq!(cid, "C1");
        assert_eq!(text, "部署一下");
    }

    /// 提及剥离：成员与角色两种片段都剥；未闭合不吞正文
    #[test]
    fn 提及剥离_成员与角色片段() {
        assert_eq!(strip_mentions("<@123> <@&456> 上线检查"), "上线检查", "剥完应收敛首尾空白");
        assert_eq!(strip_mentions("纯文本"), "纯文本");
        assert_eq!(strip_mentions("未闭合<@123"), "未闭合<@123");
    }

    /// 附件解析：图片按 content_type 归类，缺 url 的条目跳过，无附件返回空
    #[test]
    fn 附件解析_图片文件分类() {
        let d = json!({
            "attachments": [
                { "filename": "shot.png", "content_type": "image/png", "url": "https://cdn/1.png" },
                { "filename": "report.pdf", "content_type": "application/pdf", "url": "https://cdn/2.pdf" },
                { "filename": "broken.txt", "url": "" },
            ]
        });
        let atts = parse_attachments(&d);
        assert_eq!(atts.len(), 2, "缺 url 的条目不收：{atts:?}");
        assert_eq!(atts[0], ("image".to_string(), "https://cdn/1.png".to_string(), "shot.png".to_string()));
        assert_eq!(atts[1].0, "file", "非图片 content_type 归文件");
        // 无附件字段（纯文本消息）
        assert!(parse_attachments(&json!({ "content": "hi" })).is_empty());
        // content_type 缺失时保守归文件
        let no_type = parse_attachments(&json!({ "attachments": [{ "filename": "x.bin", "url": "https://cdn/3" }] }));
        assert_eq!(no_type[0].0, "file");
    }
}
