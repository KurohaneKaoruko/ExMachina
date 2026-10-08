//! SQLite 存储底座(B1)——主库 = 会话/消息/任务图/回流/账本/快照/审计。
//!
//! 布局(见 docs/多核心连结架构.md §1):
//! - `<data>/store.sqlite` 主库(WAL,单写者互斥锁)
//! - 浅层记忆(MEMORY.md)与深层记忆(分库 SQLite)是独立层级,不在本模块
//! - 证据 / 事件 / 检查点引用 / outbox / 会话来源 / 审批 / 身份 / 配对码 / 事件触发器
//!   本批次仍走 FsDb(低频小集合),后续批次按需演进
//! - 打开时自动检测旧 FSDB 数据并一次性迁移(原文件归档留存)

use crate::fsdb::FsDb;
use crate::types::*;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    group_id TEXT NOT NULL,
    rolling_summary TEXT,
    summary_upto INTEGER,
    parent_session TEXT,
    parent_upto INTEGER,
    ledger TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_group ON sessions(group_id, updated_at DESC);
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    agent_id TEXT,
    statements TEXT NOT NULL,
    created_at TEXT NOT NULL,
    token_usage TEXT,
    thinking TEXT,
    tool_calls TEXT NOT NULL DEFAULT '[]',
    archived INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_messages ON messages(session_id, archived, seq);
CREATE TABLE IF NOT EXISTS graphs (
    session_id TEXT PRIMARY KEY,
    graph TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS graph_history (
    session_id TEXT NOT NULL,
    graph_id TEXT NOT NULL,
    graph TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_reports (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    report TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_reports_node ON sync_reports(node_id, created_at DESC);
CREATE TABLE IF NOT EXISTS tool_audit (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    args TEXT NOT NULL,
    result_summary TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_agent ON tool_audit(agent_id, created_at DESC);
CREATE TABLE IF NOT EXISTS turn_snapshots (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn INTEGER NOT NULL,
    message_count INTEGER NOT NULL,
    checkpoint_ids TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL,
    completion_tokens INTEGER NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS usage (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL,
    completion_tokens INTEGER NOT NULL,
    model TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_usage_session ON usage(session_id, created_at DESC);
"#;

const MAX_AUDIT_ROWS: usize = 2000;

pub struct SqliteStore {
    conn: Mutex<Connection>,
    /// 本批次仍走 FsDb 的低频集合(证据/事件/检查点/outbox/来源/审批/身份/配对/触发器)
    pub(crate) fsdb: FsDb,
    root: PathBuf,
}

fn now_iso() -> String {
    crate::types::now_iso()
}

fn new_id() -> String {
    crate::types::new_id()
}

impl SqliteStore {
    /// 打开主库;若存在旧 FSDB 数据且主库为空,自动一次性迁移
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let conn = Connection::open(root.join("store.sqlite"))
            .context("打开 store.sqlite 失败")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(SCHEMA).context("初始化 schema 失败")?;

        let store = SqliteStore { conn: Mutex::new(conn), fsdb: FsDb::open(&root)?, root: root.clone() };

        let migrated: bool = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM meta WHERE k='migrated' AND v='1'", [], |r| r.get::<_, bool>(0))
            .unwrap_or(false);
        if !migrated {
            let n = store.migrate_from_fsdb()?;
            let conn = store.conn.lock().unwrap();
            conn.execute("INSERT OR REPLACE INTO meta(k,v) VALUES('migrated','1')", [])?;
            drop(conn);
            if n > 0 {
                eprintln!("[store] 已从 FSDB 迁移 {n} 条记录至 store.sqlite(原文件归档留存)");
            }
        }
        Ok(store)
    }

    pub fn root(&self) -> String {
        self.root.display().to_string()
    }

    // ------------------------------------------------- 自动迁移(FSDB → SQLite)

    fn migrate_from_fsdb(&self) -> Result<usize> {
        let legacy = FsDb::open(&self.root)?;
        let mut n = 0usize;
        let conn = self.conn.lock().unwrap();
        // 会话
        for s in legacy.list::<Session>("sessions").unwrap_or_default() {
            let _ = Self::insert_session_conn(&conn, &s);
            n += 1;
        }
        // 消息(按会话遍历,含归档)
        for s in legacy.list::<Session>("sessions").unwrap_or_default() {
            for (i, m) in legacy.read_lines::<ChatMessage>("messages", &s.id, 0).unwrap_or_default().into_iter().enumerate() {
                let _ = Self::insert_message_conn(&conn, &s.id, (i + 1) as i64, &m, false);
                n += 1;
            }
            for m in legacy.read_lines::<ChatMessage>("message_archive", &s.id, 0).unwrap_or_default() {
                let _ = Self::insert_message_conn(&conn, &s.id, 0, &m, true);
                n += 1;
            }
        }
        // 任务图 + 历史
        for g in legacy.list::<TaskGraph>("graphs").unwrap_or_default() {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO graphs(session_id, graph, updated_at) VALUES(?1,?2,?3)",
                rusqlite::params![g.session_id, serde_json::to_string(&g).unwrap_or_default(), now_iso()],
            );
            n += 1;
        }
        // 回流
        for v in legacy.list::<serde_json::Value>("sync_reports").unwrap_or_default() {
            let id = v["id"].as_str().unwrap_or_default();
            let node = v["taskNodeId"].as_str().unwrap_or_default();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO sync_reports(id,node_id,report,created_at) VALUES(?1,?2,?3,?4)",
                rusqlite::params![id, node, v.to_string(), now_iso()],
            );
            n += 1;
        }
        // 快照
        for snap in legacy.list::<TurnSnapshot>("turn_snapshots").unwrap_or_default() {
            let _ = Self::put_snapshot_conn(&conn, &snap);
            n += 1;
        }
        Ok(n)
    }

    // ------------------------------------------------- 会话 / 三账

    fn insert_session_conn(conn: &Connection, s: &Session) -> anyhow::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO sessions(id,title,status,group_id,rolling_summary,summary_upto,parent_session,parent_upto,ledger,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                s.id,
                s.title,
                s.status,
                s.group_id,
                s.rolling_summary,
                s.summary_upto,
                s.parent_session,
                s.parent_upto,
                serde_json::to_string(&s.ledger)?,
                s.created_at,
                s.updated_at,
            ],
        )?;
        Ok(())
    }

    fn row_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        Ok(Session {
            id: row.get("id")?,
            title: row.get("title")?,
            status: row.get("status")?,
            group_id: row.get("group_id")?,
            rolling_summary: row.get("rolling_summary")?,
            summary_upto: row.get("summary_upto")?,
            parent_session: row.get("parent_session")?,
            parent_upto: row.get("parent_upto")?,
            ledger: serde_json::from_str(&row.get::<_, String>("ledger")?).unwrap_or_default(),
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            last_message_preview: None,
            last_active_at: None,
        })
    }

    pub fn create_session(&self, title: &str, group_id: &str) -> Result<Session> {
        let now = now_iso();
        let s = Session {
            id: new_id(),
            title: title.to_string(),
            status: "active".into(),
            group_id: if group_id.trim().is_empty() { "exmachina".into() } else { group_id.to_string() },
            rolling_summary: None,
            summary_upto: None,
            parent_session: None,
            parent_upto: None,
            last_message_preview: None,
            last_active_at: None,
            ledger: SessionLedger::default(),
            created_at: now.clone(),
            updated_at: now,
        };
        let conn = self.conn.lock().unwrap();
        Self::insert_session_conn(&conn, &s)?;
        Ok(s)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT * FROM sessions WHERE id=?1")?;
        let mut rows = st.query(rusqlite::params![id])?;
        Ok(rows.next()?.map(|r| Self::row_session(&r)).transpose()?)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT * FROM sessions ORDER BY updated_at DESC LIMIT 200")?;
        let rows = st.query_map([], |r| Self::row_session(r))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 会话最后一条消息的纯文本预览
    pub fn last_message_preview(&self, session_id: &str, max_chars: usize) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        let statements: String = conn
            .query_row(
                "SELECT statements FROM messages WHERE session_id=?1 AND archived=0 ORDER BY seq DESC LIMIT 1",
                rusqlite::params![session_id],
                |r| r.get::<_, String>(0),
            )
            .ok()?;
        let v: serde_json::Value = serde_json::from_str(&statements).ok()?;
        let text = v
            .as_array()
            .and_then(|a| a.first())
            .and_then(|s| s.get("text"))
            .and_then(|t| t.as_str())?
            .trim()
            .to_string();
        Some(text.chars().take(max_chars).collect())
    }

    pub fn update_ledger(&self, session_id: &str, ledger: &SessionLedger) -> Result<()> {
        self.mutate_ledger(session_id, |l| *l = ledger.clone())?;
        Ok(())
    }

    /// 原子读-改-写三账(SQLite 单写者互斥,并发回流安全)
    pub fn mutate_ledger<F>(&self, session_id: &str, f: F) -> Result<SessionLedger>
    where
        F: FnOnce(&mut SessionLedger),
    {
        let conn = self.conn.lock().unwrap();
        let mut session = Self::get_session_conn(&conn, session_id)?.ok_or_else(|| anyhow::anyhow!("会话不存在: {session_id}"))?;
        f(&mut session.ledger);
        session.updated_at = now_iso();
        let ledger = session.ledger.clone();
        Self::insert_session_conn(&conn, &session)?;
        Ok(ledger)
    }

    pub fn ledger_of(&self, session_id: &str) -> Result<SessionLedger> {
        let conn = self.conn.lock().unwrap();
        Ok(Self::get_session_conn(&conn, session_id)?.map(|s| s.ledger).unwrap_or_default())
    }

    fn get_session_conn(conn: &Connection, id: &str) -> anyhow::Result<Option<Session>> {
        let mut st = conn.prepare("SELECT * FROM sessions WHERE id=?1")?;
        let mut rows = st.query(rusqlite::params![id])?;
        Ok(rows.next()?.map(|r| Self::row_session(&r)).transpose()?)
    }

    pub fn update_session_title(&self, session_id: &str, title: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET title=?2, updated_at=?3 WHERE id=?1",
            rusqlite::params![title.trim(), now_iso(), session_id],
        )?;
        Ok(())
    }

    pub fn update_compaction(&self, session_id: &str, summary: &str, upto: usize) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET rolling_summary=?2, summary_upto=?3, updated_at=?4 WHERE id=?1",
            rusqlite::params![
                session_id,
                if summary.trim().is_empty() { None } else { Some(summary.to_string()) },
                upto,
                now_iso()
            ],
        )?;
        Ok(())
    }

    // ------------------------------------------------- 消息

    fn insert_message_conn(
        conn: &Connection,
        session_id: &str,
        seq: i64,
        m: &ChatMessage,
        archived: bool,
    ) -> anyhow::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO messages(id,session_id,seq,role,agent_id,statements,created_at,token_usage,thinking,tool_calls,archived)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                m.id,
                session_id,
                seq,
                serde_json::to_value(&m.role).unwrap_or(serde_json::json!("user")).as_str().unwrap_or("user").to_string(),
                m.agent_id,
                serde_json::to_string(&m.statements)?,
                m.created_at,
                m.token_usage.as_ref().and_then(|u| serde_json::to_string(u).ok()),
                m.thinking,
                serde_json::to_string(&m.tool_calls)?,
                archived as i64,
            ],
        )?;
        Ok(())
    }

    fn row_message(r: &rusqlite::Row) -> rusqlite::Result<ChatMessage> {
        Ok(ChatMessage {
            id: r.get("id")?,
            session_id: r.get("session_id")?,
            role: {
                let raw: String = r.get("role")?;
                serde_json::from_value(serde_json::Value::String(raw)).unwrap_or(MessageRole::User)
            },
            agent_id: r.get("agent_id")?,
            statements: serde_json::from_str(&r.get::<_, String>("statements")?).unwrap_or_default(),
            created_at: r.get("created_at")?,
            token_usage: r
                .get::<_, Option<String>>("token_usage")?
                .and_then(|s| serde_json::from_str(&s).ok()),
            thinking: r.get("thinking")?,
            tool_calls: serde_json::from_str(&r.get::<_, String>("tool_calls")?).unwrap_or_default(),
            last_message_preview: None,
            last_active_at: None,
        })
    }

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
            thinking: None,
            tool_calls: Vec::new(),
            last_message_preview: None,
            last_active_at: None,
        };
        let conn = self.conn.lock().unwrap();
        let seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM messages WHERE session_id=?1 AND archived=0",
            rusqlite::params![session_id],
            |r| r.get(0),
        )?;
        Self::insert_message_conn(&conn, session_id, seq, &msg, false)?;
        Ok(msg)
    }

    /// 带过程数据的消息落盘(思维链 + 工具轨迹)
    pub fn add_message_full(
        &self,
        session_id: &str,
        role: MessageRole,
        agent_id: Option<&str>,
        statements: Vec<Statement>,
        thinking: Option<String>,
        tool_calls: Vec<crate::round_trace::ToolCallRecord>,
    ) -> Result<ChatMessage> {
        let mut msg = self.add_message(session_id, role, agent_id, statements)?;
        msg.thinking = thinking.filter(|s| !s.trim().is_empty());
        msg.tool_calls = tool_calls;
        let conn = self.conn.lock().unwrap();
        let seq: i64 = conn.query_row(
            "SELECT seq FROM messages WHERE id=?1",
            rusqlite::params![msg.id],
            |r| r.get(0),
        )?;
        Self::insert_message_conn(&conn, session_id, seq, &msg, false)?;
        Ok(msg)
    }

    pub fn list_messages(&self, session_id: &str, limit: usize) -> Result<Vec<ChatMessage>> {
        let conn = self.conn.lock().unwrap();
        let lim: i64 = if limit == 0 { -1 } else { limit as i64 };
        let mut st = conn.prepare(
            "SELECT * FROM (SELECT * FROM messages WHERE session_id=?1 AND archived=0 ORDER BY seq DESC LIMIT ?2) ORDER BY seq ASC",
        )?;
        let rows = st.query_map(rusqlite::params![session_id, lim], |r| Self::row_message(r))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 截断活跃消息流:保留前 keep 条,被截断部分标记归档(可查)
    pub fn truncate_messages(&self, session_id: &str, keep: usize) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE messages SET archived=1 WHERE session_id=?1 AND archived=0 AND id NOT IN (
                SELECT id FROM messages WHERE session_id=?1 AND archived=0 ORDER BY seq ASC LIMIT ?2
            )",
            rusqlite::params![session_id, keep as i64],
        )?;
        Ok(n)
    }

    pub fn list_archived_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT * FROM messages WHERE session_id=?1 AND archived=1 ORDER BY seq ASC",
        )?;
        let rows = st.query_map(rusqlite::params![session_id], |r| Self::row_message(r))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 复制会话历史前缀(fork 用)
    pub fn copy_messages_prefix(&self, src_session: &str, dst_session: &str, keep: usize) -> Result<usize> {
        let msgs = self.list_messages(src_session, 0)?;
        eprintln!("[copy] src={} list={} keep={}", src_session, msgs.len(), keep);
        let prefix: Vec<ChatMessage> = msgs.into_iter().take(keep).collect();
        let conn = self.conn.lock().unwrap();
        for (i, m) in prefix.iter().enumerate() {
            let mut copy = m.clone();
            copy.id = new_id(); // 主键 = id:跨会话副本必须换新 id,否则 REPLACE 会"搬走"源消息
            Self::insert_message_conn(&conn, dst_session, (i + 1) as i64, &copy, false)?;
        }
        let back: Vec<ChatMessage> = {
            let mut st = conn.prepare("SELECT * FROM messages WHERE session_id=?1 AND archived=0 ORDER BY seq ASC").unwrap();
            st.query_map(rusqlite::params![dst_session], |r| Self::row_message(r)).unwrap().filter_map(|r| r.ok()).collect()
        };
        Ok(prefix.len())
    }

    pub fn record_checkpoint_ref(&self, session_id: &str, date: &str, rel: &str) -> Result<()> {
        self.fsdb.append_line(
            "turn_checkpoints",
            session_id,
            &serde_json::json!({ "ref": format!("{date}/{rel}"), "at": now_iso() }),
        )
    }

    pub fn drain_checkpoint_refs(&self, session_id: &str) -> Result<Vec<String>> {
        let items: Vec<serde_json::Value> = self.fsdb.read_lines("turn_checkpoints", session_id, 0)?;
        if !items.is_empty() {
            self.fsdb.rewrite_lines::<serde_json::Value>("turn_checkpoints", session_id, &[])?;
        }
        Ok(items
            .into_iter()
            .filter_map(|v| v.get("ref").and_then(|r| r.as_str()).map(|s| s.to_string()))
            .collect())
    }

    pub fn insert_session(&self, session: &Session) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::insert_session_conn(&conn, session)
    }

    // ------------------------------------------------- 任务图

    pub fn save_graph(&self, graph: &TaskGraph) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let json = serde_json::to_string(graph)?;
        conn.execute(
            "INSERT OR REPLACE INTO graphs(session_id, graph, updated_at) VALUES(?1,?2,?3)",
            rusqlite::params![graph.session_id, json, now_iso()],
        )?;
        // 历史留痕(幂等:以图 id 去重)
        let known: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_history WHERE session_id=?1 AND graph_id=?2",
                rusqlite::params![graph.session_id, graph.id],
                |r| r.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !known {
            conn.execute(
                "INSERT INTO graph_history(session_id, graph_id, graph, created_at) VALUES(?1,?2,?3,?4)",
                rusqlite::params![graph.session_id, graph.id, json, now_iso()],
            )?;
        }
        Ok(())
    }

    pub fn latest_graph(&self, session_id: &str) -> Result<Option<TaskGraph>> {
        let conn = self.conn.lock().unwrap();
        let json: Option<String> = conn
            .query_row("SELECT graph FROM graphs WHERE session_id=?1", rusqlite::params![session_id], |r| {
                r.get(0)
            })
            .ok();
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    // ------------------------------------------------- 回流与证据

    pub fn add_sync_report(&self, report: &SyncReport) -> Result<String> {
        let id = new_id();
        let mut value = serde_json::to_value(report)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("id".into(), serde_json::json!(id));
            obj.insert("createdAt".into(), serde_json::json!(now_iso()));
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_reports(id,node_id,report,created_at) VALUES(?1,?2,?3,?4)",
            rusqlite::params![id, report.task_node_id, value.to_string(), now_iso()],
        )?;
        Ok(id)
    }

    /// 节点最新回流(断点续跑回填用)
    pub fn sync_report_of_node(&self, node_id: &str) -> Result<Option<SyncReport>> {
        let v = self.report_for_node(node_id)?;
        Ok(v.and_then(|v| serde_json::from_value(v).ok()))
    }

    pub fn report_for_node(&self, node_id: &str) -> Result<Option<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let json: Option<String> = conn
            .query_row(
                "SELECT report FROM sync_reports WHERE node_id=?1 ORDER BY created_at DESC LIMIT 1",
                rusqlite::params![node_id],
                |r| r.get(0),
            )
            .ok();
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
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
            self.fsdb.append_line("evidence", session_id, &value)?;
        }
        Ok(())
    }

    pub fn list_evidence(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        self.fsdb.read_lines("evidence", session_id, 0)
    }

    // ------------------------------------------------- 工具审计(SQLite)

    pub fn audit_tool(
        &self,
        agent_id: &str,
        tool: &str,
        args: &serde_json::Value,
        result_summary: &str,
        duration_ms: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tool_audit(id,agent_id,tool,args,result_summary,duration_ms,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![new_id(), agent_id, tool, args.to_string(), result_summary, duration_ms as i64, now_iso()],
        )?;
        // 容量裁剪:每 agent 超上限保留最近一半
        conn.execute(
            "DELETE FROM tool_audit WHERE agent_id=?1 AND id NOT IN (
                SELECT id FROM tool_audit WHERE agent_id=?1 ORDER BY created_at DESC LIMIT ?2
            )",
            rusqlite::params![agent_id, (MAX_AUDIT_ROWS / 2) as i64],
        )?;
        Ok(())
    }

    pub fn list_tool_audit(&self, agent_id: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT args, result_summary, duration_ms, created_at, agent_id, tool, id FROM tool_audit
             WHERE agent_id=?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = st.query_map(rusqlite::params![agent_id, limit as i64], |r| {
            Ok(serde_json::json!({
                "agentId": r.get::<_, String>("agent_id")?,
                "tool": r.get::<_, String>("tool")?,
                "args": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>("args").unwrap_or_default()).unwrap_or_default(),
                "resultSummary": r.get::<_, String>("result_summary")?,
                "durationMs": r.get::<_, i64>("duration_ms")?,
                "createdAt": r.get::<_, String>("created_at")?,
                "id": r.get::<_, String>("id")?,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 跨个体聚合的工具审计查询(agent/tool 过滤,时间倒序,limit 封顶)
    pub fn list_tool_audit_filtered(
        &self,
        agent: Option<&str>,
        tool: Option<&str>,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT args, result_summary, duration_ms, created_at, agent_id, tool, id FROM tool_audit
             ORDER BY created_at DESC LIMIT 5000",
        )?;
        let mut all: Vec<serde_json::Value> = st
            .query_map([], |r| {
                Ok(serde_json::json!({
                    "agentId": r.get::<_, String>("agent_id")?,
                    "tool": r.get::<_, String>("tool")?,
                    "args": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>("args").unwrap_or_default()).unwrap_or_default(),
                    "resultSummary": r.get::<_, String>("result_summary")?,
                    "durationMs": r.get::<_, i64>("duration_ms")?,
                    "createdAt": r.get::<_, String>("created_at")?,
                    "id": r.get::<_, String>("id")?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();
        if let Some(a) = agent {
            all.retain(|v| v.get("agentId").and_then(|x| x.as_str()) == Some(a));
        }
        if let Some(t) = tool {
            all.retain(|v| {
                v.get("tool")
                    .and_then(|x| x.as_str())
                    .map(|x| x.to_lowercase())
                    .unwrap_or_default()
                    .contains(&t.to_lowercase())
            });
        }
        all.truncate(limit);
        Ok(all)
    }

    // ------------------------------------------------- 用量台账(SQLite)

    pub fn add_usage(&self, session_id: &str, prompt_tokens: u64, completion_tokens: u64, model: &str) -> Result<()> {
        if prompt_tokens == 0 && completion_tokens == 0 {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO usage(id,session_id,prompt_tokens,completion_tokens,model,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![new_id(), session_id, prompt_tokens as i64, completion_tokens as i64, model, now_iso()],
        )?;
        Ok(())
    }

    pub fn usage_total(&self, session_id: &str) -> (u64, u64, usize) {
        let conn = self.conn.lock().unwrap();
        let Ok(mut st) = conn.prepare(
            "SELECT COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0), COUNT(*)
             FROM usage WHERE session_id=?1",
        ) else {
            return (0, 0, 0);
        };
        st.query_row(rusqlite::params![session_id], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as usize))
        })
        .unwrap_or((0, 0, 0))
    }

    pub fn usage_recent(&self, session_id: &str, limit: usize) -> Vec<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut st) = conn.prepare(
            "SELECT prompt_tokens, completion_tokens, model, created_at FROM usage
             WHERE session_id=?1 ORDER BY created_at DESC LIMIT ?2",
        ) else {
            return Vec::new();
        };
        st.query_map(rusqlite::params![session_id, limit as i64], |r| {
            Ok(serde_json::json!({
                "promptTokens": r.get::<_, i64>(0)?,
                "completionTokens": r.get::<_, i64>(1)?,
                "model": r.get::<_, String>(2)?,
                "createdAt": r.get::<_, String>(3)?,
            }))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    // ------------------------------------------------- 事件溯源(FsDb,B1 保留)

    pub fn append_event(&self, env: &Envelope) -> Result<()> {
        self.fsdb.append_line("events", &env.session_id, env)
    }

    pub fn list_events(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        self.fsdb.read_lines("events", session_id, 0)
    }

    // ------------------------------------------------- 执行审批(FsDb,B1 保留)

    pub fn add_approval(&self, req: &ApprovalRequest) -> Result<()> {
        self.fsdb.put("approvals", &req.id, req)
    }

    pub fn get_approval(&self, id: &str) -> Result<Option<ApprovalRequest>> {
        self.fsdb.get("approvals", id)
    }

    pub fn update_approval(&self, req: &ApprovalRequest) -> Result<()> {
        self.fsdb.put("approvals", &req.id, req)
    }

    /// 按组列出会话
    pub fn list_sessions_in_group(&self, group_id: &str) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT * FROM sessions WHERE group_id=?1 ORDER BY updated_at DESC LIMIT 200",
        )?;
        let rows = st.query_map(rusqlite::params![group_id], |r| Self::row_session(r))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 删除会话及其派生数据(SQLite 侧 + FsDb 侧)
    pub fn delete_session(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE id=?1", rusqlite::params![id])?;
        conn.execute("DELETE FROM messages WHERE session_id=?1", rusqlite::params![id])?;
        conn.execute("DELETE FROM graphs WHERE session_id=?1", rusqlite::params![id])?;
        conn.execute("DELETE FROM graph_history WHERE session_id=?1", rusqlite::params![id])?;
        conn.execute("DELETE FROM turn_snapshots WHERE session_id=?1", rusqlite::params![id])?;
        drop(conn);
        // FsDb 侧(本批次保留的集合)
        for (col, key) in [("events", id), ("evidence", id), ("turn_checkpoints", id)] {
            let _ = self.fsdb.delete(col, key);
        }
        Ok(())
    }

    pub fn list_approvals(&self, status: Option<&str>, limit: usize) -> Result<Vec<ApprovalRequest>> {
        let mut list: Vec<ApprovalRequest> = self.fsdb.list("approvals")?;
        if let Some(s) = status {
            list.retain(|r| r.status == s);
        }
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        list.truncate(limit);
        Ok(list)
    }

    // ------------------------------------------------- 轮次快照(SQLite)

    fn put_snapshot_conn(conn: &Connection, snap: &TurnSnapshot) -> anyhow::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO turn_snapshots(id,session_id,turn,message_count,checkpoint_ids,prompt_tokens,completion_tokens,status,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                snap.id,
                snap.session_id,
                snap.turn,
                snap.message_count,
                serde_json::to_string(&snap.checkpoint_ids)?,
                snap.prompt_tokens,
                snap.completion_tokens,
                snap.status,
                snap.created_at,
            ],
        )?;
        Ok(())
    }

    fn row_snapshot(r: &rusqlite::Row) -> rusqlite::Result<TurnSnapshot> {
        Ok(TurnSnapshot {
            id: r.get("id")?,
            session_id: r.get("session_id")?,
            turn: r.get("turn")?,
            message_count: r.get("message_count")?,
            checkpoint_ids: serde_json::from_str(&r.get::<_, String>("checkpoint_ids")?).unwrap_or_default(),
            prompt_tokens: r.get("prompt_tokens")?,
            completion_tokens: r.get("completion_tokens")?,
            status: r.get("status")?,
            created_at: r.get("created_at")?,
        })
    }

    pub fn put_turn_snapshot(&self, snap: &TurnSnapshot) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::put_snapshot_conn(&conn, snap)
    }

    pub fn list_turn_snapshots(&self, session_id: &str) -> Result<Vec<TurnSnapshot>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT * FROM turn_snapshots WHERE session_id=?1 ORDER BY turn ASC, created_at ASC",
        )?;
        let rows = st.query_map(rusqlite::params![session_id], |r| Self::row_snapshot(r))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn latest_turn_snapshot(&self, session_id: &str) -> Result<Option<TurnSnapshot>> {
        Ok(self.list_turn_snapshots(session_id)?.pop())
    }

    pub fn prune_turn_snapshots_after(&self, session_id: &str, upto_turn: usize) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM turn_snapshots WHERE session_id=?1 AND turn>?2",
            rusqlite::params![session_id, upto_turn as i64],
        )?;
        Ok(())
    }

    // ------------------------------------------------- 以下集合本批次保留 FsDb

    pub fn put_identity(&self, identity: &UserIdentity) -> Result<()> {
        self.fsdb.put("identities", &identity.id, identity)
    }

    pub fn get_identity(&self, channel: &str, external_id: &str) -> Result<Option<UserIdentity>> {
        self.fsdb.get("identities", &format!("{channel}:{external_id}"))
    }

    pub fn get_identity_by_id(&self, id: &str) -> Result<Option<UserIdentity>> {
        self.fsdb.get("identities", id)
    }

    pub fn list_identities(&self) -> Result<Vec<UserIdentity>> {
        let mut list: Vec<UserIdentity> = self.fsdb.list("identities")?;
        list.sort_by(|a, b| b.paired_at.cmp(&a.paired_at));
        Ok(list)
    }

    pub fn delete_identity(&self, id: &str) -> Result<()> {
        self.fsdb.delete("identities", id)
    }

    pub fn outbox_push(&self, session_id: &str, kind: &str, path: &str, caption: &str) -> Result<()> {
        self.fsdb.append_line(
            "session_outbox",
            session_id,
            &serde_json::json!({ "kind": kind, "path": path, "caption": caption, "at": now_iso() }),
        )
    }

    pub fn outbox_drain(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        let items: Vec<serde_json::Value> = self.fsdb.read_lines("session_outbox", session_id, 0)?;
        if !items.is_empty() {
            self.fsdb.rewrite_lines::<serde_json::Value>("session_outbox", session_id, &[])?;
        }
        Ok(items)
    }

    pub fn put_session_origin(&self, session_id: &str, external_id: &str, role: &str) -> Result<()> {
        self.fsdb.put(
            "session_origins",
            session_id,
            &serde_json::json!({ "externalId": external_id, "role": role }),
        )
    }

    pub fn get_session_origin(&self, session_id: &str) -> Result<Option<(String, String)>> {
        let v: Option<serde_json::Value> = self.fsdb.get("session_origins", session_id)?;
        Ok(v.map(|v| {
            (
                v.get("externalId").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                v.get("role").and_then(|x| x.as_str()).unwrap_or("member").to_string(),
            )
        }))
    }

    pub fn put_pairing_code(&self, code: &PairingCode) -> Result<()> {
        self.fsdb.put("pairing_codes", &code.code, code)
    }

    pub fn get_pairing_code(&self, code: &str) -> Result<Option<PairingCode>> {
        self.fsdb.get("pairing_codes", code)
    }

    pub fn list_pairing_codes(&self) -> Result<Vec<PairingCode>> {
        let mut list: Vec<PairingCode> = self.fsdb.list("pairing_codes")?;
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(list)
    }

    pub fn consume_pairing_code(&self, code: &str) -> Result<Option<PairingCode>> {
        match self.fsdb.get::<PairingCode>("pairing_codes", code)? {
            Some(c) => {
                self.fsdb.delete("pairing_codes", code)?;
                Ok(Some(c))
            }
            None => Ok(None),
        }
    }

    pub fn put_event_trigger(&self, trigger: &EventTrigger) -> Result<()> {
        self.fsdb.put("event_triggers", &trigger.id, trigger)
    }

    pub fn get_event_trigger(&self, id: &str) -> Result<Option<EventTrigger>> {
        self.fsdb.get("event_triggers", id)
    }

    pub fn list_event_triggers(&self, kind: Option<&str>) -> Result<Vec<EventTrigger>> {
        let mut list: Vec<EventTrigger> = self.fsdb.list("event_triggers")?;
        if let Some(k) = kind {
            list.retain(|t| t.kind == k);
        }
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(list)
    }

    pub fn delete_event_trigger(&self, id: &str) -> Result<()> {
        self.fsdb.delete("event_triggers", id)
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;
    use crate::store::Store;
    use crate::types::*;

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

