//! 事件流线程:常驻 WS 订阅,流式增量写入共享状态并驱动重绘。
//! 会话切换 = 更新期望 URL,线程感知后自动重连到新会话。

use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub struct Stream {
    /// 流式中的 assistant 正文
    pub text: String,
    /// 流式中的思维链(折叠呈现)
    pub thinking: String,
    /// 轮次已结束(触发 UI 刷新消息列表)
    pub finished: bool,
    /// 轮次错误
    pub error: Option<String>,
    /// WS 已连接
    pub connected: bool,
}

pub struct StreamHandle {
    shared: Arc<Mutex<Stream>>,
    want: Arc<Mutex<Option<String>>>,
}

impl Default for StreamHandle {
    fn default() -> Self {
        Self { shared: Arc::new(Mutex::new(Stream::default())), want: Arc::new(Mutex::new(None)) }
    }
}

impl StreamHandle {
    /// 更新订阅目标(完整 ws url 含 sessionId);None = 停止订阅
    pub fn set_target(&self, url: Option<String>) {
        let mut w = self.want.lock().unwrap();
        if *w != url {
            *w = url;
        }
        let mut st = self.shared.lock().unwrap();
        st.text.clear();
        st.thinking.clear();
        st.finished = false;
        st.error = None;
    }

    pub fn shared(&self) -> Arc<Mutex<Stream>> {
        self.shared.clone()
    }

    /// 常驻线程(应用启动时拉起一次)
    pub fn spawn(self: &Arc<Self>, ctx: egui::Context, key: String) {
        let h = self.clone();
        std::thread::spawn(move || loop {
            let url = h.want.lock().unwrap().clone();
            let Some(url) = url.clone() else {
                std::thread::sleep(Duration::from_millis(400));
                continue;
            };
            let full = if key.is_empty() {
                url.clone()
            } else if url.contains('?') {
                format!("{url}&key={key}")
            } else {
                format!("{url}?key={key}")
            };
            match tungstenite::client::connect(&full) {
                Ok((mut ws, _)) => {
                    h.shared.lock().unwrap().connected = true;
                    ctx.request_repaint();
                    loop {
                        // 期望变更(切换会话)即断开重连
                        let want_now = h.want.lock().unwrap().clone();
                        if want_now.as_deref() != Some(url.as_str()) {
                            break;
                        }
                        match ws.read() {
                            Ok(tungstenite::Message::Text(t)) => {
                                if handle_event(&ctx, &h.shared, &t) {
                                    // 轮次结束:UI 刷新消息;清 finished 由 UI 完成
                                }
                            }
                            Ok(tungstenite::Message::Ping(p)) => {
                                let _ = ws.send(tungstenite::Message::Pong(p));
                            }
                            Ok(_) => {}
                            Err(e) => {
                                h.shared.lock().unwrap().connected = false;
                                eprintln!("[ws] 断开,3s 重连: {e}");
                                ctx.request_repaint();
                                break;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => {
                    h.shared.lock().unwrap().connected = false;
                    eprintln!("[ws] 连接失败,3s 重试: {e}");
                    ctx.request_repaint();
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        });
    }
}

/// 处理一条事件;返回 true = 轮次结束
fn handle_event(ctx: &egui::Context, shared: &Arc<Mutex<Stream>>, raw: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else { return false };
    let kind = v["kind"].as_str().unwrap_or_default().to_string();
    let mut st = shared.lock().unwrap();
    match kind.as_str() {
        "orchestrator.token" => {
            if let Some(d) = v["payload"]["delta"].as_str() {
                st.text.push_str(d);
                drop(st);
                ctx.request_repaint();
                return false;
            }
        }
        "orchestrator.thinking" => {
            if let Some(d) = v["payload"]["delta"].as_str() {
                st.thinking.push_str(d);
            }
        }
        "run.finished" => {
            st.finished = true;
            drop(st);
            ctx.request_repaint();
            return true;
        }
        "run.error" => {
            st.error = v["payload"]["message"].as_str().map(String::from);
            drop(st);
            ctx.request_repaint();
            return true;
        }
        _ => {}
    }
    ctx.request_repaint();
    false
}
