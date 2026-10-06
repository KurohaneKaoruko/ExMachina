//! 协议契约类型 —— 对应 docs/04。
//! JSON 字段名一律 camelCase，与 WebUI/CLI/数据库 payload 保持一致。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- 枚举

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeechTag {
    #[serde(rename = "肯定")]
    肯定,
    #[serde(rename = "否定")]
    否定,
    #[serde(rename = "疑问")]
    疑问,
    #[serde(rename = "报告")]
    报告,
    #[serde(rename = "提案")]
    提案,
    #[serde(rename = "警告")]
    警告,
    #[serde(rename = "要求")]
    要求,
    #[serde(rename = "观测")]
    观测,
}

impl SpeechTag {
    pub fn as_str(self) -> &'static str {
        match self {
            SpeechTag::肯定 => "肯定",
            SpeechTag::否定 => "否定",
            SpeechTag::疑问 => "疑问",
            SpeechTag::报告 => "报告",
            SpeechTag::提案 => "提案",
            SpeechTag::警告 => "警告",
            SpeechTag::要求 => "要求",
            SpeechTag::观测 => "观测",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceLevel {
    A,
    B,
    C,
    D,
}

impl EvidenceLevel {
    pub fn label(self) -> &'static str {
        match self {
            EvidenceLevel::A => "A",
            EvidenceLevel::B => "B",
            EvidenceLevel::C => "C",
            EvidenceLevel::D => "D",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Coordination,
    Research,
    Architecture,
    Implementation,
    Verification,
    Rationality,
    Documentation,
    Integration,
    Operations,
    Security,
    Common,
}

impl Domain {
    pub fn label(self) -> &'static str {
        match self {
            Domain::Coordination => "协调域",
            Domain::Research => "研究域",
            Domain::Architecture => "架构域",
            Domain::Implementation => "实作域",
            Domain::Verification => "校验域",
            Domain::Rationality => "理性域",
            Domain::Documentation => "文档域",
            Domain::Integration => "集成域",
            Domain::Operations => "运维域",
            Domain::Security => "安全域",
            Domain::Common => "公共",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Orchestrator,
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    Read,
    Filesystem,
    Terminal,
    /// 精确编辑：old_string → new_string（唯一命中校验，避免整文件覆盖）
    Edit,
    /// 统一 diff 补丁：多文件原子应用（任一文件校验失败整体拒绝）
    Patch,
    /// 仓库检索：按正则搜索文件内容（尊重 .gitignore）
    Grep,
    /// 按 glob 模式列出文件
    Glob,
    #[serde(rename = "web_search")]
    WebSearch,
    /// 抓取网页并提取正文文本
    #[serde(rename = "web_fetch")]
    WebFetch,
    /// 智能体管理（仅组内主智能体可用：创建/修改/删除子个体、设置主智能体）
    #[serde(rename = "agent_manage")]
    AgentManage,
    /// 自主排程：创建/查看/删除定时与一次性任务（AI 自驱工作）
    Schedule,
    /// 浏览器自动化：headless Chrome/Chromium + CDP（导航 / 取正文 / 执行 JS / 截图）
    Browser,
    /// Computer Use：截屏 + 鼠标 / 键盘控制本机桌面（配置开启后下发，输入类动作过审批闸门）
    Computer,
    /// 记忆检索：按查询返回组内命中条目（含其个体私有层）
    #[serde(rename = "memory_read")]
    MemoryRead,
    /// 记忆写入：内容哈希去重，来源标注智能体自写（source=agent）
    #[serde(rename = "memory_write")]
    MemoryWrite,
    /// 记忆关联与主题列示：建立条目关联 / 列示主题下条目及关联链
    #[serde(rename = "memory_link")]
    MemoryLink,
}

impl ToolName {
    pub fn key(self) -> &'static str {
        match self {
            ToolName::Read => "read",
            ToolName::Filesystem => "filesystem",
            ToolName::Terminal => "terminal",
            ToolName::Edit => "edit",
            ToolName::Patch => "patch",
            ToolName::Grep => "grep",
            ToolName::Glob => "glob",
            ToolName::WebSearch => "web_search",
            ToolName::WebFetch => "web_fetch",
            ToolName::AgentManage => "agent_manage",
            ToolName::Schedule => "schedule",
            ToolName::Browser => "browser",
            ToolName::Computer => "computer",
            ToolName::MemoryRead => "memory_read",
            ToolName::MemoryWrite => "memory_write",
            ToolName::MemoryLink => "memory_link",
        }
    }

    /// 只读工具（无副作用）：同轮可并发执行（memory_link 混合读/写语义，按写工具串行口径处理）
    pub fn is_readonly(self) -> bool {
        matches!(
            self,
            ToolName::Read
                | ToolName::Grep
                | ToolName::Glob
                | ToolName::WebSearch
                | ToolName::WebFetch
                | ToolName::MemoryRead
        )
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(ToolName::Read),
            "filesystem" => Some(ToolName::Filesystem),
            "terminal" => Some(ToolName::Terminal),
            "edit" => Some(ToolName::Edit),
            "patch" => Some(ToolName::Patch),
            "grep" => Some(ToolName::Grep),
            "glob" => Some(ToolName::Glob),
            "web_search" => Some(ToolName::WebSearch),
            "web_fetch" => Some(ToolName::WebFetch),
            "agent_manage" => Some(ToolName::AgentManage),
            "schedule" => Some(ToolName::Schedule),
            "browser" => Some(ToolName::Browser),
            "computer" => Some(ToolName::Computer),
            "memory_read" => Some(ToolName::MemoryRead),
            "memory_write" => Some(ToolName::MemoryWrite),
            "memory_link" => Some(ToolName::MemoryLink),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Ready,
    Dispatched,
    Running,
    Syncing,
    Done,
    Blocked,
    Failed,
    Arbitrating,
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Done | TaskStatus::Blocked | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Ready => "ready",
            TaskStatus::Dispatched => "dispatched",
            TaskStatus::Running => "running",
            TaskStatus::Syncing => "syncing",
            TaskStatus::Done => "done",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Failed => "failed",
            TaskStatus::Arbitrating => "arbitrating",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::P0 => "P0",
            Priority::P1 => "P1",
            Priority::P2 => "P2",
            Priority::P3 => "P3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    InProgress,
    Done,
    Blocked,
    NeedArbitration,
}

impl SyncStatus {
    pub fn label(self) -> &'static str {
        match self {
            SyncStatus::InProgress => "in_progress",
            SyncStatus::Done => "done",
            SyncStatus::Blocked => "blocked",
            SyncStatus::NeedArbitration => "need_arbitration",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphStatus {
    Planning,
    Executing,
    Converged,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Orchestrator,
    Unit,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Code,
    Config,
    Log,
    Test,
    Command,
    Doc,
    #[serde(rename = "userInput")]
    UserInput,
    Reasoning,
}

impl EvidenceKind {
    pub fn label(self) -> &'static str {
        match self {
            EvidenceKind::Code => "code",
            EvidenceKind::Config => "config",
            EvidenceKind::Log => "log",
            EvidenceKind::Test => "test",
            EvidenceKind::Command => "command",
            EvidenceKind::Doc => "doc",
            EvidenceKind::UserInput => "userInput",
            EvidenceKind::Reasoning => "reasoning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskSeverity {
    Low,
    Mid,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RouteLevel {
    L0,
    L1,
    L2,
    L3,
}

// ---------------------------------------------------------------- 智能体组与个体定义

/// 组级能力模型覆盖（"档案ID" 或 "档案ID/模型名"）：每个组可用不同的模型栈；
/// 任一项缺省 = 跟随「模型设置」页的全局槽位
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupCapabilities {
    /// 语音合成（TTS）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech: Option<String>,
    /// 语音识别 / 语音转述
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcribe: Option<String>,
    /// 视觉转述
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_relay: Option<String>,
    /// 嵌入（组记忆语义检索）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<String>,
}

/// 智能体组元数据：组是隔离与切换的基本单位（docs/09）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMeta {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 组内主智能体：直接对接用户、拥有组内最大权限（可增删改其他智能体）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    /// 组工作区：该组个体的工具操作根目录（相对 = 全局工作区下；缺省 = 全局工作区）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// 组默认模型（"档案ID" 或 "档案ID/模型名"；缺省 = 跟随全局生效档案；
    /// 组内未显式指定模型的个体随组）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 组级能力模型覆盖：每组可用不同模型栈；缺省字段跟随全局槽位
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<GroupCapabilities>,
    /// 内置组（默认智械体集群）：定义受保护，不可增删个体
    #[serde(default)]
    pub builtin: bool,
    pub created_at: String,
}

fn empty_io_schema() -> AgentIoSchema {
    AgentIoSchema { required: vec![], optional: vec![] }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentIoSchema {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub name: String,
    pub identifier: String,
    /// 领域标签（自由文本；默认组装载时将旧枚举值规范化为中文标签）
    pub domain: String,
    pub tier: Tier,
    pub description: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub tools: Vec<ToolName>,
    #[serde(default)]
    pub when_to_call: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub composable_with: Vec<String>,
    #[serde(default = "empty_io_schema")]
    pub input_schema: AgentIoSchema,
    #[serde(default = "empty_io_schema")]
    pub output_schema: AgentIoSchema,
    #[serde(default)]
    pub prompt_file: String,
    #[serde(default)]
    pub model_hint: Option<String>,
    /// 连结体代理声明（智能连结网络，实验性）：该个体是外部连结体的代理——
    /// 派发到此个体的任务节点转交连结体执行，其回报作为本节点回流。
    /// 值 = nexus.links 中的连结体 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

// ---------------------------------------------------------------- 消息与三账

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statement {
    pub tag: SpeechTag,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_level: Option<EvidenceLevel>,
}

impl Statement {
    pub fn new(tag: SpeechTag, text: impl Into<String>) -> Self {
        Statement { tag, text: text.into(), evidence_level: None }
    }
    pub fn report(text: impl Into<String>) -> Self {
        Statement::new(SpeechTag::报告, text)
    }
    pub fn warn(text: impl Into<String>) -> Self {
        Statement::new(SpeechTag::警告, text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerTask {
    pub goal: String,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub forbidden: Vec<String>,
    #[serde(default)]
    pub priorities: Vec<String>,
}

impl Default for LedgerTask {
    fn default() -> Self {
        LedgerTask {
            goal: String::new(),
            acceptance: vec![],
            constraints: vec![],
            forbidden: vec![],
            priorities: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEvidenceItem {
    pub text: String,
    pub level: EvidenceLevel,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEvidence {
    #[serde(default)]
    pub confirmed: Vec<LedgerEvidenceItem>,
    #[serde(default)]
    pub gaps: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerRisk {
    #[serde(default)]
    pub open_assertions: Vec<String>,
    #[serde(default)]
    pub impact: Vec<String>,
    #[serde(default)]
    pub revert_paths: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionLedger {
    pub task: LedgerTask,
    pub evidence: LedgerEvidence,
    pub risk: LedgerRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub status: String,
    /// 会话归属的智能体组（组是交互对象：切组即切换会话上下文；旧数据归入 default）
    #[serde(default = "default_session_group")]
    pub group_id: String,
    /// 滚动摘要：超出窗口的历史经 LLM 压缩后的连续上下文（多轮对话记忆）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolling_summary: Option<String>,
    /// 摘要已覆盖到的消息条数（增量压缩从此续起）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_upto: Option<usize>,
    /// 派生来源（fork 分支）：原会话 id 与截至轮次；非派生会话为空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_upto: Option<usize>,
    pub ledger: SessionLedger,
    pub created_at: String,
    pub updated_at: String,
}

fn default_session_group() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub statements: Vec<Statement>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
    /// 本轮思维链（收束时由服务端留存，过程收起栏数据源）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// 本轮工具调用轨迹（收束时由服务端留存）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<crate::round_trace::ToolCallRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt: u64,
    pub completion: u64,
}

// ---------------------------------------------------------------- 任务 DAG

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNode {
    pub id: String,
    pub session_id: String,
    pub graph_id: String,
    pub agent_identifier: String,
    pub title: String,
    pub objective: String,
    pub acceptance: Vec<String>,
    pub priority: Priority,
    pub depends_on: Vec<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub input_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_report_id: Option<String>,
    pub retry_count: u32,
    pub idempotency_key: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGraph {
    pub id: String,
    pub session_id: String,
    pub nodes: Vec<TaskNode>,
    pub status: GraphStatus,
    pub created_at: String,
}

// ---------------------------------------------------------------- 调度与回流

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchBoundary {
    pub in_scope: Vec<String>,
    pub forbidden: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchInput {
    pub ref_id: String,
    pub kind: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchConstraints {
    pub max_steps: u32,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchOrder {
    pub task_node_id: String,
    pub objective: String,
    pub acceptance: Vec<String>,
    pub boundary: DispatchBoundary,
    pub inputs: Vec<DispatchInput>,
    pub tool_allowlist: Vec<ToolName>,
    pub constraints: DispatchConstraints,
    pub report_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub level: EvidenceLevel,
    pub kind: EvidenceKind,
    #[serde(rename = "ref")]
    pub reference: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskItem {
    pub text: String,
    pub severity: RiskSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revert_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerItem {
    pub reason: String,
    pub unblock_condition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictItem {
    pub parties: Vec<String>,
    pub point: String,
    pub levels: std::collections::BTreeMap<String, EvidenceLevel>,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendNodeProposal {
    pub title: String,
    pub agent_identifier: String,
    pub objective: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextSuggestion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_nodes: Option<Vec<AppendNodeProposal>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub source_agent: String,
    pub task_node_id: String,
    pub status: SyncStatus,
    pub statements: Vec<Statement>,
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceItem>,
    #[serde(default)]
    pub risks: Vec<RiskItem>,
    #[serde(default)]
    pub blockers: Vec<BlockerItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<ConflictItem>>,
    #[serde(default)]
    pub next_suggestion: NextSuggestion,
    pub confidence: f64,
}

// ---------------------------------------------------------------- 指挥体计划

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    pub id: String,
    pub title: String,
    pub agent_identifier: String,
    pub objective: String,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub priority: Priority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorPlan {
    pub route_level: RouteLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playbook: Option<String>,
    pub boundary: DispatchBoundary,
    pub goal: String,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub nodes: Vec<PlanNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_answer: Option<Vec<Statement>>,
}

// ---------------------------------------------------------------- Playbook

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybookStep {
    pub key: String,
    pub agent_identifier: String,
    pub title: String,
    pub objective_template: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub parallel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playbook {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub trigger_signals: Vec<String>,
    pub description: String,
    pub steps: Vec<PlaybookStep>,
    pub terminal: String,
}

// ---------------------------------------------------------------- 全连结消息与事件

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub id: String,
    pub topic: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_node_id: Option<String>,
    pub payload: serde_json::Value,
    pub trace_id: String,
    pub created_at: String,
}

impl Envelope {
    pub fn new(
        topic: impl Into<String>,
        from: impl Into<String>,
        to: impl Into<String>,
        kind: impl Into<String>,
        session_id: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Envelope {
            id: uuid::Uuid::new_v4().to_string(),
            topic: topic.into(),
            from: from.into(),
            to: to.into(),
            kind: kind.into(),
            session_id: session_id.into(),
            task_node_id: None,
            payload,
            trace_id: uuid::Uuid::new_v4().to_string(),
            created_at: now_iso(),
        }
    }
}

/// 推送给渠道（WS/CLI）的核心事件
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub session_id: String,
    pub payload: serde_json::Value,
}

/// 领域标签规范化：默认组旧枚举值 → 中文标签（自定义组可用任意文本）
pub fn normalize_domain(raw: &str) -> String {
    match raw {
        "coordination" => "协调域".into(),
        "research" => "研究域".into(),
        "architecture" => "架构域".into(),
        "implementation" => "实作域".into(),
        "verification" => "校验域".into(),
        "rationality" => "理性域".into(),
        "documentation" => "文档域".into(),
        "integration" => "集成域".into(),
        "operations" => "运维域".into(),
        "security" => "安全域".into(),
        "common" => "公共".into(),
        other => other.to_string(),
    }
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Unix 毫秒时间戳（配对码过期等数值比较口径）
pub fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------- 技能包（数据化技能，docs/10）

/// 技能包：任务目标命中触发词时，把指令注入相关个体的派发指令。
/// 技能是数据（`agents/skills/` 或自定义组 `skills/`），热装载，扩编不写代码。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 触发词：目标/节点标题包含任一词即注入
    #[serde(default)]
    pub triggers: Vec<String>,
    /// 随派发下发的行为约束与作业知识
    pub instructions: String,
    /// 适用的个体 identifier；空 = 全体适用
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub created_at: String,
}

impl SkillDef {
    pub fn applies_to(&self, agent: &str) -> bool {
        self.agents.is_empty() || self.agents.iter().any(|a| a == agent)
    }

    pub fn matches(&self, text: &str) -> bool {
        self.triggers.iter().any(|t| !t.trim().is_empty() && text.contains(t.trim()))
    }
}

// ---------------------------------------------------------------- 自动化（cron/心跳，docs/10）

/// 定时任务：五段 cron 表达式或一次性 `at`（ISO8601）；到期由网关调度器唤醒智能体执行
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// 五段 cron：分 时 日 月 周（`*` 数字 `a-b` `a,b` `c-d/n` `*/n`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// 一次性触发时间（ISO8601）；触发后自动停用
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// 执行组；None = 运行时激活组
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// 会话标题（复用同名会话；缺省 job-<id>）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    /// 上次运行的分钟戳（同一分钟只跑一次的去重键）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_minute: Option<String>,
    /// 结果推送订阅的通道 id 清单（空 = 不推送，维持现状：仅落会话与运行日志）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notify_channels: Vec<String>,
    #[serde(default)]
    pub created_at: String,
}

fn default_true() -> bool {
    true
}

/// 定时任务的一次运行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronRun {
    pub id: String,
    pub job_id: String,
    pub job_name: String,
    pub session_id: String,
    pub started_at: String,
    pub finished_at: String,
    /// done | failed
    pub status: String,
    pub summary: String,
}

// ---------------------------------------------------------------- 执行审批（docs/10）

/// 终端命令审批单：命令被闸门拦截后挂起，等待用户批准后由系统代执行
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub agent_id: String,
    pub command: String,
    /// pending | approved | denied | executed | failed
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    /// 发起会话的外部用户（通道来源会话才有；控制台/智能体发起为空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_user: Option<String>,
    /// member 发起的审批须管理员在控制台批准（升级语义标注）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_role: Option<String>,
}

// ---------------------------------------------------------------- 通道用户身份（配对绑定）

/// 配对码：控制台签发的一次性短码，外部用户在通道内 `/pair <码>` 完成绑定
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    /// 短码本身即主键（8 位随机码）
    pub code: String,
    /// 签发备注
    #[serde(default)]
    pub note: String,
    /// 目标平台（空 = 通用）；限定后其他平台提交无效
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_platform: Option<String>,
    pub created_by: String,
    pub created_at: String,
    /// 过期时间（Unix 毫秒）
    pub expires_at_ms: u64,
}

/// 绑定的外部用户身份：通道账号 → 平台用户档案（配对码换发的长期绑定）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserIdentity {
    /// `{channel}:{externalId}`
    pub id: String,
    /// 所属通道平台（telegram/discord/slack/...）
    pub channel: String,
    /// 通道侧账号标识（chat/user id）
    pub external_id: String,
    /// 通道侧展示名
    pub display_name: String,
    /// admin | member
    pub role: String,
    pub paired_at: String,
    /// 签发配对码时的备注
    #[serde(default)]
    pub note: String,
}

// ---------------------------------------------------------------- 事件触发器（文件监听 / webhook 事件源）

/// 事件触发器：匹配的事件经去抖后注入目标组会话（复用 cron 的注入通路）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTrigger {
    pub id: String,
    /// file_watch | event_webhook
    pub kind: String,
    pub name: String,
    /// 匹配模式：file_watch = 相对工作区的 glob；event_webhook = 事件类型（* = 全部）
    pub pattern: String,
    /// 目标组；空 = 运行时激活组
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// 注入提示词模板（`{event}` 占位替换为事件摘要）
    pub prompt: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<String>,
}

// ---------------------------------------------------------------- 会话轮次快照（undo / 编辑重发 / 分支基座）

/// 会话轮次快照：每轮次结束记录消息游标与检查点链，支撑历史回退类操作
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshot {
    /// `{sessionId}:{turn}`
    pub id: String,
    pub session_id: String,
    /// 轮次序号（从 1 递增）
    pub turn: usize,
    /// 快照时的消息条数（消息游标：撤销即截断到该计数）
    pub message_count: usize,
    /// 本轮写类工具产生的检查点 ID（联动回滚文件变更）
    #[serde(default)]
    pub checkpoint_ids: Vec<String>,
    /// 本轮真实 token 用量（配额与台账口径）
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    /// done | failed
    pub status: String,
    pub created_at: String,
}

// ---------------------------------------------------------------- 经验优化（子个体自适应，docs/10 §5）

/// 子个体的经验改进要点：由历史教训与可靠性统计提炼，常驻注入该个体的每次派发。
/// 只调优既有个体的作业行为，不创建个体；版本化保存，可查看历史与重置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdaptation {
    pub identifier: String,
    /// 行为要点（"- " 开头的列表文本，最多 6 条）
    pub content: String,
    pub revision: u32,
    /// 已计入提炼的教训条数（自动触发的增量阈值依据）
    pub lessons_seen: u32,
    pub updated_at: String,
    /// 生成依据的统计快照（runs/done/blocked/failed/avgConfidence）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<serde_json::Value>,
    /// 历史版本（最多保留 5 个）
    #[serde(default)]
    pub previous: Vec<AgentAdaptationVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdaptationVersion {
    pub revision: u32,
    pub content: String,
    pub updated_at: String,
}
