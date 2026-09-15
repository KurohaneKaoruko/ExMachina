//! QQ 官方机器人适配器（qqbot.rs）：q.qq.com 开放平台 WebSocket 长连接。
//! 鉴权链：appId + appSecret → getAppAccessToken（7200s，本地缓存提前 5 分钟刷新）
//! → WS identify（token 格式 `QQBot {access_token}`）。
//! intents：GROUP_AND_C2C_EVENT(1<<25) 群/单聊 + PUBLIC_GUILD_MESSAGES(1<<30) 公域频道。
//! 被动回复：群 /v2/groups/{group_openid}/messages、单聊 /v2/users/{user_openid}/messages、
//! 频道 /channels/{channel_id}/messages —— 均须携带事件里的 msg_id + 递增 msg_seq
//! （被动消息 5 分钟有效；群/单聊文本限 2000 字节，收束陈述截断到 650 字符）。
//! 断线重连：优先 Resume（补发漏掉的事件），Invalid Session(op9) 回退重新 Identify；
//! config.sandbox = "true" 时走沙箱 openapi（sandbox.api.sgroup.qq.com）。
//! 监督循环每 5 秒对账：新增账号拉起会话，删除/停用/凭证变更的账号回收任务。

use crate::platform::{flatten_statements, report_status, Channel};
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
                                    }
                                    "RESUMED" => {
                                        report_status(&ch.id, "ok", "会话已恢复");
                                    }
                                    "GROUP_AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" | "AT_MESSAGE_CREATE" => {
                                        let Some((peer, msg_id, content)) = parse_message(t, &d) else { continue };
                                        if content.trim().is_empty() {
                                            continue;
                                        }
                                        if !ch.allowed_chats.is_empty()
                                            && !ch.allowed_chats.iter().any(|a| a == peer.key())
                                        {
                                            eprintln!("[qqbot:{}] {} 不在白名单，已忽略", ch.id, peer.key());
                                            continue;
                                        }
                                        let core = core.clone();
                                        let ch2 = ch.clone();
                                        let creds2 = creds.clone();
                                        tokio::spawn(async move {
                                            handle_message(&core, &ch2, &creds2, base, peer, msg_id, content.trim()).await;
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
        tokio::time::sleep(Duration::from_secs(3)).await;
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

/// 频道 @ 消息正文剥掉 `<@…>` 提及片段
fn strip_mentions(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<@") {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(j) => rest = &rest[i + j + 1..],
            None => break,
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
    msg_id: String,
    text: &str,
) {
    // 组绑定：账号的智能体组（不存在则回落激活组）
    let prev_group = core.active_group();
    let mut switched = false;
    if let Some(g) = &ch.group {
        if *g != prev_group && core.group_meta(g).is_some() {
            switched = core.registry().set_active_group(g).is_ok();
        }
    }
    let gid = core.active_group();
    let title = format!("channel:{}:{}", ch.id, peer.key());
    let session = core
        .list_sessions_in_group(&gid)
        .ok()
        .and_then(|list| list.into_iter().find(|s| s.title == title))
        .or_else(|| core.create_session(&title).ok());
    let Some(session) = session else {
        if switched {
            let _ = core.registry().set_active_group(&prev_group);
        }
        return;
    };

    // 回复任务：运行结束把收束陈述发回原会话（被动回复：msg_id + 递增 msg_seq）
    let seq = Arc::new(AtomicU64::new(0));
    let mut rx = core.subscribe();
    let sid = session.id.clone();
    let creds = creds.clone();
    let peer2 = peer.clone();
    let msg_id2 = msg_id.clone();
    let max_chars = peer.max_chars();
    let base = base.to_string();
    let reply = tokio::spawn(async move {
        while let Ok(evt) = rx.recv().await {
            if evt.session_id != sid {
                continue;
            }
            let text = if evt.kind == "run.finished" {
                flatten_statements(&evt.payload, max_chars)
            } else if evt.kind == "run.error" {
                let msg = evt.payload.get("message").and_then(|v| v.as_str()).unwrap_or("运行失败");
                format!("【警告】{msg}")
                    .chars()
                    .take(max_chars)
                    .collect::<String>()
            } else {
                continue;
            };
            let n = seq.fetch_add(1, Ordering::Relaxed) + 1;
            send_passive(&creds, &base, &peer2, &msg_id2, n, &text).await;
            break;
        }
    });

    if let Err(e) = core.chat(&session.id, text).await {
        eprintln!("[qqbot:{}] 执行失败：{e}", ch.id);
    }
    if switched {
        let _ = core.registry().set_active_group(&prev_group);
    }
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
