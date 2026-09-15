//! OneBot 11 平台适配器（napcat.rs）：适配 NapCat / Lagrange 等_onebot11_ 实现。
//! 接法：正向 WebSocket —— 连接 OneBot 实现的 WS 服务端（config.url，可选 config.token 访问令牌），
//! 收 `post_type=message` 事件 → 绑定组内执行 → `send_group_msg` / `send_private_msg` 回复。
//! 监督循环每 5 秒对账：新增账号拉起连接，删除/停用/地址或令牌变更的账号回收任务。
//!
//! 收发解耦：入站读取不阻塞（消息处理全部 spawn），动作经 mpsc 通道交给专职写任务，
//! 避免长运行期间无法应答 WS 层 Ping 被服务端断开。

use crate::platform::{flatten_statements, report_status, Channel};
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
                    ws
                }
                Err(e) => {
                    let msg = format!("连接失败：{e}");
                    eprintln!("[napcat:{}] {msg}", ch.id);
                    report_status(&ch.id, "error", msg);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
    let text = extract_text(v.get("message").unwrap_or(&serde_json::Value::Null));
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
    tokio::spawn(async move { handle_message(&core, &ch, &tx, group_id, user_id, &text).await });
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
            None => break,
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
    let peer = group_id.map(|g| g.to_string()).or(user_id.map(|u| u.to_string())).unwrap_or_default();
    let title = format!("channel:{}:{peer}", ch.id);
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

    // 回复任务：运行结束把收束陈述发回原会话（群聊优先，无群号走私聊）
    let mut rx = core.subscribe();
    let sid = session.id.clone();
    let tx = tx.clone();
    let reply = tokio::spawn(async move {
        while let Ok(evt) = rx.recv().await {
            if evt.session_id != sid {
                continue;
            }
            if evt.kind == "run.finished" {
                let text = flatten_statements(&evt.payload, 3500);
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
                break;
            }
            if evt.kind == "run.error" {
                let msg = evt.payload.get("message").and_then(|v| v.as_str()).unwrap_or("运行失败");
                let action = match group_id {
                    Some(g) => serde_json::json!({
                        "action": "send_group_msg",
                        "params": { "group_id": g, "message": [{ "type": "text", "data": { "text": format!("【警告】{msg}") } }] },
                    }),
                    None => serde_json::json!({
                        "action": "send_private_msg",
                        "params": { "user_id": user_id.unwrap_or(0), "message": [{ "type": "text", "data": { "text": format!("【警告】{msg}") } }] },
                    }),
                };
                let _ = tx.send(action);
                break;
            }
        }
    });

    if let Err(e) = core.chat(&session.id, text).await {
        eprintln!("[napcat:{}] 执行失败：{e}", ch.id);
    }
    if switched {
        let _ = core.registry().set_active_group(&prev_group);
    }
    let _ = reply.await;
}
