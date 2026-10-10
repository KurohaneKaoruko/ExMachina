//! QQ 官方机器人适配器（qqbot.rs）：q.qq.com 开放平台 WebSocket 长连接。
//! 鉴权链：appId + appSecret → getAppAccessToken（7200s，本地缓存提前 5 分钟刷新）
//! → WS identify（token 格式 `QQBot {access_token}`）。
//! intents：GROUP_AND_C2C_EVENT(1<<25) 群/单聊 + PUBLIC_GUILD_MESSAGES(1<<30) 公域频道。
//! 被动回复：群 /v2/groups/{group_openid}/messages、单聊 /v2/users/{user_openid}/messages、
//! 频道 /channels/{channel_id}/messages —— 均须携带事件里的 msg_id + 递增 msg_seq
//! （被动消息 5 分钟有效；群/单聊文本限 2000 字节，收束陈述截断到 650 字符）。
//! 断线重连：优先 Resume（补发漏掉的事件），Invalid Session(op9) 回退重新 Identify；
//! config.sandbox = "true" 时走沙箱 openapi（sandbox.api.sgroup.qq.com）。
//! 平台限制：被动消息仅支持文本——媒体产物以链接注记降级、无 typing 指示（见 platform::caps）；
//! 入站附件按扩展名归 image/file，落 inbox 注入（官方 bot 无语音消息形态）。
//! 监督循环每 5 秒对账：新增账号拉起会话，删除/停用/凭证变更的账号回收任务。

use crate::channel_util::{builtin_command, first_seen, media_placeholder};
use crate::platform::{admit, report_status, spawn_reply, GateDecision, Channel, ChannelRun, InboundCtx, MediaItem};
use exm_core::Core;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{atomic::AtomicU64, atomic::Ordering, Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::connect_async;

const TOKEN_HOST: &str = "https://api.bot.qq.com";
const OPENAPI: &str = "https://api.bot.qq.com";
const OPENAPI_SANDBOX: &str = "https://sandbox.api.sgroup.qq.com";
const INTENTS: i64 = (1 << 25) | (1 << 30);

struct Sess {
    handle: JoinHandle<()>,
    /// 凭证指纹（appId+secret+sandbox）：变更即重开会话
    fingerprint: String,
}

fn sessions() -> &'static Mutex<HashMap<String, Sess>> {
    static SESS: OnceLock<Mutex<HashMap<String, Sess>>> = OnceLock::new();
    SESS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint(ch: &Channel) -> String {
    format!("{}|{}|{}", ch.cfg("appId").unwrap_or_default(), ch.cfg("appSecret").unwrap_or_default(), ch.cfg("sandbox").unwrap_or_default())
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
        .filter(|c| c.kind == "qqbot" && c.enabled && c.cfg("appId").is_some() && c.cfg("appSecret").is_some())
        .collect();

    let mut guards = sessions().lock();
    let stale: Vec<String> = guards
        .iter()
        .filter(|(id, s)| {
            match desired.iter().find(|d| &d.id == *id) {
                Some(d) => s.fingerprint != fingerprint(d) || s.handle.is_finished(),
                None => true,
            }
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

// ---------------------------------------------------------------- 凭证

struct BotCreds {
    app_id: String,
    secret: String,
    client: reqwest::Client,
    /// (access_token, 过期时刻)：提前 5 分钟判失效
    cache: Mutex<Option<(String, Instant)>>,
}

impl BotCreds {
    async fn token(&self) -> Option<String> {
        {
            let g = self.cache.lock();
            if let Some((t, exp)) = &*g {
                if *exp > Instant::now() {
                    return Some(t.clone());
                }
            }
        }
        let resp: Value = self
            .client
            .post(format!("{TOKEN_HOST}/app/getAppAccessToken"))
            .json(&json!({ "appId": self.app_id, "clientSecret": self.secret }))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let token = resp.get("access_token")?.as_str()?.to_string();
        let ttl: u64 = resp
            .get("expires_in")
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(7200);
        let exp = Instant::now() + Duration::from_secs(ttl.saturating_sub(300));
        *self.cache.lock() = Some((token.clone(), exp));
        Some(token)
    }
}

// ---------------------------------------------------------------- 会话循环

async fn session_loop(core: Arc<Core>, ch: Channel) {
    let Some(app_id) = ch.cfg("appId") else { return };
    let Some(secret) = ch.cfg("appSecret") else { return };
    let sandbox = ch.cfg("sandbox").as_deref() == Some("true");
    let base = if sandbox { OPENAPI_SANDBOX } else { OPENAPI };
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(30)).build() else {
        eprintln!("[qqbot:{}] HTTP 客户端构建失败", ch.id);
        return;
    };
    let creds = Arc::new(BotCreds { app_id, secret, client: client.clone(), cache: Mutex::new(None) });
    // (session_id, seq)：断线优先 Resume 补发
    let mut session: Option<(String, i64)> = None;
    // 断线退避：连续失败按 3→6→12…（封顶 300s）指数退避；成功连结（READY/RESUMED）即复位。
    // 否则凭证长期失效时会每 3 秒敲一次网关，日志与对端都不好看。
    let mut fail_streak: u32 = 0;

    loop {
        let Some(token) = creds.token().await else {
            let msg = "access_token 获取失败（检查 appId/appSecret）";
            eprintln!("[qqbot:{}] {msg}，30s 后重试", ch.id);
            report_status(&ch.id, "error", msg);
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        };
        // 网关地址
        let gw = match client
            .get(format!("{base}/gateway"))
            .header("Authorization", format!("QQBot {token}"))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                Ok(v) => v.get("url").and_then(|u| u.as_str()).map(|u| u.to_string()),
                _ => None,
            },
            Ok(r) => {
                let msg = format!("网关接口 HTTP {}（token 或权限问题）", r.status());
                eprintln!("[qqbot:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            Err(e) => {
                let msg = format!("网关接口网络错误：{e}");
                eprintln!("[qqbot:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(15)).await;
                continue;
            }
        };
        let Some(gw) = gw else {
            let msg = "网关响应缺少 url";
            eprintln!("[qqbot:{}] {msg}", ch.id);
            report_status(&ch.id, "error", msg);
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        };
        let (ws, _) = match connect_async(&gw).await {
            Ok(x) => x,
            Err(e) => {
                let msg = format!("WS 连接失败：{e}");
                eprintln!("[qqbot:{}] {msg}（{gw}）", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };
        let (mut write, mut read) = ws.split();

        // Hello（op10）→ 心跳周期
        let hb_ms = match read.next().await {
            Some(Ok(Message::Text(s))) => serde_json::from_str::<Value>(&s)
                .ok()
                .and_then(|v| v.pointer("/d/heartbeat_interval").and_then(|x| x.as_u64()))
                .unwrap_or(45_000),
            _ => 45_000,
        };
        let hb_ms = hb_ms.saturating_sub(3_000).max(5_000);

        // 鉴权：有 session 走 Resume 补发，否则 Identify
        let auth = match &session {
            Some((sid, seq)) => json!({ "op": 6, "d": { "token": format!("QQBot {token}"), "session_id": sid, "seq": seq } }),
            None => json!({ "op": 2, "d": { "token": format!("QQBot {token}"), "intents": INTENTS, "shard": [0, 1] } }),
        };
        if write.send(Message::text(auth.to_string())).await.is_err() {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let mut hb = tokio::time::interval(Duration::from_millis(hb_ms));
        hb.tick().await; // interval 首跳立即触发，消耗掉
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
                            Some(9) => { // Invalid Session：凭证或 intents 有误，重新 Identify
                                let msg = "鉴权失效（检查 appId/appSecret 与机器人权限）";
                                eprintln!("[qqbot:{}] {msg}，将重新 Identify", ch.id);
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
                                        let user = d.pointer("/user/username").and_then(|x| x.as_str()).unwrap_or("?");
                                        println!("[qqbot:{}] 机器人已连结：{user}", ch.id);
                                        report_status(&ch.id, "ok", format!("机器人 {user}"));
                                        fail_streak = 0;
                                    }
                                    "RESUMED" => {
                                        report_status(&ch.id, "ok", "会话已恢复");
                                        fail_streak = 0;
                                    }
                                    "GROUP_AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" | "AT_MESSAGE_CREATE" => {
                                        let Some((peer, msg_id, content)) = parse_message(t, &d) else { continue };
                                        // 消息去重：断线 Resume 会重放最近事件，按平台消息 id 首见放行。
                                        // 放在最前——重放期间不能重复下载附件，更不能重复执行一轮对话。
                                        if !first_seen(&format!("qqbot:{}", ch.id), &msg_id) {
                                            continue;
                                        }
                                        // 入站媒体（组 6.4）：attachments 下载
                                        let mut inbound: Vec<(String, String, String, Vec<u8>)> = Vec::new();
                                        if let Some(atts) = d.get("attachments").and_then(|x| x.as_array()) {
                                            for att in atts {
                                                let url = att.get("url").and_then(|x| x.as_str()).unwrap_or("");
                                                let name = att.get("filename").and_then(|x| x.as_str()).unwrap_or("attachment.bin");
                                                if url.is_empty() {
                                                    continue;
                                                }
                                                let full = if url.starts_with("http") { url.to_string() } else { format!("https://multimedia.qq.com{url}") };
                                                let kind = if is_image_name(&name) { "image" } else { "file" };
                                                if let Some(bytes) = crate::platform::download_bytes(&full).await {
                                                    inbound.push((kind.into(), name.into(), peer.key().to_string(), bytes));
                                                }
                                            }
                                        }
                                        // 纯媒体消息（官方平台发图常无 caption）：占位正文兜底，
                                        // 曾直接空内容丢弃——用户「只发一张图」会得不到任何响应
                                        let kinds: Vec<&str> = inbound.iter().map(|(k, _, _, _)| k.as_str()).collect();
                                        let text = if content.trim().is_empty() { media_placeholder(&kinds) } else { content.trim().to_string() };
                                        if text.trim().is_empty() && inbound.is_empty() {
                                            continue;
                                        }
                                        if !ch.allowed_chats.is_empty()
                                            && !ch.allowed_chats.iter().any(|a| a == peer.key())
                                        {
                                            eprintln!("[qqbot:{}] {} 不在白名单，已忽略", ch.id, peer.key());
                                            continue;
                                        }
                                        // 闸门：官方 bot 群聊/频道本就 @ 驱动（mentioned 恒真）；C2C 私聊豁免门控
                                        let is_group = !matches!(peer, Peer::C2C(_));
                                        let external_id =
                                            d.pointer("/author/id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        let gate_ctx = InboundCtx {
                                            text: &text,
                                            external_id: &external_id,
                                            display_name: &external_id,
                                            is_group,
                                            mentioned: true,
                                            chat_key: peer.key(),
                                        };
                                        match admit(core.as_ref(), &ch, &gate_ctx).await {
                                            GateDecision::Allow => {}
                                            GateDecision::Ignore => continue,
                                            GateDecision::Deny(reply) => {
                                                let creds2 = creds.clone();
                                                let peer2 = peer.clone();
                                                let msg_id2 = msg_id.clone();
                                                tokio::spawn(async move {
                                                    send_passive(&creds2, &base, &peer2, &msg_id2, 1, &reply).await;
                                                });
                                                continue;
                                            }
                                        }
                                        // 内置命令（/new /status）：闸门放行后、进入会话执行前拦截
                                        // ——未绑定 / 被拒用户已被闸门挡住，这里只服务合法用户
                                        if let Some(reply) = builtin_command(&core, &ch, peer.key(), &text).await {
                                            let creds2 = creds.clone();
                                            let peer2 = peer.clone();
                                            let msg_id2 = msg_id.clone();
                                            tokio::spawn(async move {
                                                send_passive(&creds2, &base, &peer2, &msg_id2, 1, &reply).await;
                                            });
                                            continue;
                                        }
                                        let core = core.clone();
                                        let ch2 = ch.clone();
                                        let creds2 = creds.clone();
                                        tokio::spawn(async move {
                                            let mut saved: Vec<(String, String, String)> = Vec::new();
                                            for (kind, name, _ck, bytes) in inbound {
                                                if let Some(p) = crate::platform::save_inbound_media(&core, &ch2, &name, bytes).await {
                                                    saved.push((kind, p, name));
                                                }
                                            }
                                            handle_message(&core, &ch2, &creds2, base, peer, &external_id, saved, msg_id, &text).await;
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
                        eprintln!("[qqbot:{}] {msg}", ch.id);
                        report_status(&ch.id, "error", msg);
                        break;
                    }
                    None => {
                        eprintln!("[qqbot:{}] WS 断开", ch.id);
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
        // 断线退避：指数递增（3→6→12…封顶 300s）；连结成功时已在 READY/RESUMED 处复位
        let wait = Duration::from_secs((3u64 << fail_streak.min(7)).min(300));
        fail_streak = fail_streak.saturating_add(1);
        tokio::time::sleep(wait).await;
    }
}

/// 消息事件 → (会话端点, msg_id, 正文)。群/单聊走 openid，公域频道走 channel_id。
fn parse_message(t: &str, d: &Value) -> Option<(Peer, String, String)> {
    let msg_id = d.get("id").and_then(|x| x.as_str())?.to_string();
    let content = d.get("content").and_then(|x| x.as_str()).unwrap_or("");
    match t {
        "GROUP_AT_MESSAGE_CREATE" => {
            let g = d
                .get("group_openid")
                .or_else(|| d.get("group_id"))
                .or_else(|| d.get("groupid"))
                .and_then(|x| x.as_str())?
                .to_string();
            Some((Peer::Group(g), msg_id, strip_mentions(content)))
        }
        "C2C_MESSAGE_CREATE" => {
            let u = d
                .get("user_openid")
                .or_else(|| d.pointer("/author/user_openid"))
                .and_then(|x| x.as_str())?
                .to_string();
            Some((Peer::C2C(u), msg_id, strip_mentions(content)))
        }
        "AT_MESSAGE_CREATE" => {
            let c = d.get("channel_id").and_then(|x| x.as_str())?.to_string();
            Some((Peer::Guild(c), msg_id, strip_mentions(content)))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
enum Peer {
    /// 群（group_openid）
    Group(String),
    /// 单聊（user_openid）
    C2C(String),
    /// 公域频道（channel_id）
    Guild(String),
}

impl Peer {
    fn key(&self) -> &str {
        match self {
            Peer::Group(s) | Peer::C2C(s) | Peer::Guild(s) => s,
        }
    }
    fn max_chars(&self) -> usize {
        match self {
            // 群/单聊文本限 2000 字节，CJK 下 650 字符稳妥
            Peer::Guild(_) => 3800,
            _ => 650,
        }
    }
}

/// 附件文件名 → 是否图片（纯函数）：按最后一个 '.' 的扩展名（小写）比对常见位图后缀。
/// 曾用 contains(".png") 判定——「a.png.txt」会误判成图片，且漏掉 gif/webp/bmp。
fn is_image_name(name: &str) -> bool {
    matches!(
        name.to_lowercase().rsplit('.').next(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    )
}

/// 频道 @ 消息正文剥掉 `<@…>` 提及片段；未闭合片段保留原样（曾把前缀重复拼一遍）
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
    out
}

/// 一条用户消息：组绑定 → 会话复用 → 订阅回复 → 执行 → 被动回复
async fn handle_message(
    core: &Arc<Core>,
    ch: &Channel,
    creds: &Arc<BotCreds>,
    base: &str,
    peer: Peer,
    external_id: &str,
    media: Vec<(String, String, String)>,
    msg_id: String,
    text: &str,
) {
    let Some((run, rx)) = ChannelRun::begin(core, ch, peer.key()).await else {
        return;
    };
    core.stamp_session_origin(&run.session_id, external_id, core.identity_of(&ch.id, external_id).map(|i| i.id).unwrap_or_else(|| format!("ch:{}:{}", ch.id, external_id)).as_str());
    // 入站媒体注入（组 6.4）
    let note = crate::platform::stage_inbound_media(core, &run.session_id, &media);
    let text = if note.is_empty() { text.to_string() } else { format!("{text}{note}") };
    // 回复任务：被动回复须带原消息 msg_id + 递增 msg_seq
    let seq = Arc::new(AtomicU64::new(0));
    let max_chars = peer.max_chars();
    let creds = creds.clone();
    let base = base.to_string();
    let reply = spawn_reply(
        &run,
        rx,
        max_chars,
        move |text| {
            let creds = creds.clone();
            let base = base.clone();
            let peer = peer.clone();
            let msg_id = msg_id.clone();
            let seq = seq.clone();
            async move {
                let n = seq.fetch_add(1, Ordering::Relaxed) + 1;
                send_passive(&creds, &base, &peer, &msg_id, n, &text).await;
            }
        },
        move |_item: MediaItem| async { false },
    );
    run.run(&text).await;
    let _ = reply.await;
}

async fn send_passive(creds: &BotCreds, base: &str, peer: &Peer, msg_id: &str, msg_seq: u64, content: &str) {
    let Some(token) = creds.token().await else {
        eprintln!("[qqbot] 回复失败：access_token 获取失败");
        return;
    };
    let (path, body) = match peer {
        Peer::Group(g) => (
            format!("/v2/groups/{g}/messages"),
            json!({ "content": content, "msg_type": 0, "msg_id": msg_id, "msg_seq": msg_seq }),
        ),
        Peer::C2C(u) => (
            format!("/v2/users/{u}/messages"),
            json!({ "content": content, "msg_type": 0, "msg_id": msg_id, "msg_seq": msg_seq }),
        ),
        Peer::Guild(c) => (
            format!("/channels/{c}/messages"),
            json!({ "content": content, "msg_id": msg_id, "msg_seq": msg_seq }),
        ),
    };
    match creds
        .client
        .post(format!("{base}{path}"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!("[qqbot] 回复失败：HTTP {status} {body}");
        }
        Err(e) => eprintln!("[qqbot] 回复网络错误：{e}"),
    }
}

// ---------------------------------------------------------------- 测试（纯函数部分）

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 消息解析：群 / 单聊 / 公域频道三种事件形态各自取到正确的会话端点与正文
    #[test]
    fn 消息解析_群单聊频道三形态() {
        // 群聊：group_openid 优先
        let (peer, msg_id, content) = parse_message(
            "GROUP_AT_MESSAGE_CREATE",
            &json!({ "id": "m1", "group_openid": "G1", "content": "<@!bot>在吗" }),
        )
        .expect("群事件应可解析");
        assert_eq!(peer.key(), "G1");
        assert!(!matches!(peer, Peer::C2C(_)), "群聊不是私聊");
        assert_eq!(msg_id, "m1");
        assert_eq!(content, "在吗", "群 @ 消息应剥掉提及片段");

        // 单聊：user_openid；缺 author.user_openid 时兜底指针
        let (peer, _, content) = parse_message(
            "C2C_MESSAGE_CREATE",
            &json!({ "id": "m2", "user_openid": "U9", "content": "帮我查天气" }),
        )
        .expect("单聊事件应可解析");
        assert!(matches!(peer, Peer::C2C(_)), "C2C 应识别为私聊（门控豁免依据）");
        assert_eq!(content, "帮我查天气");

        // 公域频道：channel_id
        let (peer, _, _) = parse_message(
            "AT_MESSAGE_CREATE",
            &json!({ "id": "m3", "channel_id": "CH1", "content": "hi" }),
        )
        .expect("频道事件应可解析");
        assert!(matches!(peer, Peer::Guild(_)));

        // 缺 msg_id / 缺端点 → 不可路由，直接放弃
        assert!(parse_message("GROUP_AT_MESSAGE_CREATE", &json!({ "content": "x" })).is_none());
        assert!(parse_message("GROUP_AT_MESSAGE_CREATE", &json!({ "id": "m4" })).is_none());
        // 未知事件类型不解析
        assert!(parse_message("DIRECT_MESSAGE_CREATE", &json!({ "id": "m5" })).is_none());
    }

    /// 提及剥离：完整片段剥掉、未闭合片段保持原样（宁可带杂质也不吞正文）
    #[test]
    fn 提及剥离_未闭合保持原样() {
        assert_eq!(strip_mentions("<@!abc123>在吗"), "在吗");
        assert_eq!(strip_mentions("前缀<@x>中缀<@y>后缀"), "前缀中缀后缀");
        assert_eq!(strip_mentions("没有提及"), "没有提及");
        assert_eq!(strip_mentions("未闭合<@abc"), "未闭合<@abc", "找不到 > 时不再吞后面正文");
    }

    /// 回复上限：群/单聊文本限 2000 字节（CJK 按 650 字符收敛），公域频道走常规上限
    #[test]
    fn 回复上限_按端点收敛() {
        assert_eq!(Peer::Group("g".into()).max_chars(), 650);
        assert_eq!(Peer::C2C("u".into()).max_chars(), 650);
        assert_eq!(Peer::Guild("c".into()).max_chars(), 3800);
    }

    /// 附件图片判定：按扩展名整词比对（大小写不敏感），假扩展名与无扩展名不算
    #[test]
    fn 图片文件名判定_扩展名整词() {
        assert!(is_image_name("shot.png"));
        assert!(is_image_name("IMG_0001.JPG"), "大小写不敏感");
        assert!(is_image_name("a.jpeg") && is_image_name("b.gif") && is_image_name("c.webp"));
        assert!(!is_image_name("a.png.txt"), "中间含 .png 不算图片（曾用 contains 误判）");
        assert!(!is_image_name("report.pdf"));
        assert!(!is_image_name("noext"), "无扩展名不算图片");
    }
}
