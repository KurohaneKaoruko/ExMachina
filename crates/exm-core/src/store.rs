//! 持久化 —— 基于纯 Rust 文档存储（FsDb）。
//!
//! 与 docs/04 §6 的对应关系（原 SQLite 表 → 文档集合）：
//!   sessions              → `sessions/{id}.json`
//!   messages              → `messages/{sessionId}.jsonl`（追加日志）
//!   task_graphs+task_nodes → `graphs/{sessionId}.json`（图与节点同文档）+ `graph_history/{sessionId}.jsonl`
//!   sync_reports          → `sync_reports/{reportId}.json` + `sync_reports_by_node/{nodeId}.json`
//!   evidence              → `evidence/{sessionId}.jsonl`
//!   tool_audit            → `tool_audit/{agentId}.jsonl`（超长自动裁剪）
//!   events                → `events/{sessionId}.jsonl`
//!   turn_snapshots        → `turn_snapshots/{sessionId}:{turn}.json`（undo/分支基座）
//!   identities            → `identities/{channel}:{externalId}.json`（通道用户配对）
//!   event_triggers        → `event_triggers/{id}.json`（文件监听 / webhook 事件源）
//!
//! 之所以是文档而非 SQL：构建环境无任何 C 编译器（见 docs/架构与设计.md）。
//! 存储层位于 repository 接口之后，替换回 SQLite/redb 不影响上层。

use crate::fsdb::FsDb;
use crate::types::*;
use anyhow::Result;
use std::path::Path;

pub struct Store {
    db: FsDb,
    /// 三账为读-改-写热点（并发节点同时回流），此锁保证不丢更新
    ledger_lock: std::sync::Mutex<()>,
}

const MAX_AUDIT_LINES: usize = 2000;

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Store { db: FsDb::open(root)?, ledger_lock: std::sync::Mutex::new(()) })
    }

    pub fn root(&self) -> String {
        self.db.root().display().to_string()
    }

    // ---------------- 会话 / 三账 ----------------

    pub fn create_session(&self, title: &str, group_id: &str) -> Result<Session> {
        let now = now_iso();
        let s = Session {
            id: new_id(),
            title: title.to_string(),
            status: "active".into(),
            group_id: if group_id.trim().is_empty() { "default".into() } else { group_id.to_string() },
            rolling_summary: None,
            summary_upto: None,
            ledger: SessionLedger::default(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.db.put("sessions", &s.id, &s)?;
        Ok(s)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.db.get("sessions", id)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut list: Vec<Session> = self.db.list("sessions")?;
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list.truncate(200);
        Ok(list)
    }

    pub fn update_ledger(&self, session_id: &str, ledger: &SessionLedger) -> Result<()> {
        let _guard = self.ledger_lock.lock().unwrap();
        let mut session = match self.get_session(session_id)? {
            Some(s) => s,
            None => anyhow::bail!("会话不存在: {session_id}"),
        };
        session.ledger = ledger.clone();
        session.updated_at = now_iso();
        self.db.put("sessions", session_id, &session)
    }

    /// 原子读-改-写三账（并发回流安全：不会互相覆盖）
    pub fn mutate_ledger<F>(&self, session_id: &str, f: F) -> Result<SessionLedger>
    where
        F: FnOnce(&mut SessionLedger),
    {
        let _guard = self.ledger_lock.lock().unwrap();
        let mut session = match self.get_session(session_id)? {
            Some(s) => s,
            None => anyhow::bail!("会话不存在: {session_id}"),
        };
        f(&mut session.ledger);
        session.updated_at = now_iso();
        let ledger = session.ledger.clone();
        self.db.put("sessions", session_id, &session)?;
        Ok(ledger)
    }

    pub fn ledger_of(&self, session_id: &str) -> Result<SessionLedger> {
        Ok(self.get_session(session_id)?.map(|s| s.ledger).unwrap_or_default())
    }

    /// 更新会话滚动摘要（上下文压缩，docs/架构与设计.md）
    /// 更新会话标题（重命名）
    pub fn update_session_title(&self, session_id: &str, title: &str) -> Result<()> {
        let _guard = self.ledger_lock.lock().unwrap();
        let mut session = match self.get_session(session_id)? {
            Some(s) => s,
            None => anyhow::bail!("会话不存在: {session_id}"),
        };
        session.title = title.trim().to_string();
        session.updated_at = now_iso();
        self.db.put("sessions", session_id, &session)
    }

    /// 更新会话滚动摘要（上下文压缩，docs/架构与设计.md）
    pub fn update_compaction(&self, session_id: &str, summary: &str, upto: usize) -> Result<()> {
        let _guard = self.ledger_lock.lock().unwrap();
        let mut session = match self.get_session(session_id)? {
            Some(s) => s,
            None => anyhow::bail!("会话不存在: {session_id}"),
        };
        session.rolling_summary = if summary.trim().is_empty() { None } else { Some(summary.to_string()) };
        session.summary_upto = Some(upto);
        session.updated_at = now_iso();
        self.db.put("sessions", session_id, &session)
    }

    // ---------------- 消息 ----------------

    pub fn add_message(
        &self,
        session_id: &str,
        role: MessageRole,
        agent_id: Option<&str>,
        statements: Vec<Statement>,
    ) -> Result<ChatMessage> {
        let msg = ChatMessage {
            id: new_id(),
            session_id: session_id.to_string(),
            role,
            agent_id: agent_id.map(|s| s.to_string()),
            statements,
            created_at: now_iso(),
            token_usage: None,
        };
        self.db.append_line("messages", session_id, &msg)?;
        Ok(msg)
    }

    pub fn list_messages(&self, session_id: &str, limit: usize) -> Result<Vec<ChatMessage>> {
        self.db.read_lines("messages", session_id, limit)
    }

    // ---------------- 任务图 ----------------

    pub fn save_graph(&self, graph: &TaskGraph) -> Result<()> {
        self.db.put("graphs", &graph.session_id, graph)?;
        // 历史留痕（幂等：以图 id 去重）
        let history: Vec<TaskGraph> = self.db.read_lines("graph_history", &graph.session_id, 0)?;
        if !history.iter().any(|g| g.id == graph.id) {
            self.db.append_line("graph_history", &graph.session_id, graph)?;
        }
        Ok(())
    }

    pub fn latest_graph(&self, session_id: &str) -> Result<Option<TaskGraph>> {
        self.db.get("graphs", session_id)
    }

    // ---------------- 回流与证据 ----------------

    pub fn add_sync_report(&self, report: &SyncReport) -> Result<String> {
        let id = new_id();
        let mut value = serde_json::to_value(report)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("id".into(), serde_json::json!(id));
            obj.insert("createdAt".into(), serde_json::json!(now_iso()));
        }
        self.db.put("sync_reports", &id, &value)?;
        self.db.put("sync_reports_by_node", &report.task_node_id, &serde_json::json!(id))?;
        Ok(id)
    }

    /// 节点最新回流（断点续跑回填用）
    pub fn sync_report_of_node(&self, node_id: &str) -> Result<Option<SyncReport>> {
        let v = self.report_for_node(node_id)?;
        Ok(v.and_then(|v| serde_json::from_value(v).ok()))
    }

    pub fn report_for_node(&self, node_id: &str) -> Result<Option<serde_json::Value>> {
        let id: Option<serde_json::Value> = self.db.get("sync_reports_by_node", node_id)?;
        match id.and_then(|v| v.as_str().map(|s| s.to_string())) {
            Some(rid) => self.db.get("sync_reports", &rid),
            None => Ok(None),
        }
    }

    pub fn add_evidence(
        &self,
        session_id: &str,
        items: &[EvidenceItem],
        sync_report_id: Option<&str>,
    ) -> Result<()> {
        for e in items {
            let mut value = serde_json::to_value(e)?;
            if let Some(obj) = value.as_object_mut() {
                obj.insert("id".into(), serde_json::json!(new_id()));
                obj.insert("sessionId".into(), serde_json::json!(session_id));
                obj.insert("syncReportId".into(), serde_json::json!(sync_report_id));
                obj.insert("createdAt".into(), serde_json::json!(now_iso()));
            }
            self.db.append_line("evidence", session_id, &value)?;
        }
        Ok(())
    }

    pub fn list_evidence(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        self.db.read_lines("evidence", session_id, 0)
    }

    pub fn audit_tool(
        &self,
        agent_id: &str,
        tool: &str,
        args: &serde_json::Value,
        result_summary: &str,
        duration_ms: u64,
    ) -> Result<()> {
        let entry = serde_json::json!({
            "id": new_id(),
            "agentId": agent_id,
            "tool": tool,
            "args": args,
            "resultSummary": result_summary,
            "durationMs": duration_ms,
            "createdAt": now_iso(),
        });
        self.db.append_line("tool_audit", agent_id, &entry)?;

        // 容量裁剪：超过上限时保留最近一半
        let mut lines: Vec<serde_json::Value> = self.db.read_lines("tool_audit", agent_id, 0)?;
        if lines.len() > MAX_AUDIT_LINES {
            let keep = lines.split_off(lines.len() - MAX_AUDIT_LINES / 2);
            self.db.rewrite_lines("tool_audit", agent_id, &keep)?;
        }
        Ok(())
    }

    pub fn list_tool_audit(&self, agent_id: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
        self.db.read_lines("tool_audit", agent_id, limit)
    }

    // ---------------- 用量台账（真实 token，替代字符粗估） ----------------

    /// 记录一次 LLM 调用的真实用量（provider 返回的 usage；0/0 不记）
    pub fn add_usage(&self, session_id: &str, prompt_tokens: u64, completion_tokens: u64, model: &str) -> Result<()> {
        if prompt_tokens == 0 && completion_tokens == 0 {
            return Ok(());
        }
        let entry = serde_json::json!({
            "id": new_id(),
            "promptTokens": prompt_tokens,
            "completionTokens": completion_tokens,
            "model": model,
            "createdAt": now_iso(),
        });
        self.db.append_line("usage", session_id, &entry)
    }

    /// 会话累计用量：(prompt, completion, 调用次数)
    pub fn usage_total(&self, session_id: &str) -> (u64, u64, usize) {
        let lines: Vec<serde_json::Value> = self.db.read_lines("usage", session_id, 0).unwrap_or_default();
        let mut p = 0u64;
        let mut c = 0u64;
        for l in &lines {
            p += l.get("promptTokens").and_then(|v| v.as_u64()).unwrap_or(0);
            c += l.get("completionTokens").and_then(|v| v.as_u64()).unwrap_or(0);
        }
        (p, c, lines.len())
    }

    /// 最近 N 条用量明细（管理视图）
    pub fn usage_recent(&self, session_id: &str, limit: usize) -> Vec<serde_json::Value> {
        self.db.read_lines("usage", session_id, limit).unwrap_or_default()
    }

    // ---------------- 事件溯源 ----------------

    pub fn append_event(&self, env: &Envelope) -> Result<()> {
        self.db.append_line("events", &env.session_id, env)
    }

    pub fn list_events(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        self.db.read_lines("events", session_id, 0)
    }

    // ---------------- 执行审批（docs/10） ----------------

    pub fn add_approval(&self, req: &ApprovalRequest) -> Result<()> {
        self.db.put("approvals", &req.id, req)
    }

    pub fn get_approval(&self, id: &str) -> Result<Option<ApprovalRequest>> {
        self.db.get("approvals", id)
    }

    pub fn update_approval(&self, req: &ApprovalRequest) -> Result<()> {
        self.db.put("approvals", &req.id, req)
    }

    /// 按组列出会话（组是交互对象：会话归属创建时的激活组）
    pub fn list_sessions_in_group(&self, group_id: &str) -> Result<Vec<Session>> {
        let all: Vec<Session> = self.db.list("sessions")?;
        let mut list: Vec<Session> = all
            .into_iter()
            .filter(|s| s.group_id == group_id)
            .collect();
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list.truncate(200);
        Ok(list)
    }

    /// 删除会话及其派生数据（消息/任务图/事件/证据）
    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.db.delete("sessions", id)?;
        for (col, key) in [
            ("messages", id),
            ("graphs", id),
            ("graph_history", id),
            ("events", id),
            ("evidence", id),
        ] {
            let _ = self.db.delete(col, key);
        }
        Ok(())
    }

    pub fn list_approvals(&self, status: Option<&str>, limit: usize) -> Result<Vec<ApprovalRequest>> {
        let mut list: Vec<ApprovalRequest> = self.db.list("approvals")?;
        if let Some(s) = status {
            list.retain(|r| r.status == s);
        }
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        list.truncate(limit);
        Ok(list)
    }

    // ---------------------------------------------------------------- 轮次快照（undo / 编辑重发 / 分支基座）

    /// 写入轮次快照（每轮次结束原子落一条；id = `{sessionId}:{turn}`）
    pub fn put_turn_snapshot(&self, snap: &TurnSnapshot) -> Result<()> {
        self.db.put("turn_snapshots", &snap.id, snap)
    }

    pub fn list_turn_snapshots(&self, session_id: &str) -> Result<Vec<TurnSnapshot>> {
        let mut list: Vec<TurnSnapshot> = self.db.list("turn_snapshots")?;
        list.retain(|s| s.session_id == session_id);
        list.sort_by(|a, b| a.turn.cmp(&b.turn).then(a.created_at.cmp(&b.created_at)));
        Ok(list)
    }

    pub fn latest_turn_snapshot(&self, session_id: &str) -> Result<Option<TurnSnapshot>> {
        Ok(self.list_turn_snapshots(session_id)?.pop())
    }

    /// 撤销后的孤儿快照清理（保留 turn <= upto 的快照）
    pub fn prune_turn_snapshots_after(&self, session_id: &str, upto_turn: usize) -> Result<()> {
        for snap in self.list_turn_snapshots(session_id)? {
            if snap.turn > upto_turn {
                self.db.delete("turn_snapshots", &snap.id)?;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- 通道用户身份（配对绑定）

    /// 身份主键约定：`{channel}:{externalId}`（同通道账号唯一）
    pub fn put_identity(&self, identity: &UserIdentity) -> Result<()> {
        self.db.put("identities", &identity.id, identity)
    }

    pub fn get_identity(&self, channel: &str, external_id: &str) -> Result<Option<UserIdentity>> {
        self.db.get("identities", &format!("{channel}:{external_id}"))
    }

    pub fn get_identity_by_id(&self, id: &str) -> Result<Option<UserIdentity>> {
        self.db.get("identities", id)
    }

    pub fn list_identities(&self) -> Result<Vec<UserIdentity>> {
        let mut list: Vec<UserIdentity> = self.db.list("identities")?;
        list.sort_by(|a, b| b.paired_at.cmp(&a.paired_at));
        Ok(list)
    }

    pub fn delete_identity(&self, id: &str) -> Result<()> {
        self.db.delete("identities", id)
    }

    // ---------------------------------------------------------------- 会话产物 outbox（媒体投递）

    /// 登记本会话产生的媒体产物（截图自动登记；`.exmachina/outbox/` 写入登记）
    pub fn outbox_push(&self, session_id: &str, kind: &str, path: &str, caption: &str) -> Result<()> {
        self.db.append_line(
            "session_outbox",
            session_id,
            &serde_json::json!({ "kind": kind, "path": path, "caption": caption, "at": now_iso() }),
        )
    }

    /// 排空产物（读出即清空，防重复投递）
    pub fn outbox_drain(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        let items: Vec<serde_json::Value> = self.db.read_lines("session_outbox", session_id, 0)?;
        if !items.is_empty() {
            self.db.rewrite_lines::<serde_json::Value>("session_outbox", session_id, &[])?;
        }
        Ok(items)
    }

    // ---------------------------------------------------------------- 会话来源（通道身份升级语义）

    /// 登记会话的外部来源（通道闸门放行时写入；审批单创建时读取打标）
    pub fn put_session_origin(&self, session_id: &str, external_id: &str, role: &str) -> Result<()> {
        self.db.put(
            "session_origins",
            session_id,
            &serde_json::json!({ "externalId": external_id, "role": role }),
        )
    }

    pub fn get_session_origin(&self, session_id: &str) -> Result<Option<(String, String)>> {
        let v: Option<serde_json::Value> = self.db.get("session_origins", session_id)?;
        Ok(v.map(|v| {
            (
                v.get("externalId").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                v.get("role").and_then(|x| x.as_str()).unwrap_or("member").to_string(),
            )
        }))
    }

    // ---------------------------------------------------------------- 配对码（一次性、限时）

    pub fn put_pairing_code(&self, code: &PairingCode) -> Result<()> {
        self.db.put("pairing_codes", &code.code, code)
    }

    pub fn get_pairing_code(&self, code: &str) -> Result<Option<PairingCode>> {
        self.db.get("pairing_codes", code)
    }

    pub fn list_pairing_codes(&self) -> Result<Vec<PairingCode>> {
        let mut list: Vec<PairingCode> = self.db.list("pairing_codes")?;
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(list)
    }

    /// 兑销（单次有效）：删除并返回原码
    pub fn consume_pairing_code(&self, code: &str) -> Result<Option<PairingCode>> {
        match self.db.get::<PairingCode>("pairing_codes", code)? {
            Some(c) => {
                self.db.delete("pairing_codes", code)?;
                Ok(Some(c))
            }
            None => Ok(None),
        }
    }

    // ---------------------------------------------------------------- 事件触发器（文件监听 / webhook 事件源）

    pub fn put_event_trigger(&self, trigger: &EventTrigger) -> Result<()> {
        self.db.put("event_triggers", &trigger.id, trigger)
    }

    pub fn get_event_trigger(&self, id: &str) -> Result<Option<EventTrigger>> {
        self.db.get("event_triggers", id)
    }

    pub fn list_event_triggers(&self, kind: Option<&str>) -> Result<Vec<EventTrigger>> {
        let mut list: Vec<EventTrigger> = self.db.list("event_triggers")?;
        if let Some(k) = kind {
            list.retain(|t| t.kind == k);
        }
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(list)
    }

    pub fn delete_event_trigger(&self, id: &str) -> Result<()> {
        self.db.delete("event_triggers", id)
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    fn test_store() -> Store {
        let dir = std::env::temp_dir().join(format!("exm-store-{}", uuid::Uuid::new_v4()));
        Store::open(&dir).expect("打开测试存储失败")
    }

    #[test]
    fn 轮次快照_增查裁剪() {
        let store = test_store();
        for turn in 1..=3 {
            store
                .put_turn_snapshot(&TurnSnapshot {
                    id: format!("s1:{turn}"),
                    session_id: "s1".into(),
                    turn,
                    message_count: turn * 2,
                    checkpoint_ids: vec![format!("cp-{turn}")],
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    status: "done".into(),
                    created_at: format!("2026-01-0{turn}T00:00:00Z"),
                })
                .unwrap();
        }
        let list = store.list_turn_snapshots("s1").unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].turn, 1, "按轮次升序");
        let latest = store.latest_turn_snapshot("s1").unwrap().unwrap();
        assert_eq!(latest.turn, 3);
        assert!(store.latest_turn_snapshot("s2").unwrap().is_none(), "会话隔离");

        store.prune_turn_snapshots_after("s1", 2).unwrap();
        let list = store.list_turn_snapshots("s1").unwrap();
        assert_eq!(list.len(), 2, "裁剪后仅保留 turn<=2");
        assert_eq!(list[1].turn, 2);
    }

    #[test]
    fn 用户身份_绑定查询删除() {
        let store = test_store();
        let identity = UserIdentity {
            id: "telegram:10086".into(),
            channel: "telegram".into(),
            external_id: "10086".into(),
            display_name: "测试用户".into(),
            role: "member".into(),
            paired_at: "2026-01-01T00:00:00Z".into(),
            note: "冒烟配对".into(),
        };
        store.put_identity(&identity).unwrap();
        let got = store.get_identity("telegram", "10086").unwrap().unwrap();
        assert_eq!(got.display_name, "测试用户");
        assert_eq!(got.role, "member");
        assert!(store.get_identity("telegram", "other").unwrap().is_none());
        assert_eq!(store.list_identities().unwrap().len(), 1);
        store.delete_identity("telegram:10086").unwrap();
        assert!(store.get_identity("telegram", "10086").unwrap().is_none());
    }

    #[test]
    fn 事件触发器_按类型过滤() {
        let store = test_store();
        for (id, kind) in [("t1", "file_watch"), ("t2", "event_webhook")] {
            store
                .put_event_trigger(&EventTrigger {
                    id: id.into(),
                    kind: kind.into(),
                    name: id.into(),
                    pattern: "*.rs".into(),
                    group: None,
                    prompt: "事件：{event}".into(),
                    enabled: true,
                    created_at: "2026-01-01T00:00:00Z".into(),
                    last_fired_at: None,
                })
                .unwrap();
        }
        assert_eq!(store.list_event_triggers(None).unwrap().len(), 2);
        let fw = store.list_event_triggers(Some("file_watch")).unwrap();
        assert_eq!(fw.len(), 1);
        assert_eq!(fw[0].id, "t1");
        assert!(store.get_event_trigger("t2").unwrap().is_some());
        store.delete_event_trigger("t2").unwrap();
        assert!(store.get_event_trigger("t2").unwrap().is_none());
    }
}
