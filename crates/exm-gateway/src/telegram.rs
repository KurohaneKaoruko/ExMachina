//! Telegram 平台适配器（docs/11）：长轮询收消息 → 绑定组内执行 → sendMessage 回复。
//! 每个启用的 telegram 通道 = 一个账号 = 一个独立轮询任务；账号绑定组后消息在该组上下文执行。
//! 监督循环每 5 秒对账：新增账号拉起轮询，删除/停用/token 变更的账号回收任务。

use crate::platform::Channel;
use exm_core::Core;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::task::JoinHandle;

struct Poller {
    handle: JoinHandle<()>,
    /// token 指纹（尾部 6 字符）：token 变更即重启轮询
    fingerprint: String,
}

fn pollers() -> &'static Mutex<HashMap<String, Poller>> {
    static POLLERS: OnceLock<Mutex<HashMap<String, Poller>>> = OnceLock::new();
    POLLERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint(token: &Option<String>) -> String {
    token
        .as_deref()
        .unwrap_or("")
        .chars()
        .rev()
        .take(6)
        .collect()
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
        .filter(|c| {
            c.kind == "telegram"
                && c.enabled
                && c.token.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false)
        })
        .collect();

    let mut guards = pollers().lock().unwrap();
    // 回收：已删除 / 停用 / token 变更 / 已退出的账号
    let stale: Vec<String> = guards
        .iter()
        .filter(|(id, p)| {
            match desired.iter().find(|c| &c.id == *id) {
                Some(c) => p.fingerprint != fingerprint(&c.token) || p.handle.is_finished(),
                None => true,
            }
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in &stale {
        if let Some(p) = guards.remove(id) {
            p.handle.abort();
        }
    }
    // 拉起缺失的账号
    for ch in &desired {
        if guards.contains_key(&ch.id) {
            continue;
        }
        let core = core.clone();
        let ch_task = ch.clone();
        let fp = fingerprint(&ch.token);
        let handle = tokio::spawn(async move { poll_loop(core, ch_task).await });
        guards.insert(ch.id.clone(), Poller { handle, fingerprint: fp });
    }
}

async fn poll_loop(core: Arc<Core>, ch: Channel) {
    let token = ch.token.clone().unwrap_or_default();
    let api = format!("https://api.telegram.org/bot{}", token.trim());
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[telegram:{}] 客户端构建失败：{e}", ch.id);
            return;
        }
    };
    // 自检：拿到机器人身份（失败只记日志，通道保留待用户修正 token）
    match client.get(format!("{api}/getMe")).send().await {
        Ok(r) if r.status().is_success() => {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                let name = v.pointer("/result/username").and_then(|x| x.as_str()).unwrap_or("?");
                println!("[telegram:{}] 机器人已连结：@{name}", ch.id);
            }
        }
        Ok(r) => eprintln!("[telegram:{}] token 校验失败：HTTP {}", ch.id, r.status()),
        Err(e) => eprintln!("[telegram:{}] token 校验网络错误：{e}", ch.id),
    }

    let mut offset: i64 = 0;
    loop {
        let resp = client
            .get(format!("{api}/getUpdates"))
            .query(&[
                ("timeout", "25".to_string()),
                ("offset", offset.to_string()),
                ("allowed_updates", "[\"message\"]".to_string()),
            ])
            .send()
            .await;
        let body = match resp {
            Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[telegram:{}] 响应解析失败：{e}", ch.id);
                    continue;
                }
            },
            Ok(r) => {
                eprintln!("[telegram:{}] getUpdates HTTP {}", ch.id, r.status());
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            Err(e) => {
                eprintln!("[telegram:{}] getUpdates 网络错误：{e}", ch.id);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };
        let updates = body
            .pointer("/result")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for u in updates {
            offset = u["update_id"].as_i64().unwrap_or(offset) + 1;
            let text = u
                .pointer("/message/text")
                .and_then(|v| v.as_str())
                .map(String::from);
            let chat_id = u.pointer("/message/chat/id").and_then(|v| v.as_i64());
            if let (Some(text), Some(chat_id)) = (text, chat_id) {
                if text.trim().is_empty() {
                    continue;
                }
                handle_message(&core, &ch, chat_id, &text).await;
            }
        }
    }
}

/// 一条用户消息：组绑定 → 会话复用 → 订阅回复 → 执行 → sendMessage
async fn handle_message(core: &Arc<Core>, ch: &Channel, chat_id: i64, text: &str) {
    // 组绑定：账号的智能体组（不存在则回落激活组）
    let prev_group = core.active_group();
    let mut switched = false;
    if let Some(g) = &ch.group {
        if *g != prev_group && core.group_meta(g).is_some() {
            switched = core.registry().set_active_group(g).is_ok();
        }
    }
    let gid = core.active_group();
    let title = format!("channel:{}:{chat_id}", ch.id);
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

    // 回复任务：运行结束把收束陈述发回 Telegram
    let token = ch.token.clone().unwrap_or_default();
    let mut rx = core.subscribe();
    let sid = session.id.clone();
    let reply = tokio::spawn(async move {
        while let Ok(evt) = rx.recv().await {
            if evt.session_id != sid {
                continue;
            }
            if evt.kind == "run.finished" {
                send_message(&token, chat_id, &flatten_statements(&evt.payload)).await;
                break;
            }
            if evt.kind == "run.error" {
                let msg = evt
                    .payload
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("运行失败");
                send_message(&token, chat_id, &format!("【警告】{msg}")).await;
                break;
            }
        }
    });

    let orch = core.orchestrator();
    if let Err(e) = orch.handle_user_message(&session.id, text).await {
        eprintln!("[telegram:{}] 执行失败：{e}", ch.id);
    }
    if switched {
        let _ = core.registry().set_active_group(&prev_group);
    }
    let _ = reply.await;
}

fn flatten_statements(payload: &serde_json::Value) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(list) = payload.get("statements").and_then(|v| v.as_array()) {
        for s in list {
            let tag = s.get("tag").and_then(|v| v.as_str()).unwrap_or("报告");
            let text = s.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.trim().is_empty() {
                continue;
            }
            lines.push(format!("【{tag}】{text}"));
        }
    }
    let mut text = lines.join("\n");
    if text.trim().is_empty() {
        text = "（本轮无收束输出）".into();
    }
    text.chars().take(3800).collect()
}

async fn send_message(token: &str, chat_id: i64, text: &str) {
    if token.trim().is_empty() {
        return;
    }
    let api = format!("https://api.telegram.org/bot{}", token.trim());
    let _ = reqwest::Client::new()
        .post(&api)
        .json(&serde_json::json!({ "chat_id": chat_id, "text": text }))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await;
}
