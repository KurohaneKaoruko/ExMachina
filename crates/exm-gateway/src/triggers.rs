//! 事件触发器 —— 文件监听（7.1）与通用事件 webhook（7.2）（docs/10，design D6）
//!
//! 边界：本层只做「事件采集 → 去抖/过滤 → 映射」，注入会话复用 Core 的事件通路
//! （`run_event_prompt`：组切换 + 会话复用 + prompt 注入），不新建执行模型。
//! 安全口径：监听遵循工作区范围；webhook 独立路径 + 独立 HMAC 密钥，失败计入审计。

use crate::AppState;
use axum::extract::ConnectInfo;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router};
use exm_core::types::EventTrigger;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 事件队列上限（触发风暴兜底，design 风险表）：同窗口待注入事件超过即丢弃最旧
const PENDING_CAP: usize = 256;
/// 单次注入合并的事件明细上限（超出部分以计数代替，避免提示词爆炸）
const MERGE_DETAIL_CAP: usize = 8;

// ---------------------------------------------------------------- 文件监听（7.1）

/// 归一化的文件变更事件（适配器无关，测试可直接构造）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEvent {
    /// 相对工作区的路径（`/` 分隔）
    pub rel: String,
    /// created | modified | removed
    pub kind: String,
}

/// 命中排除前缀（相对路径前缀口径：`.exmachina/`、`target/`、`node_modules/` 等）
pub(crate) fn excluded(rel: &str, exclude: &[String]) -> bool {
    let rel = rel.trim_start_matches("./");
    exclude.iter().any(|p| {
        let p = p.trim().trim_end_matches('/');
        !p.is_empty() && (rel == p || rel.starts_with(&format!("{p}/")))
    })
}

/// 触发器 glob 是否命中：`**` 跨目录；无 `/` 的模式同时按文件名匹配（`*.rs` 命中任意目录下的 rs）
pub(crate) fn glob_hits(pattern: &str, rel: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if !pattern.contains('/') {
        if let Ok(g) = globset::Glob::new(pattern) {
            if g.compile_matcher().is_match(name) {
                return true;
            }
        }
    }
    match globset::GlobBuilder::new(pattern).literal_separator(true).build() {
        Ok(g) => g.compile_matcher().is_match(rel),
        Err(_) => false,
    }
}

/// 待注入缓冲：同路径连续变更只保留最新事件（去抖合并的记账侧）
#[derive(Default)]
struct Pending {
    map: HashMap<String, (FsEvent, Instant)>,
    order: VecDeque<String>,
}

impl Pending {
    fn push(&mut self, ev: FsEvent) -> bool {
        if self.map.len() >= PENDING_CAP && !self.map.contains_key(&ev.rel) {
            // 队列上限：丢最旧（触发风暴兜底）
            while let Some(oldest) = self.order.pop_front() {
                if self.map.remove(&oldest).is_some() {
                    break;
                }
            }
        }
        let fresh = !self.map.contains_key(&ev.rel);
        if fresh {
            self.order.push_back(ev.rel.clone());
        }
        let rel = ev.rel.clone();
        self.map.insert(rel, (ev, Instant::now()));
        fresh
    }

    /// 取出去抖窗口已过的批次（全部到期事件合并为一次注入明细）
    fn drain_due(&mut self, debounce: Duration) -> Vec<FsEvent> {
        let mut due: Vec<FsEvent> = Vec::new();
        let keys: Vec<String> = self.order.iter().cloned().collect();
        for k in keys {
            if let Some((ev, at)) = self.map.get(&k) {
                if at.elapsed() >= debounce {
                    due.push(ev.clone());
                    self.map.remove(&k);
                }
            }
        }
        self.order.retain(|k| self.map.contains_key(k));
        due
    }
}

fn pending() -> &'static parking_lot::Mutex<Pending> {
    static P: OnceLock<parking_lot::Mutex<Pending>> = OnceLock::new();
    P.get_or_init(|| parking_lot::Mutex::new(Pending::default()))
}

/// 把一批到期事件折叠为注入摘要：同路径一次、明细封顶、超出计数
fn merge_summary(events: &[FsEvent]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for ev in events.iter().take(MERGE_DETAIL_CAP) {
        lines.push(format!("{} {}", ev.kind, ev.rel));
    }
    if events.len() > MERGE_DETAIL_CAP {
        lines.push(format!("…（另有 {} 个变更）", events.len() - MERGE_DETAIL_CAP));
    }
    lines.join("\n")
}

/// 启动文件监听（`triggers.fileWatch.enabled` 时由 serve 调起；其余情形静默不启动）
pub fn spawn_file_watcher(core: Arc<exm_core::Core>) {
    let cfg = core.config().triggers.file_watch.clone();
    if !cfg.enabled {
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<FsEvent>();
    let ws = core.config().workspace_root.clone();
    let exclude = cfg.exclude.clone();
    let ws_for_cb = ws.clone();
    let watcher_res = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        let Ok(ev) = res else { return };
        for path in ev.paths {
            let Ok(abs) = path.canonicalize() else { continue };
            let Ok(rel) = abs.strip_prefix(&ws_for_cb) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel.is_empty() || excluded(&rel, &exclude) {
                continue;
            }
            let kind = match ev.kind {
                notify::EventKind::Create(_) => "created",
                notify::EventKind::Modify(_) => "modified",
                notify::EventKind::Remove(_) => "removed",
                _ => continue,
            };
            let _ = tx.send(FsEvent { rel, kind: kind.to_string() });
        }
    });
    let mut watcher = match watcher_res {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[triggers] 文件监听启动失败：{e}");
            return;
        }
    };
    if let Err(e) = notify::Watcher::watch(&mut watcher, &ws, notify::RecursiveMode::Recursive) {
        eprintln!("[triggers] 监听目录失败：{e}");
        return;
    }
    println!(
        "[triggers] 文件监听已启动（去抖 {}ms，排除 {}）",
        cfg.debounce_ms,
        cfg.exclude.join(", ")
    );
    let debounce = Duration::from_millis(cfg.debounce_ms.max(50));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(150));
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let due: Vec<FsEvent> = pending().lock().drain_due(debounce);
                    if !due.is_empty() {
                        dispatch_fs_events(&core, &due).await;
                    }
                }
                ev = rx.recv() => {
                    match ev {
                        Some(ev) => {
                            pending().lock().push(ev);
                        }
                        None => break,
                    }
                }
            }
        }
    });
}

/// 一批到期事件 → 命中的启用触发器 → 注入会话（同批事件合并为一次注入；
/// 排除前缀在此二次过滤：监听回调与派发两侧同口径，防绕过）
pub(crate) async fn dispatch_fs_events(core: &exm_core::Core, events: &[FsEvent]) {
    let triggers: Vec<EventTrigger> = core
        .store
        .list_event_triggers(Some("file_watch"))
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.enabled)
        .collect();
    if triggers.is_empty() {
        return;
    }
    let exclude = core.config().triggers.file_watch.exclude.clone();
    let events: Vec<FsEvent> = events
        .iter()
        .filter(|ev| !excluded(&ev.rel, &exclude))
        .cloned()
        .collect();
    if events.is_empty() {
        return;
    }
    let summary = merge_summary(&events);
    for t in triggers {
        let hit = events.iter().any(|ev| glob_hits(&t.pattern, &ev.rel));
        if !hit {
            continue;
        }
        let prompt = t.prompt.replace("{event}", &summary);
        match core.run_event_prompt(&format!("trigger-{}", t.id), t.group.as_deref(), &prompt).await {
            Ok(sid) => {
                let _ = core
                    .store
                    .audit_tool("triggers", "file_watch", &json!({ "triggerId": t.id, "events": events.len() }), &format!("注入会话 {sid}"), 0);
                let _ = core.events.send(exm_core::types::CoreEvent {
                    kind: "event.fired".into(),
                    session_id: sid,
                    payload: json!({ "triggerId": t.id, "kind": "file_watch", "summary": summary }),
                });
            }
            Err(e) => {
                eprintln!("[triggers] 事件注入失败（trigger {}）：{e}", t.id);
            }
        }
    }
}

// ---------------------------------------------------------------- 通用事件 webhook（7.2）

/// HMAC-SHA256 签名（十六进制小写）；头部形态 `sha256=<hex>`
fn verify_signature(secret: &str, body: &[u8], header: Option<&str>) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let Some(sig) = header.and_then(|h| h.strip_prefix("sha256=")) else {
        return false;
    };
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    // 常量时间比较（hex 手工比对，避免引入额外依赖）
    let expect_hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();
    if sig.len() != expect_hex.len() {
        return false;
    }
    sig.bytes()
        .zip(expect_hex.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// 事件载荷摘要（审计与注入共用）：类型 + 压缩后的数据（封顶）
fn payload_digest(kind: &str, data: &Value) -> String {
    let data_str = serde_json::to_string(data).unwrap_or_default();
    let data_str: String = data_str.chars().take(600).collect();
    format!("{kind} {data_str}")
}

/// 通用事件入口：签名校验 → 映射规则（event_webhook 触发器）→ 注入；失败拒绝并审计
pub async fn event_webhook(
    State(st): State<AppState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let cfg = st.core.config().triggers.event_webhook.clone();
    let audit = |reason: &str, extra: Value| {
        let _ = st.core.store.audit_tool(
            "event-webhook",
            "event_webhook",
            &json!({ "source": addr.to_string(), "reason": reason, "extra": extra }),
            reason,
            0,
        );
    };
    if !cfg.enabled || cfg.secret.trim().is_empty() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "事件入口未启用" }))).into_response();
    }
    let sig = headers.get("x-exm-signature").and_then(|v| v.to_str().ok());
    if !verify_signature(&cfg.secret, &body, sig) {
        audit("签名校验失败", json!({ "bodyBytes": body.len() }));
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "签名校验失败" }))).into_response();
    }
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            audit("载荷解析失败", json!({ "error": e.to_string() }));
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "载荷须为 JSON" }))).into_response();
        }
    };
    let kind = parsed
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let data = parsed.get("data").cloned().unwrap_or(Value::Null);
    let digest = payload_digest(&kind, &data);

    let triggers: Vec<EventTrigger> = st
        .core
        .store
        .list_event_triggers(Some("event_webhook"))
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.enabled && (t.pattern.trim() == "*" || t.pattern.trim() == kind))
        .collect();
    let mut injected: Vec<String> = Vec::new();
    for t in triggers {
        let prompt = t.prompt.replace("{event}", &digest);
        if let Ok(sid) = st
            .core
            .run_event_prompt(&format!("trigger-{}", t.id), t.group.as_deref(), &prompt)
            .await
        {
            injected.push(format!("{}→{sid}", t.id));
        }
    }
    audit("事件已接收", json!({ "type": kind, "injected": injected }));
    Json(json!({ "ok": true, "injected": injected.len() })).into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/events/webhook", axum::routing::post(event_webhook))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(pattern: &str) -> EventTrigger {
        EventTrigger {
            id: "t1".into(),
            kind: "file_watch".into(),
            name: "测试触发器".into(),
            pattern: pattern.into(),
            group: None,
            prompt: "事件：{event}".into(),
            enabled: true,
            created_at: String::new(),
            last_fired_at: None,
        }
    }

    // ---------------- 排除与匹配 ----------------

    #[test]
    fn 排除前缀命中() {
        let exclude = vec![".exmachina/".into(), "target/".into(), "node_modules/".into()];
        assert!(excluded(".exmachina/data/foo.json", &exclude));
        assert!(excluded("target/debug/a.rs", &exclude));
        assert!(excluded("node_modules/pkg/index.js", &exclude));
        assert!(!excluded("src/main.rs", &exclude));
        assert!(!excluded("src/targets/a.rs", &exclude), "仅前缀段命中，非字符串前缀");
    }

    #[test]
    fn glob_匹配_跨目录与文件名() {
        assert!(glob_hits("**/*.rs", "src/main.rs"));
        assert!(glob_hits("*.rs", "main.rs"));
        assert!(glob_hits("*.rs", "src/main.rs"), "无 / 的模式按文件名兜底匹配");
        assert!(glob_hits("docs/*.md", "docs/x.md"));
        assert!(!glob_hits("docs/*.md", "docs/sub/x.md"), "单星不跨目录");
        assert!(!glob_hits("**/*.rs", "src/main.ts"));
        assert!(!glob_hits("", "anything"));
    }

    // ---------------- 去抖合并 ----------------

    #[test]
    fn 同路径连续变更合并为一次() {
        let mut p = Pending::default();
        for _ in 0..5 {
            p.push(FsEvent { rel: "src/lib.rs".into(), kind: "modified".into() });
        }
        // 去抖窗口未过：不产出
        assert!(p.drain_due(Duration::from_secs(60)).is_empty());
        // 窗口已过（时间戳已到期）：一次产出、内容为最新事件
        // （直接把时间戳改成已过期来模拟窗口流逝）
        let ev = p.map.get_mut("src/lib.rs").unwrap();
        ev.1 = Instant::now() - Duration::from_secs(3600);
        let due = p.drain_due(Duration::from_secs(60));
        assert_eq!(due.len(), 1, "同路径连续变更应合并为一次注入");
        assert_eq!(due[0].rel, "src/lib.rs");
        assert!(p.drain_due(Duration::from_secs(60)).is_empty(), "取走即清空");
    }

    #[test]
    fn 多路径一次批量产出且封顶() {
        let mut p = Pending::default();
        for i in 0..(MERGE_DETAIL_CAP + 5) {
            p.push(FsEvent { rel: format!("src/f{i}.rs"), kind: "created".into() });
        }
        for (_, slot) in p.map.iter_mut() {
            slot.1 = Instant::now() - Duration::from_secs(3600);
        }
        let due = p.drain_due(Duration::from_secs(60));
        assert_eq!(due.len(), MERGE_DETAIL_CAP + 5);
        let summary = merge_summary(&due);
        assert!(summary.contains("…（另有 5 个变更）"), "超出明细上限应计数: {summary}");
        assert_eq!(summary.lines().count(), MERGE_DETAIL_CAP + 1);
    }

    #[test]
    fn 队列上限_丢弃最旧() {
        let mut p = Pending::default();
        for i in 0..(PENDING_CAP + 10) {
            p.push(FsEvent { rel: format!("f/{i}"), kind: "modified".into() });
        }
        assert!(p.map.len() <= PENDING_CAP, "队列上限兜底: {}", p.map.len());
        assert!(!p.map.contains_key("f/0"), "最旧事件应被丢弃");
        assert!(p.map.contains_key(&format!("f/{}", PENDING_CAP + 9)), "最新事件保留");
    }

    // ---------------- 签名校验（7.2） ----------------

    #[test]
    fn 签名_有效与拒绝() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let secret = "s3cret";
        let body = br#"{"type":"deploy","data":{"env":"prod"}}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let good = format!("sha256={}", hex(&mac.finalize().into_bytes()));
        assert!(verify_signature(secret, body, Some(&good)));
        assert!(!verify_signature(secret, body, Some("sha256=deadbeef")));
        assert!(!verify_signature(secret, body, Some(&good[..good.len() - 2])));
        assert!(!verify_signature(secret, body, None));
        assert!(!verify_signature(secret, b"tampered", Some(&good)));
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn 签名_密钥错误拒绝() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let body = b"{}";
        let mut mac = Hmac::<Sha256>::new_from_slice(b"other-key").unwrap();
        mac.update(body);
        let sig = format!("sha256={}", hex(&mac.finalize().into_bytes()));
        assert!(!verify_signature("right-key", body, Some(&sig)));
    }

    // ---------------- 事件映射（7.2 口径复用 glob_hits 的通配语义） ----------------

    #[test]
    fn 事件类型映射_通配与精确() {
        // event_webhook 触发器的 pattern = 事件类型；* = 全部
        let t = trigger("*");
        assert_eq!(t.pattern.trim(), "*");
        let t2 = trigger("deploy");
        assert_eq!(t2.pattern.trim(), "deploy");
    }

    #[test]
    fn 载荷摘要封顶() {
        let big = json!({ "data": "x".repeat(5000) });
        let digest = payload_digest("deploy", &big);
        assert!(digest.chars().count() <= 700, "摘要应封顶: {}", digest.chars().count());
    }

    // ---------------- 临时目录集成：连续变更合并为一次注入（7.1 验收口径） ----------------

    fn orch_def() -> exm_core::types::AgentDefinition {
        exm_core::types::AgentDefinition {
            name: "值班体".into(),
            identifier: "duty-orch".into(),
            domain: "测试".into(),
            tier: exm_core::types::Tier::Orchestrator,
            description: "测试主智能体".into(),
            capabilities: vec![],
            tools: vec![],
            when_to_call: String::new(),
            dependencies: vec![],
            composable_with: vec![],
            input_schema: Default::default(),
            output_schema: Default::default(),
            prompt_file: String::new(),
            model_hint: None,
        }
    }

    #[tokio::test]
    async fn 临时目录_连续变更合并为一次注入() {
        let dir = std::env::temp_dir().join(format!("exm-fwatch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = exm_core::config::ExmConfig::load(&dir);
        cfg.use_mock = true; // 注入执行走替身通道（测试不依赖真实模型与网络）
        let core = Arc::new(exm_core::Core::with_config(cfg).unwrap());
        core.registry().create_group(Some("t".into()), "监听测试组", "").unwrap();
        core.registry().upsert_agent("t", orch_def(), None).unwrap();
        core.registry().set_primary("t", "duty-orch").unwrap();
        core.registry().set_active_group("t").unwrap();

        // 注册启用的文件监听触发器：命中 src/**，注入模板带 {event} 占位
        core.store
            .put_event_trigger(&EventTrigger {
                id: "fw1".into(),
                kind: "file_watch".into(),
                name: "源码监听".into(),
                pattern: "src/**/*.rs".into(),
                group: Some("t".into()),
                prompt: "工作区事件：{event}\n请检查相关变更。".into(),
                enabled: true,
                created_at: String::new(),
                last_fired_at: None,
            })
            .unwrap();

        // 一次风暴：同路径连续 3 次 + 另两路径各 1 次（排除前缀的路径不应触发）
        let events = vec![
            FsEvent { rel: "src/lib.rs".into(), kind: "modified".into() },
            FsEvent { rel: "src/lib.rs".into(), kind: "modified".into() },
            FsEvent { rel: "src/lib.rs".into(), kind: "modified".into() },
            FsEvent { rel: "src/main.rs".into(), kind: "created".into() },
            FsEvent { rel: "target/debug/x.rs".into(), kind: "created".into() }, // 排除前缀
        ];
        dispatch_fs_events(&core, &events).await;

        // 断言：目标组只建了一个触发器会话，用户消息只有 1 条且携带合并摘要
        let sessions = core.list_sessions_in_group("t").unwrap();
        let s = sessions.iter().find(|s| s.title == "trigger-fw1").expect("注入会话应存在");
        let msgs = core.store.list_messages(&s.id, 100).unwrap();
        let user_msgs: Vec<_> = msgs.iter().filter(|m| matches!(m.role, exm_core::types::MessageRole::User)).collect();
        assert_eq!(user_msgs.len(), 1, "连续变更应合并为一次注入，实际 {} 条", user_msgs.len());
        let text = user_msgs[0].statements.iter().map(|s| s.text.clone()).collect::<String>();
        assert!(text.contains("modified src/lib.rs"), "摘要含变更明细：{text}");
        assert!(text.contains("created src/main.rs"), "多路径并入同一次注入：{text}");
        assert!(!text.contains("target/debug"), "排除前缀路径不注入：{text}");

        // 未命中 pattern 的变更不触发（docs/*.md 不匹配 src/**）
        let before = core.store.list_messages(&s.id, 100).unwrap().len();
        dispatch_fs_events(&core, &[FsEvent { rel: "docs/x.md".into(), kind: "created".into() }]).await;
        let after = core.store.list_messages(&s.id, 100).unwrap().len();
        assert_eq!(before, after, "pattern 未命中不得注入");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------------- 事件 webhook 映射注入（7.2 集成：有效签名 → 注入） ----------------

    #[tokio::test]
    async fn 事件webhook_有效签名注入与审计() {
        use axum::body::Bytes;
        use axum::extract::ConnectInfo;
        use hmac::Mac;
        let dir = std::env::temp_dir().join(format!("exm-ewh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = exm_core::config::ExmConfig::load(&dir);
        cfg.use_mock = true;
        cfg.triggers.event_webhook.enabled = true;
        cfg.triggers.event_webhook.secret = "test-secret".into();
        let core = Arc::new(exm_core::Core::with_config(cfg).unwrap());
        core.registry().create_group(Some("t".into()), "事件测试组", "").unwrap();
        core.registry().upsert_agent("t", orch_def(), None).unwrap();
        core.registry().set_primary("t", "duty-orch").unwrap();
        core.registry().set_active_group("t").unwrap();
        core.store
            .put_event_trigger(&EventTrigger {
                id: "ew1".into(),
                kind: "event_webhook".into(),
                name: "部署事件".into(),
                pattern: "deploy".into(),
                group: Some("t".into()),
                prompt: "外部事件：{event}".into(),
                enabled: true,
                created_at: String::new(),
                last_fired_at: None,
            })
            .unwrap();

        // 状态与请求体
        let st = AppState { core: core.clone() };
        let body = br#"{"type":"deploy","data":{"env":"prod","rev":"a1b2c3"}}"#;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"test-secret").unwrap();
        mac.update(body);
        let sig: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-exm-signature", format!("sha256={sig}").parse().unwrap());
        let addr = ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 40000)));

        let resp = event_webhook(State(st.clone()), addr, headers, Bytes::from_static(body)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK, "有效签名应受理");
        // 注入落地：事件会话建立且用户消息携带载荷摘要
        let sessions = core.list_sessions_in_group("t").unwrap();
        let s = sessions.iter().find(|x| x.title == "trigger-ew1").expect("映射注入应建会话");
        let msgs = core.store.list_messages(&s.id, 100).unwrap();
        let text = msgs
            .iter()
            .filter(|m| matches!(m.role, exm_core::types::MessageRole::User))
            .flat_map(|m| m.statements.iter().map(|x| x.text.clone()))
            .collect::<String>();
        assert!(text.contains("deploy") && text.contains("prod"), "注入含类型与载荷摘要：{text}");
        // 审计：成功接收留痕
        let audit = core.store.list_tool_audit("event-webhook", 10).unwrap();
        assert!(audit.iter().any(|a| a["resultSummary"] == "事件已接收"), "有效事件入审计");

        // 无效签名：拒绝 + 审计 + 不注入
        let sessions_before = core.list_sessions_in_group("t").unwrap().len();
        let mut bad_headers = axum::http::HeaderMap::new();
        bad_headers.insert("x-exm-signature", "sha256=deadbeef".parse().unwrap());
        let resp2 = event_webhook(
            State(st),
            addr,
            bad_headers,
            Bytes::from_static(br#"{"type":"deploy","data":{}}"#),
        )
        .await
        .into_response();
        assert_eq!(resp2.status(), StatusCode::FORBIDDEN, "签名失败应拒绝");
        let audit2 = core.store.list_tool_audit("event-webhook", 10).unwrap();
        assert!(audit2.iter().any(|a| a["resultSummary"] == "签名校验失败"), "失败原因入审计");
        assert_eq!(
            core.list_sessions_in_group("t").unwrap().len(),
            sessions_before,
            "拒绝的调用不产生注入副作用"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
