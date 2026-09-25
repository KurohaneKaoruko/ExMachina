//! 核心配置 —— 环境变量与 `.exmachina/config.json` 的唯一读取入口
//!
//! 可扩展性设计（避免后期重构）：
//!   1. `configVersion` 字段 + 版本迁移函数：新增字段只需追加默认值 + 迁移分支；
//!   2. `config_schema()` 输出字段描述（分组/类型/默认值/校验），
//!      CLI 安装向导与 WebUI 设置页均由该 schema 驱动渲染——新增配置项零 UI 改动；
//!   3. 安装清单 `install.json` 记录安装剖面与渠道开关，与运行配置解耦。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 配置结构版本（新增字段时 +1，并在 `migrate_file` 中补默认值）
pub const CONFIG_VERSION: u32 = 1;

/// 提供商下的模型条目：能力开关让平台知道该模型支持哪些多模态输入。
/// 模型未列入清单（或档案无清单）视为能力未知，保持直通行为。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    /// 模型名（API 的 model 参数）
    pub model: String,
    /// 支持视觉（图片输入）
    #[serde(default)]
    pub vision: bool,
    /// 支持音频（语音输入）
    #[serde(default)]
    pub audio: bool,
}

/// 厂商模型档案：一条 OpenAI 兼容端点（OpenAI/DeepSeek/通义/Kimi/智谱/Ollama/vLLM …）。
/// 多档案可并存，`active_profile` 决定当前生效者；切换即热生效（apply_config 重建运行时）。
/// 档案描述「端点 + 密钥 + 模型清单」；指挥体 / 子个体不在此区分——编成本身已按角色分配模型。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LlmProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// 该提供商的多把 API Key：按发起方粘性负载均衡，仅限额时切换（docs/11 §1）
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// API 协议：openai（默认）| anthropic
    #[serde(default)]
    pub api_format: String,
    /// 该提供商的默认模型名（模型清单里勾「默认」的那一个）
    #[serde(default)]
    pub model: String,
    /// 模型清单（含视觉/语音能力开关）；空 = 未标记（能力未知，输入直通）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelEntry>,
    /// 失败回退：下一个档案 id（请求失败且未发出内容时切换；成环自动截断）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    /// 生效 Key 池（粘性负载均衡）；空 = 仅用 api_key 单键
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// 生效协议：openai（默认）| anthropic
    #[serde(default)]
    pub api_format: String,
    pub model: String,
}

/// 安全配置：终端命令审批闸门（docs/10）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityConfig {
    /// off | risky | always
    pub exec_approval: String,
    /// 命令前缀白名单（命中前缀即跳过审批闸门）
    pub exec_allowlist: Vec<String>,
    /// 后台访问密钥：非空时 /api/* 全部要求鉴权（env EXM_AUTH_KEY 最高优先）
    pub auth_key: String,
    /// 终端命令超时（秒）；后台任务不受此限
    pub terminal_timeout_secs: u32,
    /// 工具结果超过该字符数时落盘并回填路径（0 = 不落盘，仅截断）
    pub tool_output_spill_chars: usize,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        SecurityConfig {
            exec_approval: "off".into(),
            exec_allowlist: vec![],
            auth_key: String::new(),
            terminal_timeout_secs: 120,
            tool_output_spill_chars: 12000,
        }
    }
}

/// 联网搜索配置（web_search 工具的真实后端）。
/// **未配置时不向模型下发该工具** —— 不承诺不存在的能力。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    /// 后端：tavily | brave | searxng | custom；空 = 未配置
    pub provider: String,
    /// 端点（brave/custom 必填；tavily 留空用官方端点）
    pub endpoint: String,
    pub api_key: String,
    /// 返回条数上限
    pub max_results: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig { provider: String::new(), endpoint: String::new(), api_key: String::new(), max_results: 5 }
    }
}

impl SearchConfig {
    /// 是否已具备可用配置（provider 与所需凭据齐全）
    pub fn ready(&self) -> bool {
        match self.provider.trim().to_lowercase().as_str() {
            "tavily" => !self.api_key.trim().is_empty(),
            "brave" => !self.api_key.trim().is_empty(),
            "searxng" => !self.endpoint.trim().is_empty(),
            "custom" => !self.endpoint.trim().is_empty(),
            _ => false,
        }
    }
}

/// 生命周期钩子：可执行命令列表；占位符/上下文经环境变量注入。
/// preTool 非零退出 = **拦截**该次工具调用（返回拦截原因给模型）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HooksConfig {
    #[serde(default)]
    pub pre_tool: Vec<String>,
    #[serde(default)]
    pub post_tool: Vec<String>,
    /// 一轮 run 收束（或失败）时触发
    #[serde(default)]
    pub on_run_end: Vec<String>,
}

impl HooksConfig {
    pub fn is_empty(&self) -> bool {
        self.pre_tool.is_empty() && self.post_tool.is_empty() && self.on_run_end.is_empty()
    }
}

/// 沙箱执行配置：让 AI 在不扩大爆炸半径的前提下自行跑命令。
/// - off：仅白名单 + 审批（历史行为）
/// - workspace：**环境净化**（白名单环境变量 + HOME/TMP 重定向到 .exmachina/sandbox）
/// - strict：净化 + 联网类命令一律走审批 + 更短超时；Linux/macOS 检测到 bubblewrap 时自动做系统级隔离
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxConfig {
    /// off | workspace | strict
    pub mode: String,
    /// strict 模式下是否允许安装/下载类联网命令（false = 需审批）
    pub allow_network: bool,
    /// Linux/macOS：检测到 bwrap 时用它做文件系统隔离（工作区可写、根只读、默认断网）
    pub use_bwrap: bool,
    /// 子进程内存上限（MB；Linux 经 ulimit、Windows 经作业对象生效，0 = 不限）
    pub memory_mb: usize,
    /// 子进程数上限（Windows 作业对象生效，0 = 不限）
    #[serde(default)]
    pub max_processes: usize,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            mode: "workspace".into(),
            allow_network: false,
            use_bwrap: true,
            memory_mb: 0,
            max_processes: 0,
        }
    }
}

impl SandboxConfig {
    pub fn enabled(&self) -> bool {
        !self.mode.trim().eq_ignore_ascii_case("off")
    }
    pub fn strict(&self) -> bool {
        self.mode.trim().eq_ignore_ascii_case("strict")
    }
}

/// 浏览器自动化配置（browser 工具：headless Chrome/Chromium + CDP）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserConfig {
    /// 浏览器可执行文件路径；空 = 自动探测常见安装位置
    pub executable: String,
    /// 是否 headless（默认 true）
    pub headless: bool,
    /// 单次操作超时（秒）
    pub timeout_secs: u64,
    /// 单页正文提取上限（字符）
    pub max_chars: usize,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig {
            executable: String::new(),
            headless: true,
            timeout_secs: 30,
            max_chars: 8000,
        }
    }
}

/// 声明式自定义工具：把**外部 HTTP 接口**或**本机命令模板**包装成模型可调用的工具，
/// 免去为每个集成单写一个 MCP 服务器。名字与内置工具同命名空间（同名时内置工具优先）。
///
/// 例（HTTP GET，参数以 `{参数名}` 占位）：
/// `{ "name": "weather", "description": "查天气", "kind": "http", "method": "GET",
///    "target": "https://api.example.com/now?city={city}",
///    "parameters": { "type": "object", "properties": { "city": { "type": "string" } },
///                    "required": ["city"] },
///    "headers": { "Authorization": "Bearer …" }, "agents": [] }`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CustomTool {
    /// 工具名（模型可见；字母开头，仅字母/数字/下划线/连字符）
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 参数 JSON Schema（对象）；缺省 = 无参数
    #[serde(default)]
    pub parameters: serde_json::Value,
    /// http（默认）| shell
    #[serde(default)]
    pub kind: String,
    /// http：URL（`{参数名}` 占位 → URL 编码值）；shell：命令模板（占位 → 安全转义值）
    #[serde(default)]
    pub target: String,
    /// http：GET | POST（默认 POST，全部实参作为 JSON body）
    #[serde(default)]
    pub method: String,
    /// http：附加请求头（BTreeMap 保证序列化顺序稳定）
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    /// 可见个体（identifier 列表）；空 = 全部个体可用
    #[serde(default)]
    pub agents: Vec<String>,
}

impl CustomTool {
    /// 名字合法（与内置工具命名风格一致，避免与 MCP 的 `mcp:` 前缀冲突）
    pub fn valid(&self) -> bool {
        let n = self.name.trim();
        !n.is_empty()
            && n.len() <= 48
            && !n.contains(':')
            && n.starts_with(|c: char| c.is_ascii_alphabetic())
            && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    /// 归一化类型：http（默认）| shell
    pub fn kind_norm(&self) -> String {
        let k = self.kind.trim().to_ascii_lowercase();
        if k == "shell" {
            "shell".into()
        } else {
            "http".into()
        }
    }

    /// 该个体是否可见（agents 为空 = 全部可见）
    pub fn visible_to(&self, agent_id: &str) -> bool {
        self.agents.is_empty() || self.agents.iter().any(|a| a == agent_id)
    }

    /// 参数 schema（缺省给空对象 schema，满足 function calling 契约）
    pub fn param_schema(&self) -> serde_json::Value {
        if self.parameters.is_null() {
            serde_json::json!({ "type": "object", "properties": {} })
        } else {
            self.parameters.clone()
        }
    }
}

/// 工具面配置：目前只有声明式自定义工具列表
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolsConfig {
    #[serde(default)]
    pub custom: Vec<CustomTool>,
}

/// 自动化配置：心跳巡检（docs/10）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationConfig {
    pub heartbeat_enabled: bool,
    pub heartbeat_interval_minutes: u32,
    pub heartbeat_prompt: String,
    /// 新教训达阈值时自动优化相关子个体（经验改进要点）
    pub auto_adapt: bool,
    /// 子个体单次派发的最大工具步数（主流的 20-50 步为长任务口径，取中档）
    pub unit_max_steps: usize,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        AutomationConfig {
            heartbeat_enabled: false,
            heartbeat_interval_minutes: 30,
            heartbeat_prompt: "系统心跳巡检：检查未决任务、受阻节点与风险账，无事项则简短报告正常。".into(),
            auto_adapt: true,
            unit_max_steps: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExmConfig {
    pub config_version: u32,
    pub workspace_root: PathBuf,
    /// 数据根目录（会话/任务/证据/事件/记忆的文档存储）
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
    pub memory_md_path: PathBuf,
    pub agents_dir: PathBuf,
    /// WebUI 构建产物目录（Gateway 静态托管；不存在则仅提供 API）
    pub webui_dist: PathBuf,
    pub llm: LlmConfig,
    pub max_concurrency: usize,
    /// 会话 token 预算（估算：字符/4；0 = 不限；超出后拒绝新轮次）
    pub max_session_tokens: u64,
    /// memory.md 字数上限（超限触发 AI 自主压缩，旧内容归档）
    pub memory_md_max_chars: usize,
    /// 记忆系统开关与参数
    pub memory_enabled: bool,
    pub memory_recall_limit: usize,
    pub memory_half_life_days: f64,
    /// 语义检索目标："档案ID" 或 "档案ID/模型名"；空 = 仅词项召回
    pub memory_semantic_model: String,
    /// 能力模型槽位（"档案ID" 或 "档案ID/模型名"）：
    /// 语音合成（TTS）；空 = 全局档案默认 tts-1
    pub speech_model: String,
    /// 语音识别 / 语音转述（STT）；空 = 全局档案默认 whisper-1
    pub stt_model: String,
    /// 视觉转述：生效模型未标记视觉能力时，用它把图片转成文字描述；空 = 不转述
    pub vision_relay_model: String,
    /// 测试替身通道（仅 `EXM_LLM_MOCK=1` 或测试代码置位；产品运行时不生效）
    pub use_mock: bool,
    /// 执行审批（安全闸门）
    pub security: SecurityConfig,
    /// 自动化（心跳巡检）
    pub automation: AutomationConfig,
    /// 联网搜索后端（web_search 工具；未配置则不下发该工具）
    pub search: SearchConfig,
    /// 生命周期钩子（pre/post tool、run 收束）
    pub hooks: HooksConfig,
    /// 工具面（声明式自定义工具）
    pub tools: ToolsConfig,
    /// 沙箱执行（子进程环境净化 / bubblewrap / strict 闸门）
    pub sandbox: SandboxConfig,
    /// 浏览器自动化（browser 工具）
    pub browser: BrowserConfig,
    /// 厂商模型档案与当前生效档案（llm 字段 = 生效档案的解析结果）
    pub llm_profiles: Vec<LlmProfile>,
    pub active_profile: String,
    /// 界面与提示词语言（EXM_LANG，zh/en）：en 时优先装载 {stem}.{lang}.md 提示词变体
    pub language: String,
    /// MCP 服务器（第三方工具生态，docs/架构与设计.md 扩展点）
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    #[serde(default)]
    config_version: Option<u32>,
    #[serde(default)]
    llm: Option<LlmConfigFile>,
    #[serde(default)]
    max_concurrency: Option<usize>,
    #[serde(default)]
    max_session_tokens: Option<u64>,
    #[serde(default)]
    memory_md_max_chars: Option<usize>,
    #[serde(default)]
    memory: Option<MemoryConfigFile>,
    #[serde(default)]
    security: Option<SecurityFile>,
    #[serde(default)]
    automation: Option<AutomationFile>,
    #[serde(default)]
    llm_profiles: Option<Vec<LlmProfileFile>>,
    #[serde(default)]
    active_profile: Option<String>,
    #[serde(default)]
    capabilities: Option<CapabilitiesFile>,
    #[serde(default)]
    search: Option<SearchConfig>,
    #[serde(default)]
    hooks: Option<HooksConfig>,
    #[serde(default)]
    sandbox: Option<SandboxConfig>,
    #[serde(default)]
    browser: Option<BrowserConfig>,
    #[serde(default)]
    tools: Option<ToolsConfig>,
    #[serde(default)]
    mcp_servers: Option<Vec<crate::mcp::McpServerConfig>>,
}

/// 能力模型槽位（模型设置页配置）：语音合成 / 语音识别 / 视觉转述。
/// 嵌入（语义检索）沿用 memory.semanticModel，不在此重复。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CapabilitiesFile {
    #[serde(default)]
    speech: Option<String>,
    #[serde(default)]
    transcribe: Option<String>,
    #[serde(default)]
    vision_relay: Option<String>,
}

/// 档案的读取形态：兼容历史上区分「指挥体模型 / 子个体模型」的旧配置。
/// 两者合并为单一 `model`（优先取指挥体模型，其次子个体模型）；旧键解析后不再回写。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LlmProfileFile {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    api_keys: Vec<String>,
    #[serde(default)]
    api_format: String,
    /// 现役：该提供商的默认模型名
    #[serde(default)]
    model: String,
    /// 历史遗留：指挥体模型（迁移来源）
    #[serde(default)]
    orch_model: Option<String>,
    /// 历史遗留：子个体模型（迁移兜底）
    #[serde(default)]
    unit_model: Option<String>,
    #[serde(default)]
    fallback: Option<String>,
    /// 模型清单（模型名 + 视觉/语音能力开关）
    #[serde(default)]
    models: Option<Vec<ModelEntry>>,
    /// 历史遗留：档案级嵌入模型（现由「记忆」页的语义检索设置管理）
    #[serde(default)]
    embed_model: Option<String>,
}

impl LlmProfileFile {
    /// 补默认值并完成历史字段迁移
    fn into_profile(self, fallback_model: &str) -> LlmProfile {
        let model = if !self.model.trim().is_empty() {
            self.model.trim().to_string()
        } else if let Some(m) = self.orch_model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            m.to_string()
        } else if let Some(m) = self.unit_model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            m.to_string()
        } else {
            fallback_model.to_string()
        };
        LlmProfile {
            id: self.id,
            name: self.name,
            base_url: self.base_url,
            api_key: self.api_key,
            api_keys: self.api_keys,
            api_format: self.api_format,
            model,
            models: self
                .models
                .unwrap_or_default()
                .into_iter()
                .filter(|m| !m.model.trim().is_empty())
                .collect(),
            fallback: self.fallback,
        }
    }
}

/// LLM 段历史值：旧配置区分指挥体 / 子个体模型，合并为单一 `model`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LlmConfigFile {
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    orch_model: Option<String>,
    #[serde(default)]
    unit_model: Option<String>,
}

impl LlmConfigFile {
    fn into_config(self) -> LlmConfig {
        let model = if !self.model.trim().is_empty() {
            self.model.trim().to_string()
        } else if let Some(m) = self.orch_model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            m.to_string()
        } else if let Some(m) = self.unit_model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            m.to_string()
        } else {
            String::new()
        };
        LlmConfig {
            base_url: self.base_url,
            api_key: self.api_key,
            api_keys: Vec::new(),
            api_format: String::new(),
            model,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SecurityFile {
    #[serde(default)]
    exec_approval: Option<String>,
    #[serde(default)]
    exec_allowlist: Option<Vec<String>>,
    #[serde(default)]
    auth_key: Option<String>,
    #[serde(default)]
    terminal_timeout_secs: Option<u32>,
    #[serde(default)]
    tool_output_spill_chars: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AutomationFile {
    #[serde(default)]
    heartbeat_enabled: Option<bool>,
    #[serde(default)]
    heartbeat_interval_minutes: Option<u32>,
    #[serde(default)]
    heartbeat_prompt: Option<String>,
    #[serde(default)]
    auto_adapt: Option<bool>,
    #[serde(default)]
    unit_max_steps: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MemoryConfigFile {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    recall_limit: Option<usize>,
    #[serde(default)]
    half_life_days: Option<f64>,
    /// 语义检索目标："档案ID" 或 "档案ID/模型名"；空 = 仅词项召回
    #[serde(default)]
    semantic_model: Option<String>,
}

/// 安装清单：记录本次安装的剖面与渠道开关（与运行配置解耦）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallManifest {
    pub installed_at: String,
    pub app_version: String,
    pub config_version: u32,
    /// quick | custom
    pub profile: String,
    pub channels: InstallChannels,
    pub features: InstallFeatures,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallChannels {
    pub cli: bool,
    pub gateway: bool,
    pub gateway_port: u16,
    pub webui: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallFeatures {
    pub memory: bool,
    pub tools: bool,
}

impl Default for InstallManifest {
    fn default() -> Self {
        InstallManifest {
            installed_at: crate::types::now_iso(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            config_version: CONFIG_VERSION,
            profile: "quick".into(),
            channels: InstallChannels { cli: true, gateway: true, gateway_port: 4173, webui: true },
            features: InstallFeatures { memory: true, tools: true },
            notes: vec![],
        }
    }
}

/// 测试替身开关：仅 `EXM_LLM_MOCK=1|true|yes` 时启用（单测/端到端验收/无网联调流程）。
/// 产品运行时**不存在**"无密钥自动降级"这一类行为——未配置模型即明确报错并引导配置。
pub fn mock_enabled_from_env() -> bool {
    matches!(
        std::env::var("EXM_LLM_MOCK").unwrap_or_default().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl ExmConfig {
    /// 语言（仅供低层组件读取，避免整份配置依赖）：EXM_LANG，默认 zh
    pub fn load_language() -> String {
        std::env::var("EXM_LANG")
            .map(|v| if v.is_empty() { "zh".into() } else { v })
            .unwrap_or_else(|_| "zh".into())
    }

    pub fn state_dir(workspace_root: impl AsRef<Path>) -> PathBuf {
        workspace_root.as_ref().join(".exmachina")
    }

    pub fn install_manifest_path(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(|d| d.join("install.json"))
            .unwrap_or_else(|| PathBuf::from("install.json"))
    }

    pub fn load(workspace_root: impl AsRef<Path>) -> Self {
        let root = workspace_root.as_ref().to_path_buf();
        let dir = Self::state_dir(&root);
        let _ = std::fs::create_dir_all(&dir);
        let config_path = dir.join("config.json");

        let file = read_config_file(&config_path);
        let file = migrate_file(file, &config_path);

        let file_llm = file.llm.clone().unwrap_or_default().into_config();
        let file_mem = file.memory.clone().unwrap_or_default();
        let file_sec = file.security.clone().unwrap_or_default();
        let file_auto = file.automation.clone().unwrap_or_default();
        let security = SecurityConfig {
            exec_approval: file_sec
                .exec_approval
                .clone()
                .unwrap_or_else(|| SecurityConfig::default().exec_approval),
            exec_allowlist: file_sec.exec_allowlist.clone().unwrap_or_default(),
            auth_key: std::env::var("EXM_AUTH_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| file_sec.auth_key.clone().unwrap_or_default()),
            terminal_timeout_secs: file_sec
                .terminal_timeout_secs
                .unwrap_or_else(|| SecurityConfig::default().terminal_timeout_secs),
            tool_output_spill_chars: file_sec
                .tool_output_spill_chars
                .unwrap_or_else(|| SecurityConfig::default().tool_output_spill_chars),
        };
        let automation = AutomationConfig {
            heartbeat_enabled: file_auto.heartbeat_enabled
                .unwrap_or_else(|| AutomationConfig::default().heartbeat_enabled),
            heartbeat_interval_minutes: file_auto.heartbeat_interval_minutes
                .unwrap_or_else(|| AutomationConfig::default().heartbeat_interval_minutes),
            heartbeat_prompt: file_auto.heartbeat_prompt
                .clone()
                .unwrap_or_else(|| AutomationConfig::default().heartbeat_prompt),
            auto_adapt: file_auto
                .auto_adapt
                .unwrap_or_else(|| AutomationConfig::default().auto_adapt),
            unit_max_steps: file_auto
                .unit_max_steps
                .unwrap_or_else(|| AutomationConfig::default().unit_max_steps),
        };

        let mut llm = LlmConfig {
            base_url: env_or("EXM_LLM_BASE_URL", &file_llm.base_url, "https://api.openai.com/v1"),
            api_key: env_or("EXM_LLM_API_KEY", &file_llm.api_key, ""),
            api_keys: Vec::new(),
            api_format: String::new(),
            model: env_or("EXM_LLM_MODEL", &file_llm.model, ""),
        };

        // 模型档案：旧配置自动迁移为单一 default 档案；激活档案的值解析进 llm（env 仍最高优先）
        let mut llm_profiles: Vec<LlmProfile> = file
            .llm_profiles
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.into_profile(&llm.model))
            .collect();
        let active_profile = file.active_profile.clone().unwrap_or_else(|| "default".into());
        if llm_profiles.is_empty() {
            llm_profiles.push(LlmProfile {
                id: "default".into(),
                name: "默认".into(),
                base_url: llm.base_url.clone(),
                api_key: llm.api_key.clone(),
                api_keys: Vec::new(),
                api_format: String::new(),
                model: llm.model.clone(),
                models: Vec::new(),
                fallback: None,
            });
        }
        if !llm_profiles.iter().any(|p| p.id == active_profile) {
            if let Some(first) = llm_profiles.first() {
                llm = LlmConfig {
                    base_url: env_or("EXM_LLM_BASE_URL", &first.base_url, "https://api.openai.com/v1"),
                    api_key: env_or("EXM_LLM_API_KEY", &first.api_key, ""),
                    api_keys: Vec::new(),
                    api_format: String::new(),
                    model: env_or("EXM_LLM_MODEL", &first.model, ""),
                };
            }
        } else if let Some(p) = llm_profiles.iter().find(|p| p.id == active_profile) {
            let mut keys: Vec<String> = p.api_keys.clone().into_iter().filter(|k| !k.trim().is_empty()).collect();
            if keys.is_empty() && !p.api_key.trim().is_empty() {
                keys.push(p.api_key.clone());
            }
            llm = LlmConfig {
                base_url: env_or("EXM_LLM_BASE_URL", &p.base_url, "https://api.openai.com/v1"),
                api_key: env_or("EXM_LLM_API_KEY", &p.api_key, ""),
                api_keys: keys,
                api_format: p.api_format.clone(),
                model: env_or("EXM_LLM_MODEL", &p.model, ""),
            };
        }

        let data_dir = std::env::var("EXM_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dir.join("data"));
        let file_cap = file.capabilities.clone().unwrap_or_default();
        let agents_dir = std::env::var("EXM_AGENTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| root.join("agents"));
        let webui_dist = std::env::var("EXM_WEBUI_DIST")
            .map(PathBuf::from)
            .unwrap_or_else(|_| root.join("webui").join("dist"));

        // 无密钥**不再**自动降级为模拟通道：未配置模型就是未配置，由界面/CLI 明确引导。
        // 测试替身仅在显式设置环境变量时启用（见 docs/架构与设计.md「测试策略」）。
        let use_mock = mock_enabled_from_env();

        ExmConfig {
            config_version: CONFIG_VERSION,
            workspace_root: root,
            data_dir,
            config_path,
            memory_md_path: dir.join("memory.md"),
            agents_dir,
            webui_dist,
            llm,
            max_concurrency: file.max_concurrency.unwrap_or(4),
            max_session_tokens: file.max_session_tokens.unwrap_or(0),
            memory_md_max_chars: file.memory_md_max_chars.unwrap_or(5000),
            memory_enabled: file_mem.enabled.unwrap_or(true),
            memory_recall_limit: file_mem.recall_limit.unwrap_or(5),
            memory_half_life_days: file_mem.half_life_days.unwrap_or(30.0),
            memory_semantic_model: file_mem.semantic_model.clone().unwrap_or_default(),
            speech_model: file_cap.speech.unwrap_or_default(),
            stt_model: file_cap.transcribe.unwrap_or_default(),
            vision_relay_model: file_cap.vision_relay.unwrap_or_default(),
            use_mock,
            security,
            automation,
            search: file.search.clone().unwrap_or_default(),
            hooks: file.hooks.clone().unwrap_or_default(),
            sandbox: file.sandbox.clone().unwrap_or_default(),
            browser: file.browser.clone().unwrap_or_default(),
            tools: {
                let mut t = file.tools.clone().unwrap_or_default();
                // 声明式工具的合法性兜底：名字不合法或 target 为空的条目直接丢弃（不承诺坏配置）
                t.custom.retain(|c| c.valid() && !c.target.trim().is_empty());
                t
            },
            llm_profiles,
            active_profile,
            language: std::env::var("EXM_LANG").unwrap_or_else(|_| "zh".into()),
            mcp_servers: file.mcp_servers.unwrap_or_default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let file = ConfigFile {
            config_version: Some(CONFIG_VERSION),
            llm: Some(LlmConfigFile {
                base_url: self.llm.base_url.clone(),
                api_key: self.llm.api_key.clone(),
                model: self.llm.model.clone(),
                orch_model: None,
                unit_model: None,
            }),
            max_concurrency: Some(self.max_concurrency),
            max_session_tokens: Some(self.max_session_tokens),
            memory_md_max_chars: Some(self.memory_md_max_chars),
            memory: Some(MemoryConfigFile {
                enabled: Some(self.memory_enabled),
                recall_limit: Some(self.memory_recall_limit),
                half_life_days: Some(self.memory_half_life_days),
                semantic_model: Some(self.memory_semantic_model.clone()),
            }),
            llm_profiles: Some(
                self.llm_profiles
                    .iter()
                    .map(|p| LlmProfileFile {
                        id: p.id.clone(),
                        name: p.name.clone(),
                        base_url: p.base_url.clone(),
                        api_key: p.api_key.clone(),
                        api_keys: p.api_keys.clone(),
                        api_format: p.api_format.clone(),
                        model: p.model.clone(),
                        models: Some(p.models.clone()),
                        orch_model: None,
                        unit_model: None,
                        fallback: p.fallback.clone(),
                        embed_model: None,
                    })
                    .collect(),
            ),
            active_profile: Some(self.active_profile.clone()),
            capabilities: Some(CapabilitiesFile {
                speech: Some(self.speech_model.clone()),
                transcribe: Some(self.stt_model.clone()),
                vision_relay: Some(self.vision_relay_model.clone()),
            }),
            mcp_servers: Some(self.mcp_servers.clone()),
            security: Some(SecurityFile {
                exec_approval: Some(self.security.exec_approval.clone()),
                exec_allowlist: Some(self.security.exec_allowlist.clone()),
                auth_key: Some(self.security.auth_key.clone()),
                terminal_timeout_secs: Some(self.security.terminal_timeout_secs),
                tool_output_spill_chars: Some(self.security.tool_output_spill_chars),
            }),
            automation: Some(AutomationFile {
                heartbeat_enabled: Some(self.automation.heartbeat_enabled),
                heartbeat_interval_minutes: Some(self.automation.heartbeat_interval_minutes),
                heartbeat_prompt: Some(self.automation.heartbeat_prompt.clone()),
                auto_adapt: Some(self.automation.auto_adapt),
                unit_max_steps: Some(self.automation.unit_max_steps),
            }),
            search: Some(self.search.clone()),
            hooks: Some(self.hooks.clone()),
            sandbox: Some(self.sandbox.clone()),
            browser: Some(self.browser.clone()),
            tools: Some(self.tools.clone()),
        };
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.config_path, serde_json::to_string_pretty(&file)?)?;
        Ok(())
    }

    pub fn load_manifest(&self) -> Option<InstallManifest> {
        std::fs::read_to_string(self.install_manifest_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn save_manifest(&self, manifest: &InstallManifest) -> anyhow::Result<()> {
        let p = self.install_manifest_path();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, serde_json::to_string_pretty(manifest)?)?;
        Ok(())
    }
}

fn read_config_file(path: &Path) -> ConfigFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 版本迁移：低版本文件补齐新字段默认值后回写
fn migrate_file(mut file: ConfigFile, path: &Path) -> ConfigFile {
    let version = file.config_version.unwrap_or(0);
    if version >= CONFIG_VERSION {
        return file;
    }
    if file.llm.is_none() {
        file.llm = Some(LlmConfigFile::default());
    }
    if file.memory.is_none() {
        file.memory = Some(MemoryConfigFile {
            enabled: Some(true),
            recall_limit: Some(5),
            half_life_days: Some(30.0),
            semantic_model: None,
        });
    }
    if file.max_concurrency.is_none() {
        file.max_concurrency = Some(4);
    }
    file.config_version = Some(CONFIG_VERSION);
    if let Ok(s) = serde_json::to_string_pretty(&file) {
        let _ = std::fs::write(path, s);
    }
    file
}

fn env_or(key: &str, file_value: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            if !file_value.trim().is_empty() {
                file_value.to_string()
            } else {
                default.to_string()
            }
        }
    }
}

// ---------------------------------------------------------------- 配置 Schema（UI/向导驱动）

/// 字段描述：CLI 安装向导与 WebUI 设置页共用，新增配置项零 UI 改动
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "configVersion": CONFIG_VERSION,
        "groups": [
            {
                "key": "llm",
                "label": "模型接入",
                "fields": [
                    { "key": "llm.baseUrl", "label": "LLM Base URL（OpenAI 兼容）", "kind": "string",
                      "default": "https://api.openai.com/v1", "required": true,
                      "help": "支持官方 OpenAI、DeepSeek、通义、vLLM、Ollama 等任意兼容端点" },
                    { "key": "llm.apiKey", "label": "API Key", "kind": "password", "default": "",
                      "required": false, "help": "未配置时不接受推理请求，请在「提供商」页或此填写" },
                    { "key": "llm.model", "label": "默认模型", "kind": "string",
                      "default": "", "required": true, "help": "该端点的模型名；个体与组可各自覆盖" }
                ]
            },
            {
                "key": "runtime",
                "label": "运行时",
                "fields": [
                    { "key": "maxConcurrency", "label": "子个体并发数", "kind": "number",
                      "default": "4", "required": true, "min": 1, "max": 32,
                      "help": "调度器同时执行的最大子个体数量" },
                    { "key": "maxSessionTokens", "label": "会话 token 预算", "kind": "number",
                      "default": "0", "required": false, "min": 0, "max": 100000000,
                      "help": "估算口径（字符/4）；超出后该会话拒绝新轮次；0 = 不限" }
                ]
            },
            {
                "key": "memory",
                "label": "记忆系统",
                "fields": [
                    { "key": "memory.enabled", "label": "深层记忆（数据库检索 + 自动写入）", "kind": "boolean",
                      "default": "true", "required": false,
                      "help": "关闭后仅使用 memory.md 文件记忆（文件记忆模式）：规划时注入该文件内容，可在记忆面板直接编辑；不写数据库、不做语义检索" },
                    { "key": "memory.mdMaxChars", "label": "memory.md 字数上限", "kind": "number",
                      "default": "5000", "required": false, "min": 500, "max": 100000,
                      "help": "超过上限触发 AI 自主压缩简略；被精简的原文自动归档（深层开 = 存数据库，关 = 存工作区归档文件）" },
                    { "key": "memory.recallLimit", "label": "单轮召回条数", "kind": "number",
                      "default": "5", "required": false, "min": 0, "max": 20,
                      "help": "注入指挥体上下文的历史记忆条数" },
                    { "key": "memory.halfLifeDays", "label": "记忆衰减半衰期（天）", "kind": "number",
                      "default": "30", "required": false, "min": 1, "max": 365,
                      "help": "未被使用的记忆重要性按该半衰期衰减" }
                ]
            },
            {
                "key": "security",
                "label": "安全与审批",
                "fields": [
                    { "key": "security.execApproval", "label": "终端命令审批", "kind": "string",
                      "default": "off", "required": false,
                      "help": "off=不拦截；risky=拦截高危命令（删除/强推/关机等）；always=全部终端命令需审批。白名单前缀可跳过" },
                    { "key": "security.execAllowlist", "label": "审批白名单前缀", "kind": "string",
                      "default": "", "required": false,
                      "help": "逗号分隔的命令前缀，命中即免审批，如：git status,cargo test" },
                    { "key": "security.authKey", "label": "后台访问密钥", "kind": "password",
                      "default": "", "required": false,
                      "help": "非空时 WebUI 与全部 /api 请求需鉴权（浏览器出现登录门；程序调用带 X-Auth-Key 头）；留空 = 免鉴权。掩码表示沿用已配置密钥，清空保存 = 关闭鉴权" },
                    { "key": "security.terminalTimeoutSecs", "label": "终端命令超时（秒）", "kind": "number",
                      "default": "120", "required": false, "min": 10, "max": 1800,
                      "help": "前台命令的最长执行时间；超时强杀进程树。长任务请让 AI 用后台模式（不受此限）" },
                    { "key": "security.toolOutputSpillChars", "label": "工具结果落盘阈值（字符）", "kind": "number",
                      "default": "12000", "required": false, "min": 1000, "max": 1000000,
                      "help": "结果超过该长度即全文落盘（.exmachina/tool-output/），回填摘要与文件路径供按需回读；0 = 仅截断" }
                ]
            },
            {
                "key": "automation",
                "label": "自动化与心跳",
                "fields": [
                    { "key": "automation.heartbeatEnabled", "label": "启用心跳巡检", "kind": "boolean",
                      "default": "false", "required": false,
                      "help": "网关调度器按周期向心跳会话发送巡检提示（需 exm serve 常驻）" },
                    { "key": "automation.heartbeatIntervalMinutes", "label": "心跳间隔（分钟）", "kind": "number",
                      "default": "30", "required": false, "min": 5, "max": 1440,
                      "help": "心跳巡检的执行周期" },
                    { "key": "automation.autoAdapt", "label": "自动经验优化", "kind": "boolean",
                      "default": "true", "required": false,
                      "help": "子个体新教训累计达 3 条时自动提炼经验改进要点并注入其派发（不创建个体）" },
                    { "key": "automation.heartbeatPrompt", "label": "心跳提示词", "kind": "string",
                      "default": "系统心跳巡检：检查未决任务、受阻节点与风险账，无事项则简短报告正常。", "required": false,
                      "help": "每次心跳注入给指挥体的提示词" },
                    { "key": "automation.unitMaxSteps", "label": "子个体单次派发步数上限", "kind": "number",
                      "default": "16", "required": false, "min": 3, "max": 60,
                      "help": "一次派发内允许的「思考-调工具」轮次；复杂任务（多文件改动）需要更大的步数预算" }
                ]
            },
            {
                "key": "search",
                "label": "联网搜索",
                "fields": [
                    { "key": "search.provider", "label": "搜索后端", "kind": "string",
                      "default": "", "required": false,
                      "help": "tavily | brave | searxng | custom；留空 = 未配置 —— 此时 web_search 工具不下发给模型（不承诺不存在的能力）" },
                    { "key": "search.endpoint", "label": "搜索端点", "kind": "string",
                      "default": "", "required": false,
                      "help": "searxng 填实例地址（如 https://searx.example.com）；custom 填你自己的 JSON 搜索接口" },
                    { "key": "search.apiKey", "label": "搜索 API Key", "kind": "password",
                      "default": "", "required": false,
                      "help": "tavily / brave 需要；searxng 与自建通常留空" },
                    { "key": "search.maxResults", "label": "返回条数上限", "kind": "number",
                      "default": "5", "required": false, "min": 1, "max": 20,
                      "help": "单次搜索注入上下文的结果条数" }
                ]
            },
            {
                "key": "sandbox",
                "label": "沙箱执行",
                "fields": [
                    { "key": "sandbox.mode", "label": "沙箱级别", "kind": "string",
                      "default": "workspace", "required": false,
                      "help": "off=仅白名单+审批；workspace=子进程环境净化（敏感环境变量不外泄，HOME/TMP 重定向到 .exmachina/sandbox）；strict=净化 + 联网类命令走审批 + Linux/macOS 检测到 bubblewrap 时做系统级隔离" },
                    { "key": "sandbox.allowNetwork", "label": "strict 下允许联网安装", "kind": "boolean",
                      "default": "false", "required": false,
                      "help": "开启后 pip/npm install、curl/wget 等在 strict 模式无需审批" },
                    { "key": "sandbox.useBwrap", "label": "使用 bubblewrap（Linux/macOS）", "kind": "boolean",
                      "default": "true", "required": false,
                      "help": "检测到 bwrap 时用其做文件系统隔离：根只读、工作区可写、按需断网" },
                    { "key": "sandbox.memoryMb", "label": "子进程内存上限（MB）", "kind": "number",
                      "default": "0", "required": false, "min": 0, "max": 1048576,
                      "help": "Linux 经 ulimit、Windows 经作业对象生效；0 = 不限" },
                    { "key": "sandbox.maxProcesses", "label": "子进程数上限（Windows）", "kind": "number",
                      "default": "0", "required": false, "min": 0, "max": 4096,
                      "help": "Windows 作业对象的活动进程上限，防 fork 炸弹；0 = 不限" }
                ]
            },
            {
                "key": "browser",
                "label": "浏览器自动化",
                "fields": [
                    { "key": "browser.executable", "label": "浏览器可执行文件", "kind": "string",
                      "default": "", "required": false,
                      "help": "留空自动探测 Chrome/Chromium/Edge 常见安装位置（含 Playwright 缓存）" },
                    { "key": "browser.headless", "label": "无头模式", "kind": "boolean",
                      "default": "true", "required": false,
                      "help": "默认无头；调试时可关掉看真实窗口" },
                    { "key": "browser.timeoutSecs", "label": "单次操作超时（秒）", "kind": "number",
                      "default": "30", "required": false, "min": 5, "max": 300,
                      "help": "导航与脚本执行的等待上限" },
                    { "key": "browser.maxChars", "label": "正文提取上限（字符）", "kind": "number",
                      "default": "8000", "required": false, "min": 500, "max": 100000,
                      "help": "browser text 单次返回的正文长度" }
                ]
            },
            {
                "key": "tools",
                "label": "自定义工具",
                "fields": [
                    { "key": "tools.custom", "label": "声明式工具清单（JSON 数组）", "kind": "string",
                      "default": "", "required": false,
                      "help": "把 HTTP 接口或命令模板包成模型可调用的工具，免写 MCP 服务器。JSON 数组，元素字段：name（字母开头）、description、kind（http|shell）、target（{参数名} 占位）、method（GET|POST，默认 POST）、headers、parameters（JSON Schema）、agents（可见个体，空=全部）。例：[{\"name\":\"weather\",\"description\":\"查天气\",\"kind\":\"http\",\"method\":\"GET\",\"target\":\"https://api.example.com/now?city={city}\",\"parameters\":{\"type\":\"object\",\"properties\":{\"city\":{\"type\":\"string\"}},\"required\":[\"city\"]}}]" }
                ]
            },
            {
                "key": "hooks",
                "label": "生命周期钩子",
                "fields": [
                    { "key": "hooks.preTool", "label": "工具调用前钩子", "kind": "string",
                      "default": "", "required": false,
                      "help": "逗号分隔的命令（每行一条亦可）；环境变量 EXM_TOOL / EXM_AGENT / EXM_ARGS / EXM_WORKSPACE 可用。**非零退出 = 拦截该次工具调用**并回报原因" },
                    { "key": "hooks.postTool", "label": "工具调用后钩子", "kind": "string",
                      "default": "", "required": false,
                      "help": "同上；额外有 EXM_TOOL_OK（true/false）。用于留痕、通知、指标采集" },
                    { "key": "hooks.onRunEnd", "label": "运行收束钩子", "kind": "string",
                      "default": "", "required": false,
                      "help": "一轮 run 结束（成功或失败）时触发；EXM_RUN_STATUS 给出终态" }
                ]
            }
        ]
    })
}
