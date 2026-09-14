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

/// 厂商模型档案：一条 OpenAI 兼容端点（OpenAI/DeepSeek/通义/Kimi/智谱/Ollama/vLLM …）。
/// 多档案可并存，`active_profile` 决定当前生效者；切换即热生效（apply_config 重建运行时）。
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
    #[serde(default)]
    pub orch_model: String,
    #[serde(default)]
    pub unit_model: String,
    /// 失败回退：下一个档案 id（请求失败且未发出内容时切换；成环自动截断）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// 嵌入模型（混合记忆检索用；空 = 该档案不提供嵌入）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_model: Option<String>,
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
    pub orch_model: String,
    pub unit_model: String,
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
}

impl Default for SecurityConfig {
    fn default() -> Self {
        SecurityConfig { exec_approval: "off".into(), exec_allowlist: vec![], auth_key: String::new() }
    }
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
}

impl Default for AutomationConfig {
    fn default() -> Self {
        AutomationConfig {
            heartbeat_enabled: false,
            heartbeat_interval_minutes: 30,
            heartbeat_prompt: "系统心跳巡检：检查未决任务、受阻节点与风险账，无事项则简短报告正常。".into(),
            auto_adapt: true,
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
    /// 记忆系统开关与参数
    pub memory_enabled: bool,
    pub memory_recall_limit: usize,
    pub memory_half_life_days: f64,
    /// 无密钥时启用 Mock 通道
    pub use_mock: bool,
    /// 执行审批（安全闸门）
    pub security: SecurityConfig,
    /// 自动化（心跳巡检）
    pub automation: AutomationConfig,
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
    llm: Option<LlmConfig>,
    #[serde(default)]
    max_concurrency: Option<usize>,
    #[serde(default)]
    max_session_tokens: Option<u64>,
    #[serde(default)]
    memory: Option<MemoryConfigFile>,
    #[serde(default)]
    security: Option<SecurityFile>,
    #[serde(default)]
    automation: Option<AutomationFile>,
    #[serde(default)]
    llm_profiles: Option<Vec<LlmProfile>>,
    #[serde(default)]
    active_profile: Option<String>,
    #[serde(default)]
    mcp_servers: Option<Vec<crate::mcp::McpServerConfig>>,
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

        let file_llm = file.llm.clone().unwrap_or_default();
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
        };

        let mut llm = LlmConfig {
            base_url: env_or("EXM_LLM_BASE_URL", &file_llm.base_url, "https://api.openai.com/v1"),
            api_key: env_or("EXM_LLM_API_KEY", &file_llm.api_key, ""),
            api_keys: Vec::new(),
            api_format: String::new(),
            orch_model: env_or("EXM_LLM_MODEL_ORCH", &file_llm.orch_model, ""),
            unit_model: env_or("EXM_LLM_MODEL_UNIT", &file_llm.unit_model, ""),
        };

        // 模型档案：旧配置自动迁移为单一 default 档案；激活档案的值解析进 llm（env 仍最高优先）
        let mut llm_profiles: Vec<LlmProfile> = file.llm_profiles.clone().unwrap_or_default();
        let active_profile = file.active_profile.clone().unwrap_or_else(|| "default".into());
        if llm_profiles.is_empty() {
            llm_profiles.push(LlmProfile {
                id: "default".into(),
                name: "默认".into(),
                base_url: llm.base_url.clone(),
                api_key: llm.api_key.clone(),
                api_keys: Vec::new(),
                api_format: String::new(),
                orch_model: llm.orch_model.clone(),
                unit_model: llm.unit_model.clone(),
                fallback: None,
                embed_model: None,
            });
        }
        if !llm_profiles.iter().any(|p| p.id == active_profile) {
            if let Some(first) = llm_profiles.first() {
                llm = LlmConfig {
                    base_url: env_or("EXM_LLM_BASE_URL", &first.base_url, "https://api.openai.com/v1"),
                    api_key: env_or("EXM_LLM_API_KEY", &first.api_key, ""),
                    api_keys: Vec::new(),
                    api_format: String::new(),
                    orch_model: env_or("EXM_LLM_MODEL_ORCH", &first.orch_model, ""),
                    unit_model: env_or("EXM_LLM_MODEL_UNIT", &first.unit_model, ""),
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
                orch_model: env_or("EXM_LLM_MODEL_ORCH", &p.orch_model, ""),
                unit_model: env_or("EXM_LLM_MODEL_UNIT", &p.unit_model, ""),
            };
        }

        let data_dir = std::env::var("EXM_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dir.join("data"));
        let agents_dir = std::env::var("EXM_AGENTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| root.join("agents"));
        let webui_dist = std::env::var("EXM_WEBUI_DIST")
            .map(PathBuf::from)
            .unwrap_or_else(|_| root.join("webui").join("dist"));

        let use_mock = llm.api_key.trim().is_empty();

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
            memory_enabled: file_mem.enabled.unwrap_or(true),
            memory_recall_limit: file_mem.recall_limit.unwrap_or(5),
            memory_half_life_days: file_mem.half_life_days.unwrap_or(30.0),
            use_mock,
            security,
            automation,
            llm_profiles,
            active_profile,
            language: std::env::var("EXM_LANG").unwrap_or_else(|_| "zh".into()),
            mcp_servers: file.mcp_servers.unwrap_or_default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let file = ConfigFile {
            config_version: Some(CONFIG_VERSION),
            llm: Some(self.llm.clone()),
            max_concurrency: Some(self.max_concurrency),
            max_session_tokens: Some(self.max_session_tokens),
            memory: Some(MemoryConfigFile {
                enabled: Some(self.memory_enabled),
                recall_limit: Some(self.memory_recall_limit),
                half_life_days: Some(self.memory_half_life_days),
            }),
            llm_profiles: Some(self.llm_profiles.clone()),
            active_profile: Some(self.active_profile.clone()),
            mcp_servers: Some(self.mcp_servers.clone()),
            security: Some(SecurityFile {
                exec_approval: Some(self.security.exec_approval.clone()),
                exec_allowlist: Some(self.security.exec_allowlist.clone()),
                auth_key: Some(self.security.auth_key.clone()),
            }),
            automation: Some(AutomationFile {
                heartbeat_enabled: Some(self.automation.heartbeat_enabled),
                heartbeat_interval_minutes: Some(self.automation.heartbeat_interval_minutes),
                heartbeat_prompt: Some(self.automation.heartbeat_prompt.clone()),
                auto_adapt: Some(self.automation.auto_adapt),
            }),
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
        file.llm = Some(LlmConfig::default());
    }
    if file.memory.is_none() {
        file.memory = Some(MemoryConfigFile {
            enabled: Some(true),
            recall_limit: Some(5),
            half_life_days: Some(30.0),
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
                      "required": false, "help": "留空则以 Mock 模拟通道运行；填入后热切换为真实推理" },
                    { "key": "llm.orchModel", "label": "指挥体模型", "kind": "string",
                      "default": "", "required": true, "help": "主智能体（规划/裁决/收束）" },
                    { "key": "llm.unitModel", "label": "子个体模型", "kind": "string",
                      "default": "", "required": true, "help": "子个体执行用模型" }
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
                    { "key": "memory.enabled", "label": "启用记忆系统", "kind": "boolean",
                      "default": "true", "required": false,
                      "help": "关闭后不召回、不写入深层记忆（memory.md 保留只读）" },
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
                      "help": "逗号分隔的命令前缀，命中即免审批，如：git status,cargo test" }
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
                      "help": "每次心跳注入给指挥体的提示词" }
                ]
            }
        ]
    })
}
