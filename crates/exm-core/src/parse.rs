//! LLM 输出解析与契约校验 —— 对应 TS 版 zod 校验位置

use crate::types::{OrchestratorPlan, SyncReport};

/// 从文本中提取第一个 JSON 值：优先 ```json 代码块，退化为首尾花括号
pub fn extract_json(text: &str) -> Option<serde_json::Value> {
    let mut candidates: Vec<&str> = Vec::new();

    if let Some(start) = text.find("```") {
        let rest = &text[start + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(end) = rest.find("```") {
            candidates.push(rest[..end].trim());
        }
    }
    if let (Some(s), Some(e)) = (text.find('{'), text.rfind('}')) {
        if e > s {
            candidates.push(&text[s..=e]);
        }
    }

    for c in candidates {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(c) {
            return Some(v);
        }
    }
    None
}

/// 解析并校验 SyncReport（结构 + 业务约束）
pub fn parse_sync_report(text: &str) -> Result<SyncReport, String> {
    let raw = extract_json(text).ok_or_else(|| "未找到 JSON 输出".to_string())?;
    let report: SyncReport =
        serde_json::from_value(raw).map_err(|e| format!("SyncReport 结构校验失败: {e}"))?;
    validate_sync_report(&report)?;
    Ok(report)
}

/// 业务约束校验（对应 ts 版 zod schema 的 min/max/length 约束）
pub fn validate_sync_report(r: &SyncReport) -> Result<(), String> {
    if r.source_agent.trim().is_empty() {
        return Err("sourceAgent 不能为空".into());
    }
    if r.task_node_id.trim().is_empty() {
        return Err("taskNodeId 不能为空".into());
    }
    if r.statements.is_empty() {
        return Err("statements 至少 1 条".into());
    }
    if r.statements.iter().any(|s| s.text.trim().is_empty()) {
        return Err("statement.text 不能为空".into());
    }
    if r.summary.trim().is_empty() {
        return Err("summary 不能为空".into());
    }
    if r.summary.chars().count() > 2000 {
        return Err("summary 超过 2000 字符".into());
    }
    if !(0.0..=1.0).contains(&r.confidence) {
        return Err("confidence 必须落在 0..=1".into());
    }
    if let Some(cs) = &r.conflicts {
        if cs.iter().any(|c| c.parties.len() < 2) {
            return Err("conflict.parties 至少 2 方".into());
        }
    }
    Ok(())
}

pub fn parse_plan(text: &str) -> Result<OrchestratorPlan, String> {
    let raw = extract_json(text).ok_or_else(|| "未找到 JSON 输出".to_string())?;
    let plan: OrchestratorPlan =
        serde_json::from_value(raw).map_err(|e| format!("OrchestratorPlan 结构校验失败: {e}"))?;

    let mut ids = std::collections::HashSet::new();
    for n in &plan.nodes {
        if !ids.insert(n.id.clone()) {
            return Err(format!("节点 id 重复: {}", n.id));
        }
    }
    for n in &plan.nodes {
        for dep in &n.depends_on {
            if !ids.contains(dep) {
                return Err(format!("节点 {} 依赖不存在的 id {}", n.id, dep));
            }
        }
    }

    if matches!(plan.route_level, crate::types::RouteLevel::L0) {
        let ok = plan.final_answer.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        if !ok {
            return Err("L0 直达计划必须给出非空 finalAnswer".into());
        }
    } else if plan.nodes.is_empty() {
        return Err("L1 及以上计划的 nodes 不能为空".into());
    }
    Ok(plan)
}

/// 识别工具调用约定：{"tool": "...", "args": {...}}
pub fn as_tool_call(v: &serde_json::Value) -> Option<(crate::types::ToolName, serde_json::Value)> {
    let obj = v.as_object()?;
    let tool = obj.get("tool")?.as_str()?;
    let name = crate::types::ToolName::parse(tool)?;
    let args = obj.get("args").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
    Some((name, args))
}
