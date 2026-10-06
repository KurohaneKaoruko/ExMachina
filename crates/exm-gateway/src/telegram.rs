//! Telegram 平台适配器（docs/11）：长轮询收消息 → 绑定组内执行 → sendMessage 回复。
//! 每个启用的 telegram 通道 = 一个账号 = 一个独立轮询任务；账号绑定组后消息在该组上下文执行。
//! 监督循环每 5 秒对账：新增账号拉起轮询，删除/停用/token 变更的账号回收任务。

use crate::platform::{admit, report_status, spawn_reply, GateDecision, Channel, ChannelRun, InboundCtx};
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
    let mut bot_name = String::new();
    match client.get(format!("{api}/getMe")).send().await {
        Ok(r) if r.status().is_success() => {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                let name = v.pointer("/result/username").and_then(|x| x.as_str()).unwrap_or("?");
                bot_name = name.to_string();
                println!("[telegram:{}] 机器人已连结：@{name}", ch.id);
                report_status(&ch.id, "ok", format!("@{name}"));
            }
        }
        Ok(r) => {
            let msg = format!("token 校验失败：HTTP {}", r.status());
            eprintln!("[telegram:{}] {msg}", ch.id);
            report_status(&ch.id, "error", msg);
        }
        Err(e) => {
            let msg = format!("网络错误：{e}");
            eprintln!("[telegram:{}] token 校验{msg}", ch.id);
            report_status(&ch.id, "error", msg);
        }
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
                    let msg = format!("响应解析失败：{e}");
                    eprintln!("[telegram:{}] {msg}", ch.id);
                    report_status(&ch.id, "error", msg);
                    continue;
                }
            },
            Ok(r) => {
                let msg = format!("getUpdates HTTP {}", r.status());
                eprintln!("[telegram:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            Err(e) => {
                let msg = format!("getUpdates 网络错误：{e}");
                eprintln!("[telegram:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };
        // 轮询正常即报 ok（长轮询期间连接活着）
        report_status(&ch.id, "ok", if bot_name.is_empty() { "轮询中".into() } else { format!("@{bot_name}") });
        let updates = body
            .pointer("/result")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for u in updates {
            offset = u["update_id"].as_i64().unwrap_or(offset) + 1;
            let chat_id = u.pointer("/message/chat/id").and_then(|v| v.as_i64());
            let Some(chat_id) = chat_id else { continue };
            // 会话白名单：非空时仅放行清单内的 chat
            if !ch.allowed_chats.is_empty()
                && !ch.allowed_chats.iter().any(|a| a == &chat_id.to_string())
            {
                eprintln!("[telegram:{}] chat {chat_id} 不在白名单，已忽略", ch.id);
                continue;
            }
            let text = match u.pointer("/message/text").and_then(|v| v.as_str()) {
                Some(t) => Some(t.to_string()),
                None => {
                    // 语音消息：getFile 下载 ogg → 转写为文本
                    match u.pointer("/message/voice/file_id").and_then(|v| v.as_str()) {
                        Some(file_id) => {
                            let api = format!("https://api.telegram.org/bot{}", ch.token.as_deref().unwrap_or_default());
                            match download_voice(&api, file_id).await {
                                Ok(bytes) if !bytes.is_empty() => {
                                    match core.transcribe(&bytes, "voice.ogg").await {
                                        Ok(t) if !t.trim().is_empty() => Some(t),
                                        Ok(_) => None,
                                        Err(e) => {
                                            eprintln!("[telegram:{}] 语音转写失败：{e}", ch.id);
                                            None
                                        }
                                    }
                                }
                                _ => None,
                            }
                        }
                        None => None,
                    }
                }
            };
            if let Some(text) = text {
                if text.trim().is_empty() {
                    continue;
                }
                // 入站闸门：/pair 兑换 → 群聊唤醒门控 → 身份管控（capability-completion 组 3）
                let from_id = u.pointer("/message/from/id").and_then(|v| v.as_i64()).unwrap_or(0).to_string();
                let display = u
                    .pointer("/message/from/username")
                    .or_else(|| u.pointer("/message/from/first_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_group = matches!(
                    u.pointer("/message/chat/type").and_then(|v| v.as_str()),
                    Some("group") | Some("supergroup")
                );
                let mut mentioned = u
                    .pointer("/message/reply_to_message/from/is_bot")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_group && !mentioned {
                    // 提及归一化：实体 mention 且 @ 的正是本 bot（bot 身份经 getMe 缓存）
                    let (bot_id, bot_name) = bot_info(ch.token.as_deref().unwrap_or_default()).await;
                    if u.pointer("/message/reply_to_message/from/id").and_then(|v| v.as_i64())
                        == bot_id.parse::<i64>().ok()
                    {
                        mentioned = true;
                    }
                    if let Some(entities) = u.pointer("/message/entities").and_then(|v| v.as_array()) {
                        let hit = entities.iter().any(|e| {
                            e.get("type").and_then(|x| x.as_str()) == Some("mention")
                                && !bot_name.is_empty()
                                && text.to_lowercase().contains(&format!("@{}", bot_name.to_lowercase()))
                        });
                        mentioned = mentioned || hit;
                    }
                }
                match admit(&core, &ch, &InboundCtx {
                    text: &text, external_id: &from_id, display_name: &display, is_group, mentioned,
                }).await {
                    GateDecision::Allow => handle_message(&core, &ch, chat_id, &from_id, &text).await,
                    GateDecision::Ignore => {}
                    GateDecision::Deny(reply) => {
                        send_message(ch.token.as_deref().unwrap_or_default(), chat_id, &reply).await;
                    }
                }
            }
        }
    }
}

/// bot 身份缓存（token → (id, username)）：群聊提及归一化用，懒获取一次
async fn bot_info(token: &str) -> (String, String) {
    static CACHE: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();
    let map = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = map.lock().unwrap().get(token) {
        return v.clone();
    }
    let info = match reqwest::Client::new()
        .get(format!("https://api.telegram.org/bot{}/getMe", token.trim()))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(v) => match (
                v.pointer("/result/id").and_then(|x| x.as_i64()),
                v.pointer("/result/username").and_then(|x| x.as_str()),
            ) {
                (Some(id), Some(name)) => (id.to_string(), name.to_string()),
                _ => (String::new(), String::new()),
            },
            Err(_) => (String::new(), String::new()),
        },
        Err(_) => (String::new(), String::new()),
    };
    map.lock().unwrap().insert(token.to_string(), info.clone());
    info
}

/// 一条用户消息：组绑定 → 会话复用 → 订阅回复 → 执行 → sendMessage
async fn handle_message(core: &Arc<Core>, ch: &Channel, chat_id: i64, external_id: &str, text: &str) {
    let Some((run, rx)) = ChannelRun::begin(core, ch, &chat_id.to_string()).await else {
        return;
    };
    core.stamp_session_origin(&run.session_id, external_id, core.identity_of(&ch.id, external_id).map(|i| i.role).as_deref().unwrap_or(""));
    let token = ch.token.clone().unwrap_or_default();
    let reply = spawn_reply(&run, rx, 3800, move |text| async move {
        send_message(&token, chat_id, &text).await;
    });
    run.run(text).await;
    let _ = reply.await;
}

/// 下载语音文件（getFile → download，ogg/opus 字节）
async fn download_voice(api: &str, file_id: &str) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let meta: serde_json::Value = client
        .get(format!("{api}/getFile"))
        .query(&[("file_id", file_id)])
        .send()
        .await?
        .json()
        .await?;
    let path = meta
        .pointer("/result/file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("getFile 缺少 file_path"))?;
    let bytes = client
        .get(format!("https://api.telegram.org/file/bot{}/{path}", path.trim_start_matches('/')))
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await?
        .bytes()
        .await?;
    Ok(bytes.to_vec())
}

async fn send_message(token: &str, chat_id: i64, text: &str) {
    if token.trim().is_empty() {
        return;
    }
    // ⚠️ Bot API 的方法名必须拼在路径上（/sendMessage）——曾漏掉，导致回帖静默失败
    let api = format!("https://api.telegram.org/bot{}/sendMessage", token.trim());
    match reqwest::Client::new()
        .post(&api)
        .json(&serde_json::json!({ "chat_id": chat_id, "text": text }))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!("[telegram] 回复失败：HTTP {status} {body}");
        }
        Err(e) => eprintln!("[telegram] 回复网络错误：{e}"),
    }
}
