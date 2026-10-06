//! MCP HTTP 挂载接口测试（组 10.3）：鉴权复用全局中间件 + 会话亲和
//! （openspec capability-completion 任务 10.3）

use axum::body::Body;
use axum::http::{Request, StatusCode};
use exm_core::config::ExmConfig;
use serde_json::{json, Value};
use tower::Service as _;
use tower_http::cors::CorsLayer;

fn unique_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("exm-mcp-http-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn app_with(auth_key: &str) -> (axum::Router, std::path::PathBuf, String) {
    let dir = unique_dir(auth_key.is_empty().then(|| "open").unwrap_or("keyed"));
    let mut cfg = ExmConfig::load(&dir);
    cfg.use_mock = true;
    cfg.memory_enabled = false;
    cfg.mcp_serve.enabled = true;
    cfg.mcp_serve.allowed_tools = vec!["read".into()];
    cfg.security.auth_key = auth_key.into();
    let core = std::sync::Arc::new(exm_core::Core::with_config(cfg).unwrap());
    // 测试组 + 主智能体 + 会话
    core.registry().create_group(Some("t".into()), "mcp组", "").unwrap();
    let def = exm_core::types::AgentDefinition {
        name: "服务体".into(),
        identifier: "mcp-orch".into(),
        domain: "测试".into(),
        tier: exm_core::types::Tier::Orchestrator,
        description: "测试".into(),
        capabilities: vec![],
        tools: vec![],
        when_to_call: String::new(),
        dependencies: vec![],
        composable_with: vec![],
        input_schema: Default::default(),
        output_schema: Default::default(),
        prompt_file: String::new(),
        model_hint: None,
    };
    core.registry().upsert_agent("t", def, None).unwrap();
    core.registry().set_primary("t", "mcp-orch").unwrap();
    core.registry().set_active_group("t").unwrap();
    let s = core.create_session("http 亲和").unwrap();
    // build_router 已按配置挂载 /mcp（位于鉴权中间件之前，与 serve() 同源）
    let router = exm_gateway::build_router(core.clone()).layer(CorsLayer::permissive());
    (router, dir, s.id)
}

async fn post(router: &axum::Router, path: &str, key: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("POST").uri(path);
    if let Some(k) = key {
        builder = builder.header("x-auth-key", k);
    }
    let mut req = builder.body(Body::from(body.to_string())).unwrap();
    // serve() 经 into_make_service_with_connect_info 注入对端地址；直连调用补同款扩展
    req.extensions_mut().insert(axum::extract::ConnectInfo(
        std::net::SocketAddr::from(([127, 0, 0, 1], 50000)),
    ));
    let resp = router.clone().call(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    (status, v)
}

#[tokio::test]
async fn http挂载_未启用404_启用后调用成功且鉴权生效() {
    // 启用 + 无鉴权：协议全流程
    let (mut router, dir, sid) = app_with("").await;
    let (st, _) = post(&router, "/mcp", None, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
    assert_eq!(st, StatusCode::OK);
    // 会话亲和：有效会话执行 read
    std::fs::write(dir.join("n.txt"), "http mcp").unwrap();
    let (st, body) = post(
        &mut router,
        "/mcp",
        None,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "read", "arguments": { "session": sid, "path": "n.txt" } } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["result"]["isError"], false, "{body}");
    assert!(body["result"]["content"][0]["text"].as_str().unwrap().contains("http mcp"));
    // 无效会话拒绝（会话亲和）
    let (st, body) = post(
        &mut router,
        "/mcp",
        None,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "read", "arguments": { "session": "ghost", "path": "n.txt" } } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "协议层错误仍走 200（JSON-RPC error 载荷）");
    assert!(body["error"]["message"].as_str().unwrap().contains("会话不存在"), "{body}");

    // 鉴权：authKey 配置后缺密钥 401
    let (router2, dir2, _) = app_with("secret-key").await;
    let (st, _) = post(&router2, "/mcp", None, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "复用鉴权中间件：缺密钥拒绝");
    let (st, _) = post(&router2, "/mcp", Some("secret-key"), json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
    assert_eq!(st, StatusCode::OK, "带密钥放行");

    // 未启用：挂载点不存在（404，spec：默认关闭）
    let dir3 = unique_dir("off");
    let mut cfg = ExmConfig::load(&dir3);
    cfg.use_mock = true;
    cfg.mcp_serve.enabled = false;
    let core3 = std::sync::Arc::new(exm_core::Core::with_config(cfg).unwrap());
    let router3 = exm_gateway::build_router(core3).layer(CorsLayer::permissive());
    let (st, _) = post(&router3, "/mcp", None, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "默认关闭 = 挂载点不可用");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
    let _ = std::fs::remove_dir_all(&dir3);
}
