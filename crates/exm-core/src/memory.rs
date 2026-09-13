//! 记忆系统 —— 自我进化内核（docs/08）
//!
//! 存储（纯 Rust 文档存储，零 C 依赖）：
//!   `memory/entries/{id}.json`        记忆条目
//!   `memory/hash_index/{hash}.json`   内容哈希 → 条目 id（去重键）
//!   `memory/terms/bucket-XX.json`     倒排索引分桶：term → [(entry_id, weight)]
//!   `memory/links/{entryId}.jsonl`    记忆关联（derived_from/contradicts/supersedes）
//!   `memory/agent_stats/{agentId}.json` 个体可靠性统计
//!
//! 基础记忆 `memory.md` 由条目渲染（可随时再生成），是 DB 的人读视图。

use crate::fsdb::FsDb;
use crate::types::*;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const ENTRIES: &str = "memory/entries";
const HASH_INDEX: &str = "memory/hash_index";
const TERMS: &str = "memory/terms";
const LINKS: &str = "memory/links";
const STATS: &str = "memory/agent_stats";

// ---------------------------------------------------------------- 类型

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// 基础事实（环境、路径、约定）
    Fact,
    /// 决策与取舍
    Decision,
    /// 用户偏好（群体共享：对全体个体生效）
    Preference,
    /// 证据（A/B 级）
    Evidence,
    /// 会话摘要
    Digest,
    /// 教训（失败/受阻原因）
    Lesson,
}

impl MemoryKind {
    pub fn key(self) -> &'static str {
        match self {
            MemoryKind::Fact => "fact",
            MemoryKind::Decision => "decision",
            MemoryKind::Preference => "preference",
            MemoryKind::Evidence => "evidence",
            MemoryKind::Digest => "digest",
            MemoryKind::Lesson => "lesson",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "fact" => MemoryKind::Fact,
            "decision" => MemoryKind::Decision,
            "preference" => MemoryKind::Preference,
            "evidence" => MemoryKind::Evidence,
            "digest" => MemoryKind::Digest,
            "lesson" => MemoryKind::Lesson,
            _ => return None,
        })
    }
    fn boost(self) -> f64 {
        match self {
            MemoryKind::Preference => 0.60,
            MemoryKind::Decision => 0.45,
            MemoryKind::Lesson => 0.35,
            MemoryKind::Fact => 0.30,
            MemoryKind::Evidence => 0.15,
            MemoryKind::Digest => 0.05,
        }
    }
    pub fn all() -> [MemoryKind; 6] {
        [
            MemoryKind::Fact,
            MemoryKind::Decision,
            MemoryKind::Preference,
            MemoryKind::Evidence,
            MemoryKind::Digest,
            MemoryKind::Lesson,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub kind: MemoryKind,
    /// global | project | session
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// 组归属（docs/09 §6）：None = 全局可见；Some(gid) = 仅该组可见（旧数据视为全局）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub importance: f64,
    pub confidence: f64,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accessed_at: Option<String>,
    #[serde(default)]
    pub access_count: u32,
}

#[derive(Debug, Clone)]
pub struct MemoryDraft {
    pub kind: MemoryKind,
    pub scope: String,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub group_id: Option<String>,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub importance: f64,
    pub confidence: f64,
    pub source_ref: Option<String>,
}

impl MemoryDraft {
    pub fn new(kind: MemoryKind, title: impl Into<String>, body: impl Into<String>) -> Self {
        MemoryDraft {
            kind,
            scope: "project".into(),
            session_id: None,
            agent_id: None,
            group_id: None,
            title: title.into(),
            body: body.into(),
            tags: vec![],
            importance: 0.5,
            confidence: 0.7,
            source_ref: None,
        }
    }
    pub fn importance(mut self, v: f64) -> Self {
        self.importance = v.clamp(0.0, 1.0);
        self
    }
    pub fn tags(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn session(mut self, id: &str) -> Self {
        self.session_id = Some(id.to_string());
        self
    }
    pub fn agent(mut self, id: &str) -> Self {
        self.agent_id = Some(id.to_string());
        self
    }
    pub fn group(mut self, gid: &str) -> Self {
        self.group_id = Some(gid.to_string());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallHit {
    pub entry: MemoryEntry,
    pub score: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStat {
    pub agent_id: String,
    pub runs: u32,
    pub done: u32,
    pub blocked: u32,
    pub failed: u32,
    pub avg_confidence: f64,
    pub updated_at: String,
}

/// 可插拔嵌入器：默认词法实现；后续接入向量模型时替换即可（打分函数是唯一改动点）
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Vec<f32>;
    fn similarity(&self, a: &[f32], b: &[f32]) -> f64;
}

pub struct LexicalEmbedder;

impl Embedder for LexicalEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; 64];
        for t in tokens(text) {
            let h = t.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            v[(h % 64) as usize] += 1.0;
        }
        v
    }
    fn similarity(&self, a: &[f32], b: &[f32]) -> f64 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            (dot / (na * nb)) as f64
        }
    }
}

// ---------------------------------------------------------------- 记忆库

pub struct MemoryStore {
    db: FsDb,
    dir: PathBuf,
}

type TermBucket = std::collections::HashMap<String, Vec<(String, f64)>>;

/// 记忆分层过滤（docs/08）：个体 = 归属某智能体的私有记忆；群体 = 无归属共享
#[derive(Clone, Copy)]
enum AgentLayer<'a> {
    /// 群体 + 所有个体（管理视角）
    All,
    /// 仅群体（指挥体规划）
    SharedOnly,
    /// 某个体可见范围：其私有 + 群体
    Agent(&'a str),
}

impl MemoryStore {
    /// `data_dir` 为工作区数据根目录（与 Store 共用，但各自分集合）
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = data_dir.as_ref().to_path_buf();
        Ok(MemoryStore { db: FsDb::open(&dir)?, dir })
    }

    pub fn root(&self) -> String {
        self.dir.display().to_string()
    }

    // ---------------- 写入 ----------------

    /// 记住一条：按内容哈希去重（组维度隔离，同内容跨组各自成条）；重复命中则提升重要性/置信度并刷新时间
    pub fn remember(&self, draft: &MemoryDraft) -> Result<MemoryEntry> {
        let hash = content_hash(&draft.title, &draft.body, draft.group_id.as_deref());
        let now = now_iso();

        if let Some(existing_id) = self.hash_lookup(&hash)? {
            if let Some(mut entry) = self.get_entry(&existing_id)? {
                entry.importance = (entry.importance + 0.08).min(1.0);
                entry.confidence = entry.confidence.max(draft.confidence.clamp(0.0, 1.0));
                entry.updated_at = now;
                entry.access_count += 1;
                self.save_entry(&entry)?;
                return Ok(entry);
            }
        }

        let entry = MemoryEntry {
            id: new_id(),
            kind: draft.kind,
            scope: draft.scope.clone(),
            session_id: draft.session_id.clone(),
            agent_id: draft.agent_id.clone(),
            group_id: draft.group_id.clone(),
            title: draft.title.clone(),
            body: draft.body.clone(),
            tags: draft.tags.clone(),
            importance: draft.importance.clamp(0.0, 1.0),
            confidence: draft.confidence.clamp(0.0, 1.0),
            pinned: false,
            source_ref: draft.source_ref.clone(),
            created_at: now.clone(),
            updated_at: now,
            last_accessed_at: None,
            access_count: 0,
        };
        self.save_entry(&entry)?;
        self.db.put(HASH_INDEX, &hash, &serde_json::json!(entry.id))?;
        self.index_entry(&entry)?;
        Ok(entry)
    }

    fn save_entry(&self, entry: &MemoryEntry) -> Result<()> {
        self.db.put(ENTRIES, &entry.id, entry)
    }

    fn get_entry(&self, id: &str) -> Result<Option<MemoryEntry>> {
        self.db.get(ENTRIES, id)
    }

    fn hash_lookup(&self, hash: &str) -> Result<Option<String>> {
        let v: Option<serde_json::Value> = self.db.get(HASH_INDEX, hash)?;
        Ok(v.and_then(|x| x.as_str().map(|s| s.to_string())))
    }

    fn index_entry(&self, entry: &MemoryEntry) -> Result<()> {
        let mut terms: std::collections::HashMap<String, f64> = Default::default();
        let text = format!("{} {} {}", entry.title, entry.body, entry.tags.join(" "));
        for t in tokens(&text) {
            let w = if entry.title.to_lowercase().contains(&t) { 3.0 } else { 1.0 };
            *terms.entry(t).or_insert(0.0) += w;
        }
        for (term, weight) in terms {
            let id = entry.id.clone();
            self.db.with_bucket_map::<Vec<(String, f64)>, _>(TERMS, &term, |term, map| {
                let list = map.entry(term.to_string()).or_default();
                if let Some(slot) = list.iter_mut().find(|(eid, _)| *eid == id) {
                    slot.1 = weight;
                } else {
                    list.push((id.clone(), weight));
                }
            })?;
        }
        Ok(())
    }

    fn unindex_entry(&self, entry: &MemoryEntry) -> Result<()> {
        let text = format!("{} {} {}", entry.title, entry.body, entry.tags.join(" "));
        let mut seen = std::collections::HashSet::new();
        for t in tokens(&text) {
            if !seen.insert(t.clone()) {
                continue;
            }
            let id = entry.id.clone();
            self.db.with_bucket_map::<Vec<(String, f64)>, _>(TERMS, &t, |term, map| {
                if let Some(list) = map.get_mut(term) {
                    list.retain(|(eid, _)| *eid != id);
                }
            })?;
        }
        Ok(())
    }

    pub fn pin(&self, id: &str, pinned: bool) -> Result<()> {
        if let Some(mut e) = self.get_entry(id)? {
            e.pinned = pinned;
            e.updated_at = now_iso();
            self.save_entry(&e)?;
        }
        Ok(())
    }

    pub fn forget(&self, id: &str) -> Result<()> {
        if let Some(e) = self.get_entry(id)? {
            self.unindex_entry(&e)?;
            let hash = content_hash(&e.title, &e.body, e.group_id.as_deref());
            self.db.delete(HASH_INDEX, &hash)?;
        }
        self.db.delete(ENTRIES, id)
    }

    pub fn link(&self, from: &str, to: &str, relation: &str) -> Result<()> {
        self.db.append_line(
            LINKS,
            from,
            &serde_json::json!({ "from": from, "to": to, "relation": relation, "createdAt": now_iso() }),
        )
    }

    /// 全量重建倒排索引（索引可随时重建，docs/08 §6）
    pub fn reindex(&self) -> Result<usize> {
        self.db.clear_buckets(TERMS)?;
        let entries: Vec<MemoryEntry> = self.db.list(ENTRIES)?;
        for e in &entries {
            self.index_entry(e)?;
        }
        Ok(entries.len())
    }

    /// 时间衰减（固定项豁免），下限 floor
    pub fn decay(&self, half_life_days: f64, floor: f64) -> Result<usize> {
        let entries: Vec<MemoryEntry> = self.db.list(ENTRIES)?;
        let mut changed = 0;
        for mut e in entries {
            if e.pinned {
                continue;
            }
            let factor = 0.5f64.powf(age_days(&e.updated_at) / half_life_days.max(1.0));
            let next = (e.importance * factor).max(floor);
            if (next - e.importance).abs() > 1e-6 {
                e.importance = next;
                self.save_entry(&e)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    // ---------------- 检索 ----------------

    /// 群体记忆检索：仅共享条目（指挥体规划用，看不到任何个体私有记忆）。
    /// `group` 为 Some(gid) 时只返回「全局条目 + 该组条目」，其他组条目不可见。
    pub fn recall(
        &self,
        query: &str,
        limit: usize,
        scope: Option<&str>,
        group: Option<&str>,
    ) -> anyhow::Result<Vec<RecallHit>> {
        self.recall_filtered(query, limit, scope, AgentLayer::SharedOnly, group)
    }

    /// 全量检索（管理视角：群体 + 所有个体私有；组维度按 `group` 过滤）
    pub fn recall_all(&self, query: &str, limit: usize, group: Option<&str>) -> anyhow::Result<Vec<RecallHit>> {
        self.recall_filtered(query, limit, None, AgentLayer::All, group)
    }

    /// 个体检索：该智能体的私有记忆（agent_id 匹配）+ 群体记忆，个体条目加成
    pub fn recall_for_agent(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
        group: Option<&str>,
    ) -> anyhow::Result<Vec<RecallHit>> {
        self.recall_filtered(query, limit, None, AgentLayer::Agent(agent_id), group)
    }

    fn recall_filtered(
        &self,
        query: &str,
        limit: usize,
        scope: Option<&str>,
        layer: AgentLayer<'_>,
        group: Option<&str>,
    ) -> Result<Vec<RecallHit>> {
        let q_tokens = tokens(query);
        if q_tokens.is_empty() {
            return self.recent(limit, group);
        }

        // 候选：命中任一查询词项的条目 + 命中权重
        let mut overlap: std::collections::HashMap<String, (f64, usize)> = Default::default();
        for t in &q_tokens {
            let bucket: TermBucket = self.db.read_bucket_map(TERMS, t)?;
            if let Some(list) = bucket.get(t) {
                for (id, w) in list {
                    let slot = overlap.entry(id.clone()).or_insert((0.0, 0));
                    slot.0 += *w;
                    slot.1 += 1;
                }
            }
        }
        if overlap.is_empty() {
            return self.recent(limit, group);
        }
        let max_overlap = overlap.values().map(|v| v.0).fold(1.0f64, f64::max);

        let mut results: Vec<RecallHit> = Vec::new();
        for (id, (ov, matched)) in overlap {
            let Some(entry) = self.get_entry(&id)? else { continue };
            if let Some(sc) = scope {
                if entry.scope != sc && entry.scope != "global" && entry.scope != "project" {
                    continue;
                }
            }
            // 组隔离过滤（docs/09 §6）：全局条目处处可见；组条目仅组内可见
            if let Some(g) = group {
                if entry.group_id.as_deref().is_some_and(|eg| eg != g) {
                    continue;
                }
            }
            // 记忆分层过滤（docs/08）：个体 = 归属某智能体的私有记忆；群体 = 无归属共享
            let individual = entry.agent_id.is_some();
            let visible = match layer {
                AgentLayer::All => true,
                AgentLayer::SharedOnly => !individual,
                AgentLayer::Agent(owner) => {
                    !individual || entry.agent_id.as_deref() == Some(owner)
                }
            };
            if !visible {
                continue;
            }
            let overlap_ratio = ov / max_overlap;
            let coverage = matched as f64 / q_tokens.len() as f64;
            let recency = 0.5f64.powf(age_days(&entry.updated_at) / 30.0);
            let usage = (entry.access_count as f64 / 10.0).min(1.0);
            let pinned_bonus = if entry.pinned { 0.5 } else { 0.0 };
            let layer_bonus = if individual { 0.3 } else { 0.0 };

            let score = 3.0 * overlap_ratio
                + 1.2 * coverage
                + 1.5 * entry.importance
                + 1.0 * recency
                + 0.4 * usage
                + entry.kind.boost()
                + pinned_bonus
                + layer_bonus;

            let mut reasons = vec![
                format!("词项覆盖 {:.0}%", coverage * 100.0),
                format!("重要性 {:.2}", entry.importance),
                format!("时间衰减 {:.2}", recency),
                if individual { "个体记忆".to_string() } else { "群体记忆".to_string() },
            ];
            if let Some(g) = &entry.group_id {
                reasons.push(format!("组范围 {g}"));
            }
            if entry.pinned {
                reasons.push("固定记忆".into());
            }
            results.push(RecallHit { entry, score, reasons });
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        for h in &results {
            let mut e = h.entry.clone();
            e.access_count += 1;
            e.last_accessed_at = Some(now_iso());
            let _ = self.save_entry(&e);
        }
        Ok(results)
    }

    pub fn recent(&self, limit: usize, group: Option<&str>) -> Result<Vec<RecallHit>> {
        let mut entries: Vec<MemoryEntry> = self.db.list(ENTRIES)?;
        if let Some(g) = group {
            entries.retain(|e| e.group_id.as_deref().is_none_or(|eg| eg == g));
        }
        entries.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal))
                .then(b.updated_at.cmp(&a.updated_at))
        });
        entries.truncate(limit);
        Ok(entries
            .into_iter()
            .map(|e| {
                let score = e.importance;
                RecallHit { entry: e, score, reasons: vec!["按重要性/时间排序".into()] }
            })
            .collect())
    }

    pub fn list(&self, kind: Option<MemoryKind>, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.list_filtered(kind, None, limit, None)
    }

    /// 列表过滤：`agent` 为 Some(id) 时返回该个体私有 + 群体共享；`group` 过滤组范围；均 None 返回全部
    pub fn list_filtered(
        &self,
        kind: Option<MemoryKind>,
        agent: Option<&str>,
        limit: usize,
        group: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let mut entries: Vec<MemoryEntry> = self.db.list(ENTRIES)?;
        if let Some(k) = kind {
            entries.retain(|e| e.kind == k);
        }
        if let Some(id) = agent {
            entries.retain(|e| e.agent_id.as_deref() == Some(id) || e.agent_id.is_none());
        }
        if let Some(g) = group {
            entries.retain(|e| e.group_id.as_deref().is_none_or(|eg| eg == g));
        }
        entries.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal))
                .then(b.updated_at.cmp(&a.updated_at))
        });
        entries.truncate(limit);
        Ok(entries)
    }

    pub fn stats(&self) -> Result<serde_json::Value> {
        let entries: Vec<MemoryEntry> = self.db.list(ENTRIES)?;
        let mut by_kind = serde_json::Map::new();
        for k in MemoryKind::all() {
            let n = entries.iter().filter(|e| e.kind == k).count();
            if n > 0 {
                by_kind.insert(k.key().to_string(), serde_json::json!(n));
            }
        }
        let pinned = entries.iter().filter(|e| e.pinned).count();
        let individual = entries.iter().filter(|e| e.agent_id.is_some()).count();
        let shared = entries.len() - individual;
        let mut by_group: std::collections::BTreeMap<String, usize> = Default::default();
        for e in entries.iter().filter_map(|e| e.group_id.as_ref()) {
            *by_group.entry(e.clone()).or_default() += 1;
        }
        Ok(serde_json::json!({
            "total": entries.len(),
            "pinned": pinned,
            "individual": individual,
            "shared": shared,
            "groupScoped": entries.iter().filter(|e| e.group_id.is_some()).count(),
            "byGroup": by_group,
            "terms": self.db.bucket_key_count(TERMS)?,
            "indexBuckets": super::fsdb::TERM_BUCKETS,
            "byKind": by_kind,
            "dbPath": self.root(),
        }))
    }

    // ---------------- 个体统计（进化反馈） ----------------

    pub fn record_agent_outcome(&self, agent_id: &str, status: &str, confidence: f64) -> Result<()> {
        let mut stat: AgentStat = self
            .db
            .get::<AgentStat>(STATS, agent_id)?
            .unwrap_or_else(|| AgentStat {
                agent_id: agent_id.to_string(),
                runs: 0,
                done: 0,
                blocked: 0,
                failed: 0,
                avg_confidence: 0.0,
                updated_at: now_iso(),
            });
        let total_conf = stat.avg_confidence * stat.runs as f64 + confidence;
        stat.runs += 1;
        match status {
            "done" => stat.done += 1,
            "blocked" => stat.blocked += 1,
            _ => stat.failed += 1,
        }
        stat.avg_confidence = total_conf / stat.runs as f64;
        stat.updated_at = now_iso();
        self.db.put(STATS, agent_id, &stat)
    }

    pub fn agent_stats(&self, limit: usize) -> Result<Vec<AgentStat>> {
        let mut list: Vec<AgentStat> = self.db.list(STATS)?;
        list.sort_by(|a, b| b.runs.cmp(&a.runs));
        list.truncate(limit);
        Ok(list)
    }

    // ---------------- memory.md 渲染（基础记忆） ----------------

    pub fn render_memory_md(&self, path: impl AsRef<Path>) -> Result<usize> {
        let mut entries: Vec<MemoryEntry> = self.db.list(ENTRIES)?;
        entries.sort_by(|a, b| {
            b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal)
        });
        let pinned: Vec<&MemoryEntry> = entries.iter().filter(|e| e.pinned).collect();
        let important: Vec<&MemoryEntry> = entries.iter().filter(|e| !e.pinned).take(20).collect();

        let mut md = String::new();
        md.push_str("# EXMACHINA 基础记忆\n\n");
        md.push_str("<!-- 本文件由 `exm memory render` 自动生成，是深层记忆库的人读视图；请勿手工编辑 -->\n\n");
        md.push_str(&format!("- 更新时间：{}\n", now_iso()));
        md.push_str(&format!("- 记忆库：`{}`\n", self.root()));
        md.push_str(&format!("- 固定记忆 {} 条；下方另列高重要性记忆 {} 条\n\n", pinned.len(), important.len()));

        md.push_str("## 固定记忆（核心）\n\n");
        if pinned.is_empty() {
            md.push_str("_（暂无；用 `exm memory add --pin` 或 `exm memory pin <id>` 添加）_\n\n");
        }
        for e in &pinned {
            md.push_str(&render_entry_md(e));
        }

        md.push_str("## 近期重要记忆\n\n");
        if important.is_empty() {
            md.push_str("_（暂无）_\n\n");
        }
        for e in &important {
            md.push_str(&render_entry_md(e));
        }

        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, md)?;
        Ok(pinned.len())
    }

    /// 生成注入给指挥体的「历史记忆」提示块
    pub fn render_prompt_block(hits: &[RecallHit]) -> String {
        if hits.is_empty() {
            return String::new();
        }
        let mut s = String::from("## 历史记忆（群体记忆库召回，供参考，不得当事实直接引用）\n");
        for h in hits {
            s.push_str(&format!(
                "- [{}|{:.2}] {}：{}（来源：{}）\n",
                h.entry.kind.key(),
                h.score,
                h.entry.title,
                h.entry.body.chars().take(200).collect::<String>(),
                h.entry.source_ref.clone().unwrap_or_else(|| "本机记忆库".into())
            ));
        }
        s
    }
}

// ---------------------------------------------------------------- 工具函数

fn render_entry_md(e: &MemoryEntry) -> String {
    let tags = if e.tags.is_empty() { String::new() } else { format!(" `{}`", e.tags.join("` `")) };
    format!(
        "- **{}**（{}｜重要性 {:.2}{}）\n  {}\n",
        e.title,
        e.kind.key(),
        e.importance,
        tags,
        e.body.replace('\n', " ")
    )
}

fn content_hash(title: &str, body: &str, group: Option<&str>) -> String {
    let mut h: u64 = 1469598103934665603;
    for b in format!("{}\u{1}{title}\u{1}{body}", group.unwrap_or("*")).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

fn age_days(ts: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds() as f64 / 86400.0)
        .unwrap_or(0.0)
}

/// 词项切分：ASCII 词（≥2）+ CJK 二元组（单字串退化为单字）
pub fn tokens(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut ascii = String::new();
    let mut cjk: Vec<char> = Vec::new();

    fn flush_ascii(buf: &mut String, out: &mut Vec<String>) {
        if buf.chars().count() >= 2 {
            out.push(buf.clone());
        }
        buf.clear();
    }
    fn flush_cjk(buf: &mut Vec<char>, out: &mut Vec<String>) {
        if buf.len() == 1 {
            out.push(buf[0].to_string());
        } else {
            for w in buf.windows(2) {
                out.push(format!("{}{}", w[0], w[1]));
            }
        }
        buf.clear();
    }

    for ch in lower.chars() {
        let is_cjk = ('\u{4e00}'..='\u{9fff}').contains(&ch);
        if is_cjk {
            flush_ascii(&mut ascii, &mut out);
            cjk.push(ch);
        } else if ch.is_alphanumeric() || ch == '_' {
            flush_cjk(&mut cjk, &mut out);
            ascii.push(ch);
        } else {
            flush_ascii(&mut ascii, &mut out);
            flush_cjk(&mut cjk, &mut out);
        }
    }
    flush_ascii(&mut ascii, &mut out);
    flush_cjk(&mut cjk, &mut out);
    out.truncate(200);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 词项切分_中文二元与英文词() {
        let t = tokens("EXMACHINA 记忆系统 memory");
        assert!(t.contains(&"记忆".to_string()));
        assert!(t.contains(&"忆系".to_string()));
        assert!(t.contains(&"memory".to_string()));
    }

    #[test]
    fn 写入去重_检索排序_衰减豁免() {
        let dir = std::env::temp_dir().join(format!("exm-mem-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(&dir).unwrap();

        store
            .remember(
                &MemoryDraft::new(MemoryKind::Preference, "用户偏好", "偏好中文汇报，风格简短")
                    .importance(0.9),
            )
            .unwrap();
        store
            .remember(&MemoryDraft::new(MemoryKind::Digest, "会话摘要", "讨论了无关话题").importance(0.3))
            .unwrap();
        // 去重：同内容再写不新增，重要性提升
        let dup = store
            .remember(&MemoryDraft::new(MemoryKind::Preference, "用户偏好", "偏好中文汇报，风格简短"))
            .unwrap();
        assert!(dup.importance > 0.9, "重复写入应提升重要性");

        let list = store.list(None, 10).unwrap();
        assert_eq!(list.len(), 2, "去重后应只有 2 条");

        let hits = store.recall("汇报风格偏好", 5, None, None).unwrap();
        assert!(!hits.is_empty());
        assert!(
            hits[0].entry.title.contains("偏好"),
            "偏好条目应排第一，实际: {:?}",
            hits[0].entry.title
        );

        // 衰减：固定项豁免
        store.pin(&hits[0].entry.id, true).unwrap();
        let before = store.list(Some(MemoryKind::Preference), 1).unwrap()[0].importance;
        let _ = store.decay(30.0, 0.05).unwrap();
        let after = store.list(Some(MemoryKind::Preference), 1).unwrap()[0].importance;
        assert!((before - after).abs() < 1e-9, "固定记忆不应被衰减");

        // 遗忘
        store.forget(&hits[0].entry.id).unwrap();
        assert_eq!(store.list(None, 10).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 个体统计累加() {
        let dir = std::env::temp_dir().join(format!("exm-stat-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(&dir).unwrap();
        store.record_agent_outcome("context-agent", "done", 0.9).unwrap();
        store.record_agent_outcome("context-agent", "blocked", 0.5).unwrap();
        let stats = store.agent_stats(10).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].runs, 2);
        assert_eq!(stats[0].done, 1);
        assert_eq!(stats[0].blocked, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 个体记忆与群体记忆分层() {
        let dir = std::env::temp_dir().join(format!("exm-layer-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(&dir).unwrap();

        // 个体记忆：归属 context-agent
        let mut draft = MemoryDraft::new(MemoryKind::Lesson, "上下文体教训", "重构配置前先跑全量测试").importance(0.8);
        draft.agent_id = Some("context-agent".into());
        store.remember(&draft).unwrap();

        // 群体记忆：无归属，人人可见
        store
            .remember(&MemoryDraft::new(MemoryKind::Decision, "共享决策", "构建入口统一为 cargo build"))
            .unwrap();

        // 归属者：个体 + 群体都可见，且个体条目带"个体记忆"标记
        let hits = store.recall_for_agent("context-agent", "测试 构建", 10, None).unwrap();
        let owners: Vec<bool> = hits.iter().map(|h| h.entry.agent_id.is_some()).collect();
        assert!(owners.contains(&true), "归属者应看到个体记忆");
        assert!(owners.contains(&false), "归属者也应看到群体记忆");
        let ind = hits.iter().find(|h| h.entry.agent_id.is_some()).unwrap();
        assert!(ind.reasons.iter().any(|r| r == "个体记忆"));

        // 其他个体：只能看到群体记忆，看不到他人私有
        let hits2 = store.recall_for_agent("scout-agent", "测试 构建", 10, None).unwrap();
        assert!(
            hits2.iter().all(|h| h.entry.agent_id.is_none()),
            "其他个体不应看到私有记忆，实际: {:?}",
            hits2.iter().map(|h| h.entry.title.clone()).collect::<Vec<_>>()
        );

        // 群体检索（指挥体视角）：仅群体记忆
        let shared = store.recall("测试 构建", 10, None, None).unwrap();
        assert!(shared.iter().all(|h| h.entry.agent_id.is_none()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 组级记忆隔离与全局条目() {
        let dir = std::env::temp_dir().join(format!("exm-group-{}", uuid::Uuid::new_v4()));
        let store = MemoryStore::open(&dir).unwrap();

        // A 组条目 / B 组条目 / 全局条目（None = 处处可见）
        store
            .remember(&MemoryDraft::new(MemoryKind::Lesson, "翻译组教训", "术语表必须先于正文翻译建立").group("gA"))
            .unwrap();
        store
            .remember(&MemoryDraft::new(MemoryKind::Lesson, "游戏组教训", "数值改动必须附模拟对比").group("gB"))
            .unwrap();
        store
            .remember(&MemoryDraft::new(MemoryKind::Fact, "全局事实", "构建入口统一为 cargo build"))
            .unwrap();

        // A 组视角：自己的 + 全局；看不到 B 组
        let a_hits = store.recall("教训 构建", 10, None, Some("gA")).unwrap();
        let titles: Vec<&str> = a_hits.iter().map(|h| h.entry.title.as_str()).collect();
        assert!(titles.contains(&"翻译组教训"), "A 组应看到本组条目");
        assert!(titles.contains(&"全局事实"), "A 组应看到全局条目");
        assert!(!titles.contains(&"游戏组教训"), "A 组不应看到 B 组条目");

        // B 组视角对称
        let b_hits = store.recall("教训 数值", 10, None, Some("gB")).unwrap();
        assert!(b_hits.iter().all(|h| h.entry.group_id.as_deref() != Some("gA")));

        // 个体检索同样受组约束：同一 identifier 的私有记忆不跨组泄漏
        let mut draft = MemoryDraft::new(MemoryKind::Lesson, "A组私有教训", "该组内教训仅组内可见");
        draft.agent_id = Some("shared-agent".into());
        draft.group_id = Some("gA".into());
        store.remember(&draft).unwrap();
        let in_ga = store.recall_for_agent("shared-agent", "私有教训", 10, Some("gA")).unwrap();
        assert!(in_ga.iter().any(|h| h.entry.title == "A组私有教训"));
        let in_gb = store.recall_for_agent("shared-agent", "私有教训", 10, Some("gB")).unwrap();
        assert!(
            in_gb.iter().all(|h| h.entry.title != "A组私有教训"),
            "组外不应检索到他人组内私有记忆，实际: {:?}",
            in_gb.iter().map(|h| h.entry.title.clone()).collect::<Vec<_>>()
        );

        // 去重哈希按组隔离：同内容跨组各自成条
        let same_title = "同内容条目";
        let same_body = "内容完全一致";
        store.remember(&MemoryDraft::new(MemoryKind::Fact, same_title, same_body).group("gA")).unwrap();
        store.remember(&MemoryDraft::new(MemoryKind::Fact, same_title, same_body).group("gB")).unwrap();
        store.remember(&MemoryDraft::new(MemoryKind::Fact, same_title, same_body)).unwrap();
        let all = store.list(None, 50).unwrap();
        let count = all.iter().filter(|e| e.title == same_title).count();
        assert_eq!(count, 3, "同内容在 gA/gB/全局 应为 3 条独立条目，实际 {count}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
