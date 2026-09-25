//! Discord 适配器（discord.rs）：Gateway WebSocket 长连接（API v10）。
//! 自检：GET /users/@me（bot token 校验 + 机器人身份）；
//! 网关：GET /gateway/bot → `wss://…/?v=10&encoding=json` → HELLO(op10) → Identify(op2)。
//! intents = GUILDS | GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT；
//! 其中 **MESSAGE_CONTENT 是特权 intent**，须在开发者门户 → Bot 页勾选，
//! 否则服务器频道里的消息正文为空（私聊与 @ 提及不受此限）。
//! 回复：POST /channels/{channel_id}/messages（单条限 2000 字符 → 截 1800）。
//! 断线优先 Resume(op6，补发漏掉的事件)；op9 Invalid Session 回退重新 Identify。
//! 监督循环每 5 秒对账：新增账号拉起会话，删除/停用/token 变更的账号回收任务。

use crate::platform::{report_status, spawn_reply, Channel, ChannelRun};
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
                                        let user = d.pointer("/user/username").and_then(|x| x.as_str()).unwrap_or("?");
                                        println!("[discord:{}] 机器人已连结：{user}", ch.id);
                                        report_status(&ch.id, "ok", format!("机器人 {user}"));
                                    }
                                    "RESUMED" => report_status(&ch.id, "ok", "会话已恢复"),
                                    "MESSAGE_CREATE" => {
                                        let Some((channel_id, text)) = parse_message(&d) else { continue };
                                        if text.trim().is_empty() {
                                            continue;
                                        }
                                        if !ch.allowed_chats.is_empty()
                                            && !ch.allowed_chats.iter().any(|a| a == &channel_id)
                                        {
                                            eprintln!("[discord:{}] {channel_id} 不在白名单，已忽略", ch.id);
                                            continue;
                                        }
                                        let core = core.clone();
                                        let ch2 = ch.clone();
                                        let client2 = client.clone();
                                        let token2 = token.clone();
                                        tokio::spawn(async move {
                                            handle_message(&core, &ch2, &client2, &token2, &channel_id, text.trim()).await;
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
        tokio::time::sleep(Duration::from_secs(3)).await;
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

/// 剥掉 `<@123456>` / `<@&123456>` 提及片段
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
    out.trim().to_string()
}

/// 一条用户消息：组绑定 → 会话复用 → 订阅回复 → 执行 → 频道回帖
async fn handle_message(
    core: &Arc<Core>,
    ch: &Channel,
    client: &reqwest::Client,
    token: &str,
    channel_id: &str,
    text: &str,
) {
    let Some((run, rx)) = ChannelRun::begin(core, ch, channel_id).await else { return };
    let client2 = client.clone();
    let token2 = token.to_string();
    let cid = channel_id.to_string();
    let reply = spawn_reply(&run, rx, MAX_CHARS, move |text| async move {
        send_message(&client2, &token2, &cid, &text).await;
    });
    run.run(text).await;
    let _ = reply.await;
}

async fn send_message(client: &reqwest::Client, token: &str, channel_id: &str, text: &str) {
    if token.trim().is_empty() {
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
