//! Slack 适配器（slack.rs）：Socket Mode —— 免公网回调地址的官方接法。
//! 鉴权链：app 级令牌（config.appToken，`xapp-…`，需 `connections:write` 权限）
//! → POST apps.connections.open 取 WSS 地址；机器人令牌（顶层 token，`xoxb-…`）
//! → POST chat.postMessage 回帖。
//! 事件：`hello` → `events_api` 信封；**每条信封须在 3 秒内 ack**（回 `{"envelope_id":…}`），
//! 故 ack 一律先发、再异步处理消息。忽略机器人自己发的消息（bot_id / subtype / bot_message）。
//! 回复文本限 4000 字符 → 截 3800。`disconnect` 帧按服务端要求重连。
//! 监督循环每 5 秒对账：新增账号拉起会话，删除/停用/凭证变更的账号回收任务。

use crate::platform::{admit, report_status, spawn_reply, GateDecision, Channel, ChannelRun, InboundCtx, MediaItem};
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

const API: &str = "https://slack.com/api";
/// chat.postMessage 单条上限 4000 字符，留出余量
const MAX_CHARS: usize = 3800;

struct Sess {
    handle: JoinHandle<()>,
    /// 凭证指纹（appToken + bot token）：变更即重开会话
    fingerprint: String,
}

fn sessions() -> &'static Mutex<HashMap<String, Sess>> {
    static SESS: OnceLock<Mutex<HashMap<String, Sess>>> = OnceLock::new();
    SESS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fingerprint(ch: &Channel) -> String {
    format!("{}|{}", ch.cfg("appToken").unwrap_or_default(), ch.token.clone().unwrap_or_default())
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
            c.kind == "slack"
                && c.enabled
                && c.cfg("appToken").is_some()
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
    let app_token = ch.cfg("appToken").unwrap_or_default();
    let bot_token = ch.token.clone().unwrap_or_default();
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(30)).build() else {
        eprintln!("[slack:{}] HTTP 客户端构建失败", ch.id);
        return;
    };
    // 自检：机器人令牌有效性 + 工作区身份
    match client
        .post(format!("{API}/auth.test"))
        .header("Authorization", format!("Bearer {bot_token}"))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            let v: Value = r.json().await.unwrap_or(Value::Null);
            if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                let name = v.get("user").and_then(|x| x.as_str()).unwrap_or("?");
                let team = v.get("team").and_then(|x| x.as_str()).unwrap_or("");
                println!("[slack:{}] 机器人已连结：{name}（{team}）", ch.id);
                report_status(&ch.id, "ok", format!("机器人 {name}"));
            } else {
                let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("unknown");
                let msg = format!("机器人令牌校验失败：{err}");
                eprintln!("[slack:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
            }
        }
        _ => {
            let msg = "机器人令牌校验请求失败";
            eprintln!("[slack:{}] {msg}", ch.id);
            report_status(&ch.id, "error", msg);
        }
    }

    loop {
        // Socket Mode 连接地址（应用级令牌换取）
        let url = match client
            .post(format!("{API}/apps.connections.open"))
            .header("Authorization", format!("Bearer {app_token}"))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                let v: Value = r.json().await.unwrap_or(Value::Null);
                if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                    v.get("url").and_then(|u| u.as_str()).map(|u| u.to_string())
                } else {
                    let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("unknown");
                    let msg = format!("Socket Mode 开启失败：{err}（检查 appToken 与 connections:write 权限）");
                    eprintln!("[slack:{}] {msg}", ch.id);
                    report_status(&ch.id, "error", msg);
                    None
                }
            }
            Ok(r) => {
                let msg = format!("Socket Mode 开启 HTTP {}", r.status());
                eprintln!("[slack:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                None
            }
            Err(e) => {
                let msg = format!("Socket Mode 开启网络错误：{e}");
                eprintln!("[slack:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                None
            }
        };
        let Some(url) = url else {
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        };
        let (ws, _) = match connect_async(&url).await {
            Ok(x) => x,
            Err(e) => {
                let msg = format!("WS 连接失败：{e}");
                eprintln!("[slack:{}] {msg}", ch.id);
                report_status(&ch.id, "error", msg);
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };
        let (mut write, mut read) = ws.split();

        while let Some(msg) = read.next().await {
            let text = match msg {
                Ok(Message::Text(s)) => s,
                Ok(Message::Close(c)) => {
                    eprintln!("[slack:{}] 服务端关闭：{c:?}", ch.id);
                    report_status(&ch.id, "error", "服务端关闭连接");
                    break;
                }
                Ok(_) => continue,
                Err(e) => {
                    let msg = format!("WS 错误：{e}");
                    eprintln!("[slack:{}] {msg}", ch.id);
                    report_status(&ch.id, "error", msg);
                    break;
                }
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            match v.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                "hello" => {
                    println!("[slack:{}] Socket Mode 已连结", ch.id);
                    report_status(&ch.id, "ok", "Socket Mode 已连结");
                }
                "disconnect" => {
                    let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("unknown");
                    eprintln!("[slack:{}] 服务端要求重连：{reason}", ch.id);
                    report_status(&ch.id, "error", format!("服务端要求重连：{reason}"));
                    break;
                }
                "events_api" => {
                    // 先 ack（3 秒内），再处理
                    if let Some(id) = v.get("envelope_id").and_then(|x| x.as_str()) {
                        let ack = json!({ "envelope_id": id }).to_string();
                        if write.send(Message::text(ack)).await.is_err() {
                            break;
                        }
                    }
                    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
                    let Some((channel, mentioned, text)) = parse_event(&payload) else { continue };
                    if text.trim().is_empty() && payload.pointer("/event/files").and_then(|x| x.as_array()).map(|a| a.is_empty()).unwrap_or(true) {
                        continue;
                    }
                    if !ch.allowed_chats.is_empty() && !ch.allowed_chats.iter().any(|a| a == &channel) {
                        eprintln!("[slack:{}] {channel} 不在白名单，已忽略", ch.id);
                        continue;
                    }
                    // 入站媒体（组 6.4）：event.files → url_private_download（带 bot token）
                    let mut inbound: Vec<(String, String, String, String)> = Vec::new();
                    if let Some(files) = payload.pointer("/event/files").and_then(|x| x.as_array()) {
                        for f in files {
                            let url = f.get("url_private_download").or_else(|| f.get("url_private")).and_then(|x| x.as_str()).unwrap_or("");
                            let name = f.get("name").and_then(|x| x.as_str()).unwrap_or("file.bin");
                            if url.is_empty() {
                                continue;
                            }
                            let kind = if f.get("mimetype").and_then(|x| x.as_str()).unwrap_or("").starts_with("image/") { "image" } else { "file" };
                            inbound.push((kind.to_string(), url.to_string(), name.to_string(), bot_token.clone()));
                        }
                    }
                    // DM（channel 以 D 开头）豁免群聊门控；app_mention 事件即提及信号
                    let is_group = !channel.starts_with('D');
                    let external_id = payload
                        .pointer("/event/user")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let gate_ctx = InboundCtx { text: &text, external_id: &external_id, display_name: &external_id, is_group, mentioned };
                    match admit(core.as_ref(), &ch, &gate_ctx).await {
                        GateDecision::Allow => {}
                        GateDecision::Ignore => continue,
                        GateDecision::Deny(reply) => {
                            let client2 = client.clone();
                            let token2 = bot_token.clone();
                            let chan = channel.clone();
                            tokio::spawn(async move { send_message(&client2, &token2, &chan, &reply).await });
                            continue;
                        }
                    }
                    let core = core.clone();
                    let ch2 = ch.clone();
                    let client2 = client.clone();
                    let token2 = bot_token.clone();
                    tokio::spawn(async move {
                        // 入站媒体（组 6.4）：下载落 inbox → 注入
                        let mut saved: Vec<(String, String, String)> = Vec::new();
                        for (kind, url, name, tk) in inbound {
                            if let Some(bytes) = crate::platform::download_bytes_auth(&url, &tk).await {
                                if let Some(p) = crate::platform::save_inbound_media(&core, &ch2, &name, bytes).await {
                                    saved.push((kind, p, name));
                                }
                            }
                        }
                        handle_message(&core, &ch2, &client2, &token2, &channel, &external_id, saved, text.trim()).await;
                    });
                }
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// events_api payload → (频道 id, 是否提及(app_mention), 正文)。机器人自身消息与非文本消息一律忽略。
fn parse_event(payload: &Value) -> Option<(String, bool, String)> {
    let ev = payload.get("event")?;
    let t = ev.get("type").and_then(|x| x.as_str()).unwrap_or("");
    if t != "message" && t != "app_mention" {
        return None;
    }
    // bot_id = 其他机器人/自己；subtype = 编辑、加入频道等系统事件
    if ev.get("bot_id").is_some() || ev.get("subtype").is_some() {
        return None;
    }
    let channel = ev.get("channel").and_then(|x| x.as_str())?.to_string();
    let content = ev.get("text").and_then(|x| x.as_str()).unwrap_or("");
    Some((channel, t == "app_mention", strip_mentions(content)))
}

/// 剥掉 `<@U123>` 提及片段
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
    bot_token: &str,
    channel: &str,
    external_id: &str,
    media: Vec<(String, String, String)>,
    text: &str,
) {
    let Some((run, rx)) = ChannelRun::begin(core, ch, channel).await else { return };
    core.stamp_session_origin(&run.session_id, external_id, core.identity_of(&ch.id, external_id).map(|i| i.id).unwrap_or_else(|| format!("ch:{}:{}", ch.id, external_id)).as_str());
    // 入站媒体注入（组 6.4）
    let note = crate::platform::stage_inbound_media(core, &run.session_id, &media);
    let text = if note.is_empty() { text.to_string() } else { format!("{text}{note}") };
    let client2 = client.clone();
    let token2 = bot_token.to_string();
    let chan = channel.to_string();
    let media_client = client.clone();
    let media_token = bot_token.to_string();
    let media_chan = chan.clone();
    let reply = spawn_reply(
        &run,
        rx,
        MAX_CHARS,
        move |text| {
            let client2 = client2.clone();
            let token2 = token2.clone();
            let chan = chan.clone();
            async move {
                send_message(&client2, &token2, &chan, &text).await;
            }
        },
        move |item| {
            let client2 = media_client.clone();
            let token2 = media_token.clone();
            let chan = media_chan.clone();
            async move { slack_send_media(&client2, &token2, &chan, item).await }
        },
    );
    run.run(&text).await;
    let _ = reply.await;
}

/// slack 媒体直发（组 6.2）：getUploadURLExternal → POST 上传 → completeV2 三步
async fn slack_send_media(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
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
    // ① 预签名上传地址
    let up: Value = match client
        .post(format!("{API}/files.getUploadURLExternal"))
        .bearer_auth(bot_token)
        .query(&[("filename", name.as_str()), ("length", &bytes.len().to_string())])
        .send()
        .await
    {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => v,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    let Some(upload_url) = up.get("upload_url").and_then(|x| x.as_str()).map(|s| s.to_string()) else {
        return false;
    };
    let Some(file_id) = up.get("file_id").and_then(|x| x.as_str()).map(|s| s.to_string()) else {
        return false;
    };
    // ② 直传字节
    let resp = match client
        .post(&upload_url)
        .header("Content-Type", "application/octet-stream")
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
    // ③ 完成登记（带频道与文案）
    let done: Value = match client
        .post(format!("{API}/files.completeV2"))
        .bearer_auth(bot_token)
        .form(&[
            ("files", file_id.as_str()),
            ("channel_id", channel),
            ("initial_comment", item.caption.as_str()),
        ])
        .send()
        .await
    {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => v,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    done.get("ok").and_then(|x| x.as_bool()).unwrap_or(false)
}

async fn send_message(client: &reqwest::Client, bot_token: &str, channel: &str, text: &str) {
    if bot_token.trim().is_empty() {
        return;
    }
    match client
        .post(format!("{API}/chat.postMessage"))
        .header("Authorization", format!("Bearer {bot_token}"))
        .json(&json!({ "channel": channel, "text": text }))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            // HTTP 200 也可能是 {ok:false}——Slack 的错误在响应体里
            let v: Value = r.json().await.unwrap_or(Value::Null);
            if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
                let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("unknown");
                eprintln!("[slack] 回复失败：{err}");
            }
        }
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!("[slack] 回复失败：HTTP {status} {body}");
        }
        Err(e) => eprintln!("[slack] 回复网络错误：{e}"),
    }
}
