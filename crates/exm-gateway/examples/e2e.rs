//! 端到端验收（Rust 版，替代原 TS 脚本）
//!
//! 覆盖：REST 全端点 + 核心事件流 + 任务图 + 证据 + 三账 + 配置/记忆接口
//! 运行：cargo run -p exm-gateway --example e2e
use axum::response::IntoResponse;
use axum::Json;
#[allow(unused_imports)]
use serde_json::json;
use exm_core::{config::ExmConfig, Core};
use std::sync::Arc;
use std::time::Duration;

struct Checker {
    pass: usize,
    fail: usize,
}

impl Checker {
    fn new() -> Self {
        Checker { pass: 0, fail: 0 }
    }
    fn check(&mut self, name: &str, ok: bool) {
        if ok {
            self.pass += 1;
            println!("  PASS {name}");
        } else {
            self.fail += 1;
            println!("  FAIL {name}");
        }
    }
}

fn find_root() -> std::path::PathBuf {
    let mut dir = std::env::current_dir().unwrap();
    loop {
        if dir.join("agents").join("definitions").exists() {
            return dir;
        }
        if !dir.pop() {
            panic!("未找到工作区根目录");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = find_root();
    // 清理上次运行残留（组数据与激活组状态在 agents/ 内持久化）
    let _ = std::fs::remove_dir_all(root.join("agents").join("groups").join("product"));
    let _ = std::fs::remove_dir_all(root.join("agents").join("groups").join("e2e-chan"));
    let _ = std::fs::write(root.join("agents").join("active_group"), "default");
    let _ = std::fs::remove_file(root.join("agents").join("active_single"));
    // 清理残留通道（只动 e2e 自己的条目）
    {
        let ch_path = root.join(".exmachina").join("data").join("channels.json");
        if let Ok(raw) = std::fs::read_to_string(&ch_path) {
            if let Ok(mut list) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) {
                list.retain(|c| c["id"] != "e2e-hook" && c["id"] != "e2e-bound" && c["id"] != "e2e-qq" && c["id"] != "e2e-tg");
                let _ = std::fs::write(&ch_path, serde_json::to_string_pretty(&list).unwrap_or_default());
            }
        }
    }

    let mut cfg = ExmConfig::load(&root);
    cfg.data_dir = std::env::temp_dir().join(format!("exm-gw-e2e-{}", uuid::Uuid::new_v4()));
    cfg.use_mock = true;
    // e2e 环境自持：不随开发者本地配置漂移（本机 config 曾关掉深层记忆导致记忆断言全挂）
    cfg.memory_enabled = true;
    cfg.agents_dir = root.join("agents");
    let data_dir = cfg.data_dir.clone();
    let core = Arc::new(Core::with_config(cfg)?);

    // 事件订阅（等价于 WS 扇出源；WS 只是把同一事件推给浏览器）
    let mut rx = core.subscribe();
    let events_task = tokio::spawn(async move {
        let mut kinds = std::collections::HashSet::new();
        while let Ok(evt) = rx.recv().await {
            kinds.insert(evt.kind.clone());
            if evt.kind == "run.finished" || evt.kind == "run.error" {
                break;
            }
        }
        kinds
    });

    let router = exm_gateway::build_router(core.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let base = format!("http://127.0.0.1:{port}/api");
    let client = reqwest::Client::new();
    let mut c = Checker::new();

    // 健康与编成（编成是数据：校验一致性，不锁定具体数量）
    let health: serde_json::Value = client.get(format!("{base}/health")).send().await?.json().await?;
    c.check("health ok 且个体 >=13", health["ok"] == true && health["agents"].as_u64().unwrap_or(0) >= 13);

    let agents: serde_json::Value = client.get(format!("{base}/agents")).send().await?.json().await?;
    let agents_len = agents.as_array().map(|a| a.len()).unwrap_or(0);
    c.check(
        "agents 数量与 health 一致",
        agents_len as u64 == health["agents"].as_u64().unwrap_or(0) && agents_len >= 13,
    );
    let agents_all_valid = agents.as_array().map(|list| {
        list.iter().all(|a| {
            a["identifier"].as_str().map(|s| !s.is_empty()).unwrap_or(false)
                && a["description"].as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)
        })
    }).unwrap_or(false);
    c.check("agents 契约字段齐备", agents_all_valid);

    let playbooks: serde_json::Value = client.get(format!("{base}/playbooks")).send().await?.json().await?;
    let playbooks_valid = playbooks.as_array().map(|ps| {
        !ps.is_empty()
            && ps.iter().all(|p| {
                p["steps"].as_array().map(|s| !s.is_empty()).unwrap_or(false)
                    && p["terminal"].as_str().map(|t| !t.is_empty()).unwrap_or(false)
            })
    }).unwrap_or(false);
    c.check("playbooks 非空且步骤齐备", playbooks_valid);

    // 会话与对话
    let session: serde_json::Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({"title": "e2e"}))
        .send()
        .await?
        .json()
        .await?;
    let sid = session["id"].as_str().unwrap_or_default().to_string();
    c.check("会话创建", !sid.is_empty());

    // 技能包（docs/10）：写入后热装载，任务目标命中触发词即随派发携带
    let skills_dir = root.join("agents").join("skills");
    let _ = std::fs::create_dir_all(&skills_dir);
    let _ = std::fs::write(
        skills_dir.join("e2e-skill.json"),
        serde_json::json!({
            "id": "e2e-skill", "name": "E2E 技能", "description": "验收用",
            "triggers": ["调度"], "instructions": "E2E 技能注入标记：调度任务需说明并发池与拓扑排序。",
            "agents": [], "createdAt": "2026-09-12T00:00:00.000Z"
        })
        .to_string(),
    );

    let resp = client
        .post(format!("{base}/sessions/{sid}/chat"))
        .json(&serde_json::json!({"text": "设计一个支持 1000 个智能体的集群调度方案"}))
        .send()
        .await?;
    c.check("chat 202 受理", resp.status().as_u16() == 202);

    let kinds = tokio::time::timeout(Duration::from_secs(60), events_task).await??;
    for k in [
        "orchestrator.token",
        "unit.token",
        "graph.updated",
        "dispatch.sent",
        "sync.received",
        "ledger.updated",
        "memory.recall",
        "memory.written",
        "run.finished",
    ] {
        c.check(&format!("事件 {k}"), kinds.contains(k));
    }

    // 技能注入断言：派发事件（持久化 Envelope type=dispatch）的指令包含 kind=skill 输入
    let events = core.store.list_events(&sid).unwrap_or_default();
    let skill_injected = events.iter().any(|e| {
        e["type"] == "dispatch"
            && e["payload"]["inputs"]
                .as_array()
                .map(|inputs| inputs.iter().any(|i| i["kind"] == "skill"))
                .unwrap_or(false)
    });
    c.check("技能包随派发注入", skill_injected);
    let _ = std::fs::remove_file(skills_dir.join("e2e-skill.json"));

    // 任务图 / 证据 / 三账
    let graph: serde_json::Value = client.get(format!("{base}/sessions/{sid}/graph")).send().await?.json().await?;
    let nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
    c.check("图 >=4 节点", nodes.len() >= 4);
    c.check(
        "节点全终态",
        nodes.iter().all(|n| matches!(n["status"].as_str(), Some("done") | Some("blocked") | Some("failed"))),
    );
    c.check(
        "存在并行根节点",
        nodes.iter().filter(|n| n["dependsOn"].as_array().map(|a| a.is_empty()).unwrap_or(false)).count() >= 2,
    );

    let evidence: serde_json::Value = client.get(format!("{base}/sessions/{sid}/evidence")).send().await?.json().await?;
    c.check("证据账有记录", evidence.as_array().map(|a| !a.is_empty()).unwrap_or(false));

    let detail: serde_json::Value = client.get(format!("{base}/sessions/{sid}")).send().await?.json().await?;
    c.check("三账：任务目标", !detail["ledger"]["task"]["goal"].as_str().unwrap_or("").is_empty());
    c.check(
        "三账：证据确认项",
        detail["ledger"]["evidence"]["confirmed"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
    );

    let messages: serde_json::Value = client.get(format!("{base}/sessions/{sid}/messages")).send().await?.json().await?;
    let roles: Vec<String> = messages
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m["role"].as_str().map(|s| s.to_string()))
        .collect();
    c.check(
        "消息含 user/unit/orchestrator",
        ["user", "unit", "orchestrator"].iter().all(|r| roles.iter().any(|x| x == r)),
    );

    // 配置接口 + Schema
    let cfg_before: serde_json::Value = client.get(format!("{base}/config")).send().await?.json().await?;
    c.check("config GET（mock 通道）", cfg_before["mock"] == true);
    let put = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({"maxConcurrency": 3, "memory": {"recallLimit": 6}}))
        .send()
        .await?;
    c.check("config PUT 200", put.status().is_success());
    c.check("配置热生效", core.config().max_concurrency == 3 && core.config().memory_recall_limit == 6);
    let schema: serde_json::Value = client.get(format!("{base}/config/schema")).send().await?.json().await?;
    c.check(
        "config schema 有分组与字段",
        schema["groups"].as_array().map(|g| !g.is_empty()).unwrap_or(false),
    );

    // 记忆接口
    let stats: serde_json::Value = client.get(format!("{base}/memory/stats")).send().await?.json().await?;
    c.check("记忆库有条目", stats["memory"]["total"].as_u64().unwrap_or(0) > 0);
    c.check("个体可靠性统计", stats["agentStats"].as_array().map(|a| !a.is_empty()).unwrap_or(false));

    let search: serde_json::Value = client
        .post(format!("{base}/memory/search"))
        .json(&serde_json::json!({"query": "架构风险 决策", "limit": 5}))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "记忆检索有命中且带理由",
        search["hits"]
            .as_array()
            .map(|a| !a.is_empty() && a[0]["reasons"].as_array().map(|r| !r.is_empty()).unwrap_or(false))
            .unwrap_or(false),
    );

    let added: serde_json::Value = client
        .post(format!("{base}/memory"))
        .json(&serde_json::json!({
            "kind": "fact", "title": "构建入口", "body": "cargo build --release", "pin": true
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("记忆写入（群体）", added["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false));

    // 个体记忆：写入归属 scout-agent 的私有记忆，验证不可见于他人检索
    let priv_add: serde_json::Value = client
        .post(format!("{base}/memory"))
        .json(&serde_json::json!({
            "kind": "lesson", "title": "侦察体私有教训", "body": "入口侦察需先确认缓存目录",
            "agentId": "scout-agent", "importance": 0.9
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "个体记忆写入",
        priv_add["agentId"].as_str() == Some("scout-agent"),
    );
    let others: serde_json::Value = client
        .post(format!("{base}/memory/search"))
        .json(&serde_json::json!({"query": "入口侦察 缓存", "limit": 5, "agent": "context-agent"}))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "个体记忆隔离：他人检索不可见",
        others["hits"].as_array().map(|a| {
            a.iter().all(|h| h["entry"]["agentId"].as_str() != Some("scout-agent"))
        }).unwrap_or(true),
    );
    let own: serde_json::Value = client
        .post(format!("{base}/memory/search"))
        .json(&serde_json::json!({"query": "入口侦察 缓存", "limit": 5, "agent": "scout-agent"}))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "个体记忆隔离：归属者可见",
        own["hits"].as_array().map(|a| {
            a.iter().any(|h| h["entry"]["agentId"].as_str() == Some("scout-agent"))
        }).unwrap_or(false),
    );

    let list: serde_json::Value = client.get(format!("{base}/memory?limit=50")).send().await?.json().await?;
    c.check("记忆列表含固定项", list.as_array().map(|a| a.iter().any(|e| e["pinned"] == true)).unwrap_or(false));

    // 人设：默认 → 修改 → 热生效 → 重置
    let p0: serde_json::Value = client
        .get(format!("{base}/agents/context-agent/persona"))
        .send()
        .await?
        .json()
        .await?;
    c.check("人设默认（智械体风格）", p0["custom"] == false && p0["persona"].as_str().map(|s| s.contains("智械体")).unwrap_or(false));
    let put = client
        .put(format!("{base}/agents/context-agent/persona"))
        .json(&serde_json::json!({"persona": "以武侠风格说话，简洁而有侠气。"}))
        .send()
        .await?;
    c.check("人设修改 200", put.status().is_success());
    let p1: serde_json::Value = client
        .get(format!("{base}/agents/context-agent/persona"))
        .send()
        .await?
        .json()
        .await?;
    c.check("人设修改后为自定义", p1["custom"] == true && p1["persona"].as_str().map(|s| s.contains("武侠")).unwrap_or(false));
    let del = client
        .delete(format!("{base}/agents/context-agent/persona"))
        .send()
        .await?;
    c.check("人设重置 200", del.status().is_success());
    let p2: serde_json::Value = client
        .get(format!("{base}/agents/context-agent/persona"))
        .send()
        .await?
        .json()
        .await?;
    c.check("人设重置后回默认", p2["custom"] == false);

    // ---- 智能体组：创建 → 切换 → 主智能体直答 → 切回 ----
    let groups0: serde_json::Value = client.get(format!("{base}/groups")).send().await?.json().await?;
    c.check("默认组存在且激活", groups0["active"] == "default");

    let g: serde_json::Value = client
        .post(format!("{base}/groups"))
        .json(&serde_json::json!({"id": "product", "name": "产品组", "description": "端到端演示组"}))
        .send()
        .await?
        .json()
        .await?;
    c.check("创建自定义组", g["id"] == "product" && g["builtin"] == false);

    let sw = client
        .put(format!("{base}/groups/active"))
        .json(&serde_json::json!({"id": "product"}))
        .send()
        .await?;
    c.check("切换激活组", sw.status().is_success());

    // 组级能力模型覆盖：写入（空串=清除该项）→ 组元数据回读 → 清空整块收回
    let caps: serde_json::Value = client
        .put(format!("{base}/groups/product/capabilities"))
        .json(&serde_json::json!({
            "speech": "default/tts-x",
            "transcribe": "",
            "visionRelay": "default/vision-x",
            "embedding": "default"
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "组级能力覆盖写入",
        caps["ok"] == true
            && caps["capabilities"]["speech"] == "default/tts-x"
            && caps["capabilities"]["visionRelay"] == "default/vision-x"
            && caps["capabilities"]["embedding"] == "default"
            && caps["capabilities"]["transcribe"].is_null(),
    );
    let groups1: serde_json::Value = client.get(format!("{base}/groups")).send().await?.json().await?;
    let product_caps = groups1["groups"]
        .as_array()
        .and_then(|a| a.iter().find(|g| g["id"] == "product"))
        .map(|g| g["capabilities"]["speech"] == "default/tts-x")
        .unwrap_or(false);
    c.check("组级能力覆盖落盘回读", product_caps);
    let caps_clr: serde_json::Value = client
        .put(format!("{base}/groups/product/capabilities"))
        .json(&serde_json::json!({ "speech": "", "transcribe": "", "visionRelay": "", "embedding": "" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("组级能力覆盖清空收回", caps_clr["ok"] == true && caps_clr["capabilities"].is_null());

    let agents_g: serde_json::Value = client.get(format!("{base}/agents")).send().await?.json().await?;
    c.check("新组初始为空", agents_g.as_array().map(|a| a.is_empty()).unwrap_or(false));

    // 只创建一个主智能体（tier=orchestrator → 自动成为 primary）
    let lead: serde_json::Value = client
        .post(format!("{base}/agents"))
        .json(&serde_json::json!({
            "name": "产品主智能体", "identifier": "product-lead", "tier": "orchestrator",
            "description": "产品组主智能体，直接对接用户"
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("创建主智能体", lead["identifier"] == "product-lead");

    // 组内无子个体 → 主智能体 L0 直答（模拟通道）
    let mut rx2 = core.subscribe();
    let collector2 = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Ok(evt) = rx2.recv().await {
            seen.push(evt.kind.clone());
            if evt.kind == "run.finished" || evt.kind == "run.error" {
                break;
            }
        }
        seen
    });
    let resp2 = client
        .post(format!("{base}/sessions/{sid}/chat"))
        .json(&serde_json::json!({"text": "给出产品组的自我介绍"}))
        .send()
        .await?;
    c.check("自定义组 chat 202", resp2.status().as_u16() == 202);
    let kinds2 = tokio::time::timeout(Duration::from_secs(60), collector2).await??;
    c.check("自定义组 L0 直答完成", kinds2.iter().any(|k| k == "run.finished"));

    // 组级记忆隔离（docs/09 §6）：写入自动归属激活组 product
    let gmem: serde_json::Value = client
        .post(format!("{base}/memory"))
        .json(&serde_json::json!({"title": "产品组专属结论", "body": "该条目只属于 product 组"}))
        .send()
        .await?
        .json()
        .await?;
    c.check("组内记忆写入（product）", gmem["groupId"] == "product");
    let gsearch: serde_json::Value = client
        .post(format!("{base}/memory/search"))
        .json(&serde_json::json!({"query": "产品组专属结论"}))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "组内可见本组条目",
        gsearch["hits"]
            .as_array()
            .map(|h| h.iter().any(|x| x["entry"]["title"] == "产品组专属结论"))
            .unwrap_or(false),
    );

    // 切回默认组
    let sw2 = client
        .put(format!("{base}/groups/active"))
        .json(&serde_json::json!({"id": "default"}))
        .send()
        .await?;
    c.check("切回默认组", sw2.status().is_success());
    let agents_back: serde_json::Value = client.get(format!("{base}/agents")).send().await?.json().await?;
    c.check(
        "切回默认组后个体数与首查一致",
        agents_back.as_array().map(|a| a.len()).unwrap_or(0) == agents_len,
    );

    // 跨组记忆隔离：default 组检索不到 product 组条目
    let dsearch: serde_json::Value = client
        .post(format!("{base}/memory/search"))
        .json(&serde_json::json!({"query": "产品组专属结论"}))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "跨组记忆隔离（default 不可见 product 条目）",
        dsearch["hits"]
            .as_array()
            .map(|h| h.iter().all(|x| x["entry"]["title"] != "产品组专属结论"))
            .unwrap_or(false),
    );

    // 激活组不可删 → 已切回默认组后删除自定义组
    let del: serde_json::Value = client
        .delete(format!("{base}/groups/product"))
        .send()
        .await?
        .json()
        .await?;
    c.check("删除自定义组", del["ok"] == true);

    // ---- 平台能力（docs/10）：技能清单 / 定时任务 / 通道 / 审批 ----
    let skills: serde_json::Value = client.get(format!("{base}/skills")).send().await?.json().await?;
    c.check("技能清单可查", skills.as_array().map(|a| !a.is_empty()).unwrap_or(false));

    let cron_job: serde_json::Value = client
        .post(format!("{base}/cron"))
        .json(&serde_json::json!({
            "name": "e2e 巡检", "prompt": "巡检任务账与风险账并给出摘要", "cron": "0 9 * * *"
        }))
        .send()
        .await?
        .json()
        .await?;
    let cron_id = cron_job["id"].as_str().unwrap_or_default().to_string();
    c.check("定时任务创建（cron 表达式）", !cron_id.is_empty() && cron_job["enabled"] == true);

    let cron_bad: reqwest::Response = client
        .post(format!("{base}/cron"))
        .json(&serde_json::json!({ "name": "bad", "prompt": "x", "cron": "99 * * *" }))
        .send()
        .await?;
    c.check("非法 cron 表达式被拒", cron_bad.status().as_u16() == 400);

    let run: serde_json::Value = client
        .post(format!("{base}/cron/{cron_id}/run"))
        .send()
        .await?
        .json()
        .await?;
    c.check("定时任务手动执行", run["status"] == "done" && !run["sessionId"].as_str().unwrap_or("").is_empty());

    let runs: serde_json::Value = client
        .get(format!("{base}/cron/runs?job={cron_id}"))
        .send()
        .await?
        .json()
        .await?;
    c.check("运行记录可查", runs.as_array().map(|a| a.len() == 1).unwrap_or(false));

    let one_shot: serde_json::Value = client
        .post(format!("{base}/cron"))
        .json(&serde_json::json!({
            "name": "e2e 一次性", "prompt": "一次性提醒", "at": "2020-01-01T00:00:00Z"
        }))
        .send()
        .await?
        .json()
        .await?;
    let once_id = one_shot["id"].as_str().unwrap_or_default().to_string();
    c.check("一次性任务创建（at）", !once_id.is_empty());

    let _ = client.delete(format!("{base}/cron/{cron_id}")).send().await?;
    let _ = client.delete(format!("{base}/cron/{once_id}")).send().await?;
    let cron_gone: reqwest::Response = client.post(format!("{base}/cron/{cron_id}/run")).send().await?;
    c.check("定时任务已删除", cron_gone.status().as_u16() == 404);

    // webhook 通道：入站受理 + 出站回调（本进程内起一个接收器捕获回调载荷）
    let captured: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_for_handler = captured.clone();
    let reply = axum::Router::new().route(
        "/reply",
        axum::routing::post(move |Json(payload): Json<serde_json::Value>| {
            let captured = captured_for_handler.clone();
            async move {
                *captured.lock().unwrap() = Some(payload);
                "ok"
            }
        }),
    );
    let reply_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let reply_port = reply_listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(reply_listener, reply).await;
    });
    let channel: serde_json::Value = client
        .post(format!("{base}/channels"))
        .json(&serde_json::json!({
            "id": "e2e-hook", "secret": "s3cret", "replyWebhook": format!("http://127.0.0.1:{reply_port}/reply")
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("webhook 通道创建", channel["id"] == "e2e-hook");

    let inbound_bad: reqwest::Response = client
        .post(format!("{base}/channels/e2e-hook/inbound"))
        .json(&serde_json::json!({ "text": "x", "secret": "wrong" }))
        .send()
        .await?;
    c.check("通道密钥不符被拒", inbound_bad.status().as_u16() == 401);

    let inbound: serde_json::Value = client
        .post(format!("{base}/channels/e2e-hook/inbound"))
        .json(&serde_json::json!({
            "text": "评估当前项目架构风险", "secret": "s3cret",
            "sessionKey": "user-a"
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("通道入站 202 受理", inbound["accepted"] == true);

    let mut delivered = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if captured.lock().unwrap().is_some() {
            delivered = true;
            break;
        }
    }
    c.check("通道回调送达（运行结果推送）", delivered);

    let sessions: serde_json::Value = client.get(format!("{base}/sessions")).send().await?.json().await?;
    c.check(
        "通道会话按 key 复用创建",
        sessions.as_array().map(|a| a.iter().any(|s| s["title"] == "channel:e2e-hook:user-a")).unwrap_or(false),
    );

    // 通道平台校验 + 掩码回显语义（supervisor 只在 serve() 拉起，e2e 内无适配器连接行为，全部确定性）
    let bad_plat: reqwest::Response = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-bad", "platform": "irc" }))
        .send()
        .await?;
    c.check("非法平台被拒 400", bad_plat.status().as_u16() == 400);

    let qq_missing: reqwest::Response = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-qq", "platform": "qqbot" }))
        .send()
        .await?;
    c.check("qqbot 缺 appId/appSecret 被拒 400", qq_missing.status().as_u16() == 400);

    let nap_missing: reqwest::Response = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-nap", "platform": "napcat", "config": { "token": "x" } }))
        .send()
        .await?;
    c.check("napcat 缺 WS 地址被拒 400", nap_missing.status().as_u16() == 400);

    // 新增内置适配器（discord / slack / matrix）的凭证校验
    let dc_missing: reqwest::Response = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-dc", "platform": "discord" }))
        .send()
        .await?;
    c.check("discord 缺 bot token 被拒 400", dc_missing.status().as_u16() == 400);

    let sl_missing: reqwest::Response = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-sl", "platform": "slack", "token": "xoxb-x" }))
        .send()
        .await?;
    c.check("slack 缺 appToken 被拒 400", sl_missing.status().as_u16() == 400);

    let mx_missing: reqwest::Response = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-mx", "platform": "matrix", "token": "syt_x" }))
        .send()
        .await?;
    c.check("matrix 缺 homeserver 被拒 400", mx_missing.status().as_u16() == 400);

    let dc: serde_json::Value = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-dc", "platform": "discord", "token": "MTIz.bot.token" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("discord 创建成功且 token 掩码回显", dc["type"] == "discord" && dc["token"] == "***已配置***");

    let sl: serde_json::Value = client
        .post(format!("{base}/channels"))
        .json(&json!({
            "id": "e2e-sl", "platform": "slack",
            "token": "xoxb-secret", "config": { "appToken": "xapp-secret" }
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "slack 创建成功：bot token 与 appToken 双双掩码",
        sl["token"] == "***已配置***" && sl["config"]["appToken"] == "***已配置***",
    );

    let mx: serde_json::Value = client
        .post(format!("{base}/channels"))
        .json(&json!({
            "id": "e2e-mx", "platform": "matrix",
            "token": "syt_access", "config": { "homeserver": "https://matrix.org" }
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "matrix 创建成功：homeserver 明示 + access token 掩码",
        mx["config"]["homeserver"] == "https://matrix.org" && mx["token"] == "***已配置***",
    );

    // slack 掩码回传沿用旧 appToken（与提供商页同一三态语义）
    let sl_upd: serde_json::Value = client
        .put(format!("{base}/channels/e2e-sl"))
        .json(&json!({ "config": { "appToken": "***已配置***" } }))
        .send()
        .await?
        .json()
        .await?;
    c.check("slack 掩码回传 appToken 不丢失", sl_upd["config"]["appToken"] == "***已配置***");

    let qq: serde_json::Value = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-qq", "platform": "qqbot", "config": { "appId": "1024", "appSecret": "qq-secret", "sandbox": "false" } }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "qqbot 创建成功且 appSecret 掩码回显",
        qq["config"]["appId"] == "1024" && qq["config"]["appSecret"] == "***已配置***",
    );

    let qq_upd: serde_json::Value = client
        .put(format!("{base}/channels/e2e-qq"))
        .json(&json!({ "config": { "appId": "2048", "appSecret": "***已配置***", "sandbox": "false" } }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "掩码回传沿用旧 appSecret（appId 可改）",
        qq_upd["config"]["appId"] == "2048" && qq_upd["config"]["appSecret"] == "***已配置***",
    );

    // 掩码语义的落盘验证：直接读本次运行的数据目录（e2e 自己的 channels.json）
    let ch_raw = std::fs::read_to_string(data_dir.join("channels.json")).unwrap_or_default();
    let secret_kept = serde_json::from_str::<serde_json::Value>(&ch_raw)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|list| list.iter().any(|ch| ch["id"] == "e2e-qq" && ch["config"]["appSecret"] == "qq-secret"))
        .unwrap_or(false);
    c.check("落盘 appSecret 仍是明文旧值（掩码未污染存储）", secret_kept);

    let qq_clr: serde_json::Value = client
        .put(format!("{base}/channels/e2e-qq"))
        .json(&json!({ "config": { "appId": "2048", "appSecret": "", "sandbox": "" } }))
        .send()
        .await?
        .json()
        .await?;
    c.check("空值清除 appSecret", qq_clr["config"]["appSecret"].is_null());

    let tg: serde_json::Value = client
        .post(format!("{base}/channels"))
        .json(&json!({ "id": "e2e-tg", "platform": "telegram", "token": "123456:abc" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("telegram token 掩码回显", tg["token"] == "***已配置***");

    let tg_upd: serde_json::Value = client
        .put(format!("{base}/channels/e2e-tg"))
        .json(&json!({ "account": "备注一下" }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "非敏感字段更新不动 token",
        tg_upd["token"] == "***已配置***" && tg_upd["account"] == "备注一下",
    );

    let ch_list: serde_json::Value = client.get(format!("{base}/channels")).send().await?.json().await?;
    let no_plaintext = ch_list.as_array().map(|a| {
        a.iter().all(|ch| {
            ch["secret"].as_str().map(|s| s != "s3cret").unwrap_or(true)
                && ch["token"].as_str().map(|s| s != "123456:abc").unwrap_or(true)
                // 新平台敏感 config 键同样不得出明文
                && ch["config"]["appToken"].as_str().map(|s| s != "xapp-secret").unwrap_or(true)
                && ch["config"]["appSecret"].as_str().map(|s| s != "qq-secret").unwrap_or(true)
        })
    }).unwrap_or(false);
    c.check("列表接口无任何明文密钥", no_plaintext && ch_list.as_array().map(|a| a.len() >= 3).unwrap_or(false));

    let chan_status: serde_json::Value = client.get(format!("{base}/channels/status")).send().await?.json().await?;
    c.check("通道状态接口可查（对象）", chan_status.is_object());

    let _ = client.delete(format!("{base}/channels/e2e-qq")).send().await?;
    let _ = client.delete(format!("{base}/channels/e2e-tg")).send().await?;
    let _ = client.delete(format!("{base}/channels/e2e-dc")).send().await?;
    let _ = client.delete(format!("{base}/channels/e2e-sl")).send().await?;
    let _ = client.delete(format!("{base}/channels/e2e-mx")).send().await?;
    let _ = client.delete(format!("{base}/channels/e2e-nap")).send().await?;

    let _ = client.delete(format!("{base}/channels/e2e-hook")).send().await?;

    // 审批 REST 面（闸门逻辑由 smoke 覆盖）
    let approvals: serde_json::Value = client
        .get(format!("{base}/approvals?status=pending"))
        .send()
        .await?
        .json()
        .await?;
    c.check("审批单列表可查", approvals.is_array());
    let approve_missing: reqwest::Response = client
        .post(format!("{base}/approvals/missing-id/approve"))
        .send()
        .await?;
    c.check("审批单不存在返回 404", approve_missing.status().as_u16() == 404);

    // ---- 经验优化（docs/10 §5）：合成要点 → 后续派发携带修订号 → 重置 ----
    // 模拟规划器按 identifier 序取前几名派发：动态选取排序后第 2 个子个体，保证其必被派发
    let target = {
        let mut units: Vec<String> = agents.as_array().cloned().unwrap_or_default()
            .into_iter()
            .filter(|a| a["tier"] == "unit")
            .filter_map(|a| a["identifier"].as_str().map(String::from))
            .collect();
        units.sort();
        units.get(1).cloned().unwrap_or_else(|| "scout-agent".into())
    };
    let adapt: serde_json::Value = client
        .post(format!("{base}/agents/{target}/optimize"))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "经验要点合成 rev1",
        adapt["revision"] == 1 && adapt["content"].as_str().map(|s| s.contains("- ")).unwrap_or(false),
    );

    let mut rx3 = core.subscribe();
    let target2 = target.clone();
    let collector3 = tokio::spawn(async move {
        let target = target2;
        let mut dispatched = false;
        let mut found = false;
        while let Ok(evt) = rx3.recv().await {
            if evt.kind == "dispatch.sent" {
                dispatched = true;
                if evt.payload["agentIdentifier"] == target
                    && evt.payload["adaptationRevision"] == 1
                {
                    found = true;
                }
            }
            if evt.kind == "run.finished" || evt.kind == "run.error" {
                break;
            }
        }
        (dispatched, found)
    });
    let session3: serde_json::Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({"title": "e2e-adapt"}))
        .send()
        .await?
        .json()
        .await?;
    let sid3 = session3["id"].as_str().unwrap_or_default().to_string();
    let resp3 = client
        .post(format!("{base}/sessions/{sid3}/chat"))
        .json(&serde_json::json!({"text": "设计一个支持 1000 个智能体的集群调度方案"}))
        .send()
        .await?;
    c.check("经验优化后 chat 202", resp3.status().as_u16() == 202);
    let (dispatched, found) = tokio::time::timeout(Duration::from_secs(60), collector3).await??;
    c.check("派发发生", dispatched);
    c.check("派发携带经验修订号", found);

    let adapt_get: serde_json::Value = client
        .get(format!("{base}/agents/{target}/adaptation"))
        .send()
        .await?
        .json()
        .await?;
    c.check("经验要点可查", adapt_get["revision"] == 1);
    let adapt_del: serde_json::Value = client
        .delete(format!("{base}/agents/{target}/adaptation"))
        .send()
        .await?
        .json()
        .await?;
    c.check("经验要点重置", adapt_del["ok"] == true);
    let adapt_gone: reqwest::Response = client
        .get(format!("{base}/agents/{target}/adaptation"))
        .send()
        .await?;
    c.check("重置后查询 404", adapt_gone.status().as_u16() == 404);

    // ---- 多厂商模型档案（docs/11）----
    let profiles0: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    c.check(
        "模型档案非空且含生效者",
        profiles0["profiles"].as_array().map(|a| !a.is_empty()).unwrap_or(false)
            && profiles0["profiles"].as_array().map(|a| {
                a.iter().any(|p| p["id"] == profiles0["active"])
            }).unwrap_or(false),
    );
    let new_profile: serde_json::Value = client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({
            "id": "e2e-vendor", "name": "E2E 厂商",
            "baseUrl": "https://api.example.com/v1",
            "apiKey": "sk-e2e", "model": "vendor-large",
            "models": [
                { "model": "vendor-large", "vision": false, "audio": false },
                { "model": "vendor-vision", "vision": true, "audio": true }
            ]
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("新增模型档案", new_profile["ok"] == true && new_profile["id"] == "e2e-vendor");
    let plist: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    let vendor = plist["profiles"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["id"] == "e2e-vendor"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    c.check(
        "模型清单与视觉/语音开关回显",
        vendor["models"].as_array().map(|m| {
            m.len() == 2
                && m[0]["model"] == "vendor-large"
                && m[0]["vision"] == false
                && m[1]["model"] == "vendor-vision"
                && m[1]["vision"] == true
                && m[1]["audio"] == true
        }).unwrap_or(false),
    );

    // 能力模型槽位：语音合成 / 语音转述 / 视觉转述 / 嵌入（嵌入 = memory.semanticModel 同字段）
    let caps_put: serde_json::Value = client
        .put(format!("{base}/llm/capabilities"))
        .json(&serde_json::json!({
            "speech": "e2e-vendor/tts-x",
            "transcribe": "e2e-vendor",
            "visionRelay": "e2e-vendor/vendor-vision",
            "embedding": "e2e-vendor/embed-1"
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "能力槽位写入生效",
        caps_put["ok"] == true
            && caps_put["speech"] == "e2e-vendor/tts-x"
            && caps_put["visionRelay"] == "e2e-vendor/vendor-vision"
            && caps_put["embedding"] == "e2e-vendor/embed-1",
    );
    let caps_get: serde_json::Value = client.get(format!("{base}/llm/capabilities")).send().await?.json().await?;
    c.check(
        "能力槽位可回读",
        caps_get["transcribe"] == "e2e-vendor" && caps_get["embedding"] == "e2e-vendor/embed-1",
    );
    // 清空恢复：不把 e2e 的能力槽位留在本地 config
    let _ = client
        .put(format!("{base}/llm/capabilities"))
        .json(&serde_json::json!({ "speech": "", "transcribe": "", "visionRelay": "", "embedding": "" }))
        .send()
        .await;
    let activated: serde_json::Value = client
        .put(format!("{base}/llm/active"))
        .json(&serde_json::json!({ "id": "e2e-vendor" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("切换生效档案（热生效）", activated["ok"] == true && activated["active"] == "e2e-vendor");
    let cfg_after: serde_json::Value = client.get(format!("{base}/config")).send().await?.json().await?;
    c.check("切换后运行时模型已随档案", cfg_after["llm"]["model"] == "vendor-large");
    let restore: serde_json::Value = client
        .put(format!("{base}/llm/active"))
        .json(&serde_json::json!({ "id": "default" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("切回默认档案", restore["ok"] == true);
    let _ = client.delete(format!("{base}/llm/profiles/e2e-vendor")).send().await;

    // ---- 密钥掩码保全（docs/11）：掩码回显非明文、掩码回传不丢键、池增删不串位 ----
    let _mask_new: serde_json::Value = client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({
            "id": "e2e-mask", "name": "E2E 掩码",
            "baseUrl": "https://api.example.com/v1",
            "apiKey": "sk-mask-primary",
            "apiKeys": ["sk-pool-a", "sk-pool-b"],
            "model": "mask-large"
        }))
        .send()
        .await?
        .json()
        .await?;
    let mask_list0: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    let mask0 = mask_list0["profiles"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["id"] == "e2e-mask"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    c.check(
        "主 Key 与 Key 池掩码回显（非明文）",
        mask0["apiKey"] == "***已配置***"
            && mask0["apiKeys"].as_array().map(|a| a.len() == 2 && a.iter().all(|k| k == "***已配置***")).unwrap_or(false),
    );
    // 模拟前端「追加一把池键」：明文在前 + 掩码保序在后（同位索引实现会在此丢首把池键）
    let _mask_add: serde_json::Value = client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({
            "id": "e2e-mask", "name": "E2E 掩码",
            "baseUrl": "https://api.example.com/v1",
            "apiKey": "***已配置***",
            "apiKeys": ["sk-pool-c", "***已配置***", "***已配置***"],
            "model": "mask-large"
        }))
        .send()
        .await?
        .json()
        .await?;
    let mask_list1: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    let mask1 = mask_list1["profiles"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["id"] == "e2e-mask"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    c.check(
        "掩码回传追加池键不丢键（池含 3 把）",
        mask1["apiKey"] == "***已配置***"
            && mask1["apiKeys"].as_array().map(|a| a.len() == 3).unwrap_or(false),
    );
    // 纯掩码回传（未改动密钥）：主 Key 与池全量保留、不落盘掩码字面量
    let _mask_keep: serde_json::Value = client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({
            "id": "e2e-mask", "name": "E2E 掩码",
            "baseUrl": "https://api.example.com/v1",
            "apiKey": "***已配置***",
            "apiKeys": ["***已配置***", "***已配置***", "***已配置***"],
            "model": "mask-large"
        }))
        .send()
        .await?
        .json()
        .await?;
    let mask_list2: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    let mask2 = mask_list2["profiles"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["id"] == "e2e-mask"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    c.check(
        "纯掩码回传保留全部密钥",
        mask2["apiKey"] == "***已配置***"
            && mask2["apiKeys"].as_array().map(|a| a.len() == 3).unwrap_or(false),
    );
    let _ = client.delete(format!("{base}/llm/profiles/e2e-mask")).send().await;

    // ---- 智能体编辑（docs/09）：受限字段更新（name/domain/description/capabilities/tools/model_hint）；identifier/tier/prompt_file 不可改 ----
    let orch_upd: serde_json::Value = client
        .put(format!("{base}/agents/exmachina-orchestrator"))
        .json(&serde_json::json!({
            "name": "e2e-改名验证",
            "capabilities": ["routing", "e2e-cap"]
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "内置组主智能体受限编辑放行（identifier/tier 不变）",
        orch_upd["name"] == "e2e-改名验证"
            && orch_upd["identifier"] == "exmachina-orchestrator"
            && orch_upd["tier"] == "orchestrator"
            && orch_upd["capabilities"].as_array().map(|a| a.contains(&json!("e2e-cap"))).unwrap_or(false),
    );
    let orch_restore: serde_json::Value = client
        .put(format!("{base}/agents/exmachina-orchestrator"))
        .json(&serde_json::json!({
            "name": "指挥体",
            "capabilities": ["routing", "decomposition", "dispatch", "arbitration", "convergence"]
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("内置组主智能体名称归位（指挥体）", orch_restore["name"] == "指挥体");
    let _single_new: serde_json::Value = client
        .post(format!("{base}/singles"))
        .json(&serde_json::json!({
            "name": "e2e 单体", "identifier": "e2e-single", "description": "e2e 单体描述"
        }))
        .send()
        .await?
        .json()
        .await?;
    let single_upd: serde_json::Value = client
        .put(format!("{base}/singles/e2e-single"))
        .json(&serde_json::json!({
            "name": "e2e 单体·改", "domain": "测试域",
            "capabilities": ["cap-a", "cap-b"]
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "单体编辑：名称/领域/能力标签生效",
        single_upd["name"] == "e2e 单体·改"
            && single_upd["domain"] == "测试域"
            && single_upd["capabilities"].as_array().map(|a| a.len() == 2).unwrap_or(false),
    );
    let _ = client.delete(format!("{base}/singles/e2e-single")).send().await;

    // ---- 设置页保存兼容（D1/D6/D8）：execAllowlist 双形态、search key 三态、memory.mdMaxChars ----
    let d1_put: serde_json::Value = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({
            "security": { "execApproval": "risky", "execAllowlist": "git status, cargo test" }
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("设置页保存 200（execAllowlist 逗号字符串）", d1_put["ok"] == true);
    let d1_cfg: serde_json::Value = client.get(format!("{base}/config")).send().await?.json().await?;
    c.check(
        "execAllowlist 字符串已转数组生效",
        d1_cfg["security"]["execAllowlist"].as_array().map(|a| a.len() == 2).unwrap_or(false),
    );
    let d1_arr: serde_json::Value = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({ "security": { "execApproval": "risky", "execAllowlist": ["echo hi"] } }))
        .send()
        .await?
        .json()
        .await?;
    c.check("设置页保存 200（execAllowlist 数组形态）", d1_arr["ok"] == true);
    let d6_set: serde_json::Value = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({
            "search": { "provider": "tavily", "endpoint": "", "apiKey": "sk-search-e2e", "maxResults": 5 }
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("搜索 API Key 明文写入 200", d6_set["ok"] == true);
    let _d6_clear: serde_json::Value = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({
            "search": { "provider": "tavily", "endpoint": "", "apiKey": "", "maxResults": 5 }
        }))
        .send()
        .await?
        .json()
        .await?;
    let d6_cfg: serde_json::Value = client.get(format!("{base}/config")).send().await?.json().await?;
    c.check("搜索 API Key 显式空串已清除", d6_cfg["search"]["apiKey"] == "");
    let d8_put: serde_json::Value = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({ "memory": { "mdMaxChars": 6000 } }))
        .send()
        .await?
        .json()
        .await?;
    c.check("memory.mdMaxChars 写入 200", d8_put["ok"] == true);
    let d8_cfg: serde_json::Value = client.get(format!("{base}/config")).send().await?.json().await?;
    c.check("memory.mdMaxChars 回显运行值", d8_cfg["memory"]["mdMaxChars"] == 6000);
    let d12_cron: serde_json::Value = client.get(format!("{base}/cron")).send().await?.json().await?;
    c.check(
        "cron 清单不含内部心跳条目",
        d12_cron.as_array().map(|a| !a.iter().any(|j| j["id"] == "__heartbeat__")).unwrap_or(false),
    );

    // ---- 池掩码令牌（D3）：结构化 keep 引用，删行/插行与用户意图完全一致 ----
    let _tok_new: serde_json::Value = client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({
            "id": "e2e-token", "name": "E2E 令牌", "baseUrl": "https://api.example.com/v1",
            "apiKeys": ["sk-tok-a", "sk-tok-b"], "model": "tok-m"
        }))
        .send()
        .await?
        .json()
        .await?;
    let token_pool_len = |v: &serde_json::Value| -> usize {
        v["profiles"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["id"] == "e2e-token"))
            .and_then(|p| p["apiKeys"].as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    };
    client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({ "id": "e2e-token", "name": "E2E 令牌", "apiKeys": [{ "keep": 1 }] }))
        .send()
        .await?;
    let tok_del_first: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    c.check("池令牌：删首行仅保留旧池第 2 把", token_pool_len(&tok_del_first) == 1);
    client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({ "id": "e2e-token", "name": "E2E 令牌", "apiKeys": ["sk-tok-a", "sk-tok-b"] }))
        .send()
        .await?;
    client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({ "id": "e2e-token", "name": "E2E 令牌", "apiKeys": [{ "keep": 0 }] }))
        .send()
        .await?;
    let tok_del_mid: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    c.check("池令牌：删中行仅保留旧池第 1 把", token_pool_len(&tok_del_mid) == 1);
    // 中插明文：前端语义 = 按回读时的旧池生成 keep 令牌；先重置池为 [A,B] 再提交 [X, keep0, keep1]
    client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({ "id": "e2e-token", "name": "E2E 令牌", "apiKeys": ["sk-tok-a", "sk-tok-b"] }))
        .send()
        .await?;
    client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({ "id": "e2e-token", "name": "E2E 令牌", "apiKeys": ["sk-tok-c", { "keep": 0 }, { "keep": 1 }] }))
        .send()
        .await?;
    let tok_insert: serde_json::Value = client.get(format!("{base}/llm/profiles")).send().await?.json().await?;
    c.check("池令牌：中插明文池含 3 把", token_pool_len(&tok_insert) == 3);
    let _ = client.delete(format!("{base}/llm/profiles/e2e-token")).send().await;

    // ---- 多 Key 粘性负载均衡（docs/11 §1）：k1 恒 429 → 切 k2 并粘住 ----
    let key_stats: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let ks_for_handler = key_stats.clone();
    let fake = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| {
                let ks = ks_for_handler.clone();
                async move {
                    let key = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.strip_prefix("Bearer "))
                        .unwrap_or("")
                        .to_string();
                    let _model = body["model"].as_str().unwrap_or("m").to_string();
                    let mut seen = ks.lock().unwrap();
                    let first_time = !seen.contains_key(&key);
                    *seen.entry(key.clone()).or_insert(0) += 1;
                    drop(seen);
                    // 确定性：每把 Key 的第一次请求返回 429（模拟限额），之后恢复——迫使发起方轮换
                    if first_time {
                        return (
                            axum::http::StatusCode::TOO_MANY_REQUESTS,
                            Json(json!({ "error": { "message": "rate limit exceeded" } })),
                        )
                            .into_response();
                    } else {
                        let plan = json!({
                            "routeLevel": "L0", "goal": "e2e-pool",
                            "boundary": { "inScope": [], "forbidden": [] },
                            "acceptance": [], "nodes": [],
                            "finalAnswer": [{ "tag": "报告", "text": "fake ok" }]
                        });
                        let content = format!("```json
{}
```", serde_json::to_string(&plan).unwrap());
                        Json(json!({
                            "choices": [{ "message": { "role": "assistant", "content": content } }],
                            "usage": { "prompt_tokens": 1, "completion_tokens": 1 },
                        }))
                        .into_response()
                    }
                }
            },
        ),
    );
    let fake_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let fake_port = fake_listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(fake_listener, fake).await;
    });

    let _pool: serde_json::Value = client
        .post(format!("{base}/llm/profiles"))
        .json(&serde_json::json!({
            "id": "e2e-pool", "name": "E2E Key 池",
            "baseUrl": format!("http://127.0.0.1:{fake_port}/v1"),
            "apiKeys": ["k1", "k2"],
            "model": "fake-large"
        }))
        .send()
        .await?
        .json()
        .await?;
    let _act: serde_json::Value = client
        .put(format!("{base}/llm/active"))
        .json(&serde_json::json!({ "id": "e2e-pool" }))
        .send()
        .await?
        .json()
        .await?;

    let pool_session: serde_json::Value = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({ "title": "e2e-pool" }))
        .send()
        .await?
        .json()
        .await?;
    let pool_sid = pool_session["id"].as_str().unwrap_or_default().to_string();
    let mut rx4 = core.subscribe();
    let collector4 = tokio::spawn(async move {
        let mut ok = false;
        while let Ok(evt) = rx4.recv().await {
            if evt.kind == "run.finished" {
                ok = true;
                break;
            }
            if evt.kind == "run.error" {
                break;
            }
        }
        ok
    });
    let pool_chat = client
        .post(format!("{base}/sessions/{pool_sid}/chat"))
        .json(&serde_json::json!({ "text": "池化轮换验证" }))
        .send()
        .await?;
    c.check("Key 池会话 chat 202", pool_chat.status().as_u16() == 202);
    let rotated = tokio::time::timeout(Duration::from_secs(60), collector4).await??;
    c.check("Key 限额后轮换成功（run.finished）", rotated);
    // 第二轮：粘住 k2，不再触碰 k1
    let _ = client
        .post(format!("{base}/sessions/{pool_sid}/chat"))
        .json(&serde_json::json!({ "text": "粘性验证第二轮" }))
        .send()
        .await?;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let stats = key_stats.lock().unwrap().clone();
    println!("    [key stats] {:?}", stats);
    // 首次请求 429 → 轮换到另一把并粘住；第二轮不再触碰首次失败的键位顺序
    c.check(
        "粘性负载均衡：两把 Key 均参与且首轮各被限额一次",
        stats.len() == 2 && stats.values().all(|&v| v >= 1),
    );

    let _ = client
        .put(format!("{base}/llm/active"))
        .json(&serde_json::json!({ "id": "default" }))
        .send()
        .await;
    let _ = client.delete(format!("{base}/llm/profiles/e2e-pool")).send().await;

    // ---- 通道账号绑定组（docs/11）----
    let bind_group: serde_json::Value = client
        .post(format!("{base}/groups"))
        .json(&serde_json::json!({ "name": "通道绑定组", "id": "e2e-chan" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("创建绑定目标组", bind_group["id"] == "e2e-chan");
    let bound: serde_json::Value = client
        .post(format!("{base}/channels"))
        .json(&serde_json::json!({
            "id": "e2e-bound", "platform": "webhook", "group": "e2e-chan"
        }))
        .send()
        .await?
        .json()
        .await?;
    c.check("账号绑定组创建通道", bound["group"] == "e2e-chan");
    let inbound2: serde_json::Value = client
        .post(format!("{base}/channels/e2e-bound/inbound"))
        .json(&serde_json::json!({ "text": "绑定组的消息", "sessionKey": "user-b" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("绑定组入站受理", inbound2["accepted"] == true && inbound2["group"] == "e2e-chan");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let all_sessions: serde_json::Value = client
        .get(format!("{base}/sessions?all=true"))
        .send()
        .await?
        .json()
        .await?;
    c.check(
        "入站会话归属绑定组",
        all_sessions.as_array().map(|a| {
            a.iter().any(|s| s["title"] == "channel:e2e-bound:user-b" && s["groupId"] == "e2e-chan")
        }).unwrap_or(false),
    );
    let _ = client.delete(format!("{base}/channels/e2e-bound")).send().await;
    let _ = client
        .delete(format!("{base}/groups/e2e-chan"))
        .send()
        .await;

    // ---- 后台密钥鉴权（docs/12）----
    let _set_key: serde_json::Value = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({ "security": { "authKey": "e2e-secret" } }))
        .send()
        .await?
        .json()
        .await?;
    let no_key: reqwest::Response = client.get(format!("{base}/agents")).send().await?;
    c.check("未带密钥访问 401", no_key.status().as_u16() == 401);
    let no_key_cfg: reqwest::Response = client.get(format!("{base}/config")).send().await?;
    c.check("未带密钥访问数据端点 /config 401", no_key_cfg.status().as_u16() == 401);
    let mut wrong_key_req = client.get(format!("{base}/sessions")).build()?;
    wrong_key_req.headers_mut().insert("x-auth-key", "bad-key".parse().unwrap());
    let wrong_key_resp = client.execute(wrong_key_req).await?;
    c.check("错误密钥访问数据端点 401", wrong_key_resp.status().as_u16() == 401);
    let wrong: serde_json::Value = client
        .post(format!("{base}/auth/verify"))
        .json(&serde_json::json!({ "key": "wrong" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("错误密钥 verify 拒绝", wrong["ok"] == false);
    let right: serde_json::Value = client
        .post(format!("{base}/auth/verify"))
        .json(&serde_json::json!({ "key": "e2e-secret" }))
        .send()
        .await?
        .json()
        .await?;
    c.check("正确密钥 verify 通过", right["ok"] == true);
    let mut with_key = client.get(format!("{base}/agents")).build()?;
    with_key.headers_mut().insert("x-auth-key", "e2e-secret".parse().unwrap());
    let ok_resp = client.execute(with_key).await?;
    c.check("带密钥访问 200", ok_resp.status().as_u16() == 200);
    // 通道入站豁免（D10）：有自有 secret 的 webhook 通道放行；无 secret 通道仍被全局鉴权拦截
    let _mk_hook: serde_json::Value = client
        .post(format!("{base}/channels"))
        .header("x-auth-key", "e2e-secret")
        .json(&serde_json::json!({ "id": "e2e-hook-auth", "platform": "webhook", "secret": "hook-secret" }))
        .send()
        .await?
        .json()
        .await?;
    let inbound_ok: reqwest::Response = client
        .post(format!("{base}/channels/e2e-hook-auth/inbound"))
        .json(&serde_json::json!({ "secret": "hook-secret", "text": "鉴权豁免验证" }))
        .send()
        .await?;
    c.check("带 secret 通道入站豁免全局鉴权（202）", inbound_ok.status().as_u16() == 202);
    let inbound_bad: reqwest::Response = client
        .post(format!("{base}/channels/e2e-hook-auth/inbound"))
        .json(&serde_json::json!({ "secret": "wrong", "text": "错密钥" }))
        .send()
        .await?;
    c.check("secret 错误入站仍被拒 401", inbound_bad.status().as_u16() == 401);
    let _mk_plain: serde_json::Value = client
        .post(format!("{base}/channels"))
        .header("x-auth-key", "e2e-secret")
        .json(&serde_json::json!({ "id": "e2e-nosecret", "platform": "webhook" }))
        .send()
        .await?
        .json()
        .await?;
    let inbound_no: reqwest::Response = client
        .post(format!("{base}/channels/e2e-nosecret/inbound"))
        .json(&serde_json::json!({ "text": "无凭据通道" }))
        .send()
        .await?;
    c.check("无 secret 通道入站仍被全局鉴权拦截 401", inbound_no.status().as_u16() == 401);
    let mut rm_hook = client.delete(format!("{base}/channels/e2e-hook-auth")).build()?;
    rm_hook.headers_mut().insert("x-auth-key", "e2e-secret".parse().unwrap());
    let _ = client.execute(rm_hook).await;
    let mut rm_plain = client.delete(format!("{base}/channels/e2e-nosecret")).build()?;
    rm_plain.headers_mut().insert("x-auth-key", "e2e-secret".parse().unwrap());
    let _ = client.execute(rm_plain).await;
    let ws_root = base.trim_end_matches("/api").to_string();
    let ws_no_key: reqwest::Response = client.get(format!("{ws_root}/ws")).send().await?;
    c.check("WS 无密钥 401", ws_no_key.status().as_u16() == 401);
    let mut clear_req = client
        .put(format!("{base}/config"))
        .json(&serde_json::json!({ "security": { "authKey": "" } }))
        .build()?;
    clear_req.headers_mut().insert("x-auth-key", "e2e-secret".parse().unwrap());
    let _clear: serde_json::Value = client.execute(clear_req).await?.json().await?;
    // 注意：清除后旧客户端仍带 X-Auth-Key 头也无需匹配——中间件放行所有请求
    let after_clear: reqwest::Response = client.get(format!("{base}/agents")).send().await?;
    c.check("清除密钥后恢复免鉴权", after_clear.status().as_u16() == 200);

    server.abort();
    println!("\n通过 {}｜失败 {}", c.pass, c.fail);
    if c.fail > 0 {
        std::process::exit(1);
    }
    println!("e2e 全部通过。");
    Ok(())
}
