//! Matrix 适配器（matrix.rs）：Client-Server API 长轮询 `/sync`（任何 homeserver 通用）。
//! 凭证：`config.homeserver`（如 https://matrix.org）+ 顶层 token（access token，
//! 从 Element「设置 → 帮助与关于 → 访问令牌」获取，或应用服务签发）。
//! 自检：GET /account/whoami 取自己的 user_id（同时用于过滤自己发的消息）。
//! 首次 sync 只取 `next_batch` 游标并丢弃历史事件，其后逐批处理 join 房间里的
//! `m.room.message` 文本事件（时间线倒序不需要，按批处理即可）。
//! 回复：PUT /rooms/{roomId}/send/m.room.message/{txnId}（txn 自增，幂等去重）。
//! token 失效（M_UNKNOWN_TOKEN）上报错误后指数退避重试。
//! 监督循环每 5 秒对账：新增账号拉起轮询，删除/停用/凭证变更的账号回收任务。

use crate::platform::{report_status, spawn_reply, Channel, ChannelRun};
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
                        for (room, sender, text) in parse_events(&body, &self_user) {
                            if text.trim().is_empty() {
                                continue;
                            }
                            if !ch.allowed_chats.is_empty() && !ch.allowed_chats.iter().any(|a| a == &room) {
                                eprintln!("[matrix:{}] {room} 不在白名单，已忽略", ch.id);
                                continue;
                            }
                            let core = core.clone();
                            let ch2 = ch.clone();
                            let hs2 = hs.clone();
                            let token2 = token.clone();
                            let client2 = client.clone();
                            let sender2 = sender.clone();
                            tokio::spawn(async move {
                                handle_message(&core, &ch2, &client2, &hs2, &token2, &room, &sender2, text.trim()).await;
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

/// /sync 响应 → [(房间 id, 发送者, 正文)]。自己发的消息与非文本消息一律忽略。
fn parse_events(body: &Value, self_user: &str) -> Vec<(String, String, String)> {
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
            if content.get("msgtype").and_then(|x| x.as_str()) != Some("m.text") {
                continue;
            }
            let sender = e.get("sender").and_then(|x| x.as_str()).unwrap_or("");
            if !self_user.is_empty() && sender == self_user {
                continue;
            }
            let text = content.get("body").and_then(|x| x.as_str()).unwrap_or("");
            out.push((room_id.clone(), sender.to_string(), text.to_string()));
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
    text: &str,
) {
    // 会话键用房间 id，房间内多人共享同一会话（Matrix 房间即群）
    let Some((run, rx)) = ChannelRun::begin(core, ch, room).await else { return };
    let _ = sender;
    let client2 = client.clone();
    let hs2 = hs.to_string();
    let token2 = token.to_string();
    let room2 = room.to_string();
    let reply = spawn_reply(&run, rx, MAX_CHARS, move |text| async move {
        send_message(&client2, &hs2, &token2, &room2, &text).await;
    });
    run.run(text).await;
    let _ = reply.await;
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
