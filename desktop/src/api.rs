//! 网关 REST 客户端(ureq 阻塞式;本地调用快,UI 线程直用可接受)

use anyhow::Context;
use serde_json::Value;

pub struct Api {
    pub base: String,
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub mode: String,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub preview: String,
}

#[derive(Debug, Clone)]
pub struct Msg {
    pub role: String,
    pub text: String,
    pub tool_calls: usize,
    pub thinking: Option<String>,
}

impl Api {
    pub fn new(base: String, key: String) -> Self {
        Api { base: base.trim_end_matches('/').to_string(), key }
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let mut r = ureq::request(method, &format!("{}{}", self.base, path));
        if !self.key.is_empty() {
            r = r.set("X-Auth-Key", &self.key);
        }
        r
    }

    fn send_json(&self, method: &str, path: &str, body: Value) -> anyhow::Result<Value> {
        let resp = self
            .req(method, path)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .context(format!("{method} {path} 请求失败"))?;
        let v: Value = resp.into_json().context("响应非 JSON")?;
        Ok(v)
    }

    fn get_json(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self.req("GET", path).call().context(format!("GET {path} 请求失败"))?;
        let v: Value = resp.into_json().context("响应非 JSON")?;
        Ok(v)
    }

    pub fn target(&self) -> anyhow::Result<Target> {
        let v = self.get_json("/api/target")?;
        Ok(Target {
            mode: v["mode"].as_str().unwrap_or("group").into(),
            id: v["id"].as_str().unwrap_or("exmachina").into(),
            name: v["name"].as_str().unwrap_or("智能连结").into(),
        })
    }

    pub fn set_target(&self, mode: &str, id: &str) -> anyhow::Result<()> {
        self.send_json("PUT", "/api/target", serde_json::json!({ "mode": mode, "id": id }))?;
        Ok(())
    }

    pub fn sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let v = self.get_json("/api/sessions")?;
        let list = v.as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .map(|s| SessionInfo {
                id: s["id"].as_str().unwrap_or_default().into(),
                title: s["title"].as_str().unwrap_or("会话").into(),
                preview: s["lastMessagePreview"].as_str().unwrap_or_default().into(),
            })
            .collect())
    }

    pub fn create_session(&self, title: &str) -> anyhow::Result<String> {
        let v = self.send_json("POST", "/api/sessions", serde_json::json!({ "title": title }))?;
        Ok(v["id"]
            .as_str()
            .context("创建会话响应缺 id")?
            .to_string())
    }

    pub fn messages(&self, id: &str) -> anyhow::Result<Vec<Msg>> {
        let v = self.get_json(&format!("/api/sessions/{id}/messages"))?;
        let list = v.as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .map(|m| Msg {
                role: m["role"].as_str().unwrap_or_default().into(),
                text: m["statements"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|s| s["text"].as_str().unwrap_or_default().to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                tool_calls: m["toolCalls"].as_array().map(|a| a.len()).unwrap_or(0),
                thinking: m["thinking"].as_str().map(String::from),
            })
            .collect())
    }

    pub fn chat(&self, id: &str, text: &str) -> anyhow::Result<()> {
        self.send_json("POST", &format!("/api/sessions/{id}/chat"), serde_json::json!({ "text": text }))?;
        Ok(())
    }

    pub fn stop_session(&self, id: &str) -> anyhow::Result<()> {
        self.send_json("POST", &format!("/api/sessions/{id}/stop"), serde_json::json!({}))?;
        Ok(())
    }

    /// 读取 LLM 配置(baseUrl / apiKey / model)
    pub fn llm_config(&self) -> anyhow::Result<(String, String, String)> {
        let v = self.get_json("/api/config")?;
        Ok((
            v["llm"]["baseUrl"].as_str().unwrap_or_default().into(),
            v["llm"]["apiKey"].as_str().unwrap_or_default().into(),
            v["llm"]["model"].as_str().unwrap_or_default().into(),
        ))
    }

    pub fn save_llm(&self, base_url: &str, api_key: &str, model: &str) -> anyhow::Result<()> {
        self.send_json(
            "PUT",
            "/api/config",
            serde_json::json!({
                "llm": { "baseUrl": base_url, "apiKey": api_key, "model": model }
            }),
        )?;
        Ok(())
    }
}
