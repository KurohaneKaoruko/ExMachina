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
}
