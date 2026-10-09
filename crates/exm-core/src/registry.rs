//! 个体注册表 —— 多组实现（docs/09）
//!
//! 组（Group）是隔离与切换的基本单位，统一布局 `agents/groups/<gid>/`：
//!   - SOUL.md（主智能体人格，用户面）+ MEMORY.md（组浅层记忆快照）
//!   - agents/*.json（成员定义，含主智能体）+ prompts/*.md（成员提示词）+ adaptations/（经验要点）
//!   - 内置默认组 = groups/default（builtin=true，定义受保护）；主智能体与单体持 SOUL，子个体无人格层
//!   - 集群级共享：protocol/（协议）、skills/（技能）、default-soul.md（兜底人格）
//! 切换组即热切换：指挥体的规划/派发/提示词/SOUL 全部落在激活组内。
//! 分布式实现可替换本文件（接口一致）。

use crate::types::{normalize_domain, AgentDefinition, GroupMeta, Tier, ToolName};
use anyhow::Context;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct GroupData {
    pub meta: GroupMeta,
    pub defs: HashMap<String, AgentDefinition>,
    pub dir: PathBuf,
}

/// 个体可编辑字段补丁（None = 沿用旧值）。identifier/tier/prompt_file 不可经此修改。
/// model_hint 语义与 set_agent_model 一致：Some("") = 清除（跟随所属组/全局）。
#[derive(Debug, Clone, Default)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub domain: Option<String>,
    pub description: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub tools: Option<Vec<ToolName>>,
    pub model_hint: Option<String>,
}

impl AgentPatch {
    fn apply(&self, def: &mut AgentDefinition) {
        if let Some(v) = self.name.as_deref() {
            let t = v.trim();
            if !t.is_empty() {
                def.name = t.to_string();
            }
        }
        if let Some(v) = self.domain.as_deref() {
            def.domain = normalize_domain(v);
        }
        if let Some(v) = self.description.as_deref() {
            let t = v.trim();
            if !t.is_empty() {
                def.description = t.to_string();
            }
        }
        if let Some(list) = &self.capabilities {
            def.capabilities = list
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Some(list) = &self.tools {
            def.tools = list.clone();
        }
        if let Some(v) = self.model_hint.as_deref() {
            let t = v.trim();
            def.model_hint = if t.is_empty() { None } else { Some(t.to_string()) };
        }
    }
}

pub struct LocalRegistry {
    dir: PathBuf,
    groups: RwLock<HashMap<String, GroupData>>,
    active: RwLock<String>,
    /// 单体智能体（独立于任何组，直接作为交互对象；entities/agents/）
    singles: RwLock<HashMap<String, AgentDefinition>>,
    /// 当前交互目标：None = 激活组；Some(id) = 单体智能体
    active_single: RwLock<Option<String>>,
    /// 工作目录（干活的项目）：显式设置后优先于组声明与全局根，智能体/智能体组通用
    active_workspace: RwLock<Option<String>>,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && s.len() <= 48
}

impl LocalRegistry {
    pub fn new(agents_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let dir = agents_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(dir.join("agents"))?;
        let reg = LocalRegistry {
            dir,
            groups: RwLock::new(HashMap::new()),
            active: RwLock::new("exmachina".to_string()),
            singles: RwLock::new(HashMap::new()),
            active_single: RwLock::new(None),
            active_workspace: RwLock::new(None),
        };
        reg.reload()?;
        Ok(reg)
    }

    pub fn reload(&self) -> anyhow::Result<()> {
        let mut groups = HashMap::new();

        // 组统一放 agents/groups/<gid>/；内置默认组 = groups/default（builtin=true，定义受保护）
        let groups_dir = self.dir.join("groups");
        std::fs::create_dir_all(&groups_dir)?;
        let default_dir = groups_dir.join("exmachina");
        let default_meta_path = default_dir.join("group.json");
        let default_meta = if default_meta_path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&default_meta_path)?)
                .context("解析 groups/default/group.json 失败")?
        } else {
            let meta = GroupMeta {
                id: "exmachina".into(),
                name: "智能连结".into(),
                description: "内置默认组（连结体）：指挥体 + 子个体集群（定义受保护）".into(),
                primary: Some("orchestrator".into()),
                workspace: None,
                model: None,
                capabilities: None,
                builtin: true,
                created_at: crate::types::now_iso(),
            };
            std::fs::create_dir_all(&default_dir)?;
            std::fs::write(&default_meta_path, serde_json::to_string_pretty(&meta)?)?;
            meta
        };
        groups.insert(
            default_meta.id.clone(),
            self.load_group(default_meta, default_dir)?,
        );

        // 自定义组：groups/<gid>/（跳过默认组）
        if groups_dir.exists() {
            for entry in std::fs::read_dir(&groups_dir)?.filter_map(|e| e.ok()) {
                let gdir = entry.path();
                let meta_path = gdir.join("group.json");
                if !gdir.is_dir() || !meta_path.exists() {
                    continue;
                }
                let meta: GroupMeta = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)
                    .with_context(|| format!("解析组元数据失败: {}", meta_path.display()))?;
                if !valid_id(&meta.id) {
                    anyhow::bail!("非法组 id: {}", meta.id);
                }
                if meta.id == "exmachina" {
                    continue; // 默认组已在上方装载
                }
                groups.insert(meta.id.clone(), self.load_group(meta, gdir)?);
            }
        }
        if groups.is_empty() {
            anyhow::bail!("未装载到任何智能体组");
        }
        if !groups.contains_key("exmachina") {
            anyhow::bail!("缺少内置默认组");
        }

        // 单体智能体：entities/agents/*.json（独立交互对象，不属于任何组）
        let mut singles = HashMap::new();
        let singles_dir = self.dir.join("agents");
        if singles_dir.exists() {
            // 目录制：entities/agents/<id>/agent.json
            let mut def_files: Vec<PathBuf> = std::fs::read_dir(&singles_dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir() && p.join("agent.json").is_file())
                .map(|p| p.join("agent.json"))
                .collect();
            // 兼容旧布局：<id>.json 平铺定义
            if let Ok(entries) = std::fs::read_dir(&singles_dir) {
                for e in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
                    if e.is_file() && e.extension().map(|x| x == "json").unwrap_or(false) {
                        def_files.push(e);
                    }
                }
            }
            def_files.sort();
            for p in def_files {
                let def: AgentDefinition = serde_json::from_str(&std::fs::read_to_string(&p)?)
                    .with_context(|| format!("单体定义校验失败: {}", p.display()))?;
                singles.insert(def.identifier.clone(), def);
            }
        }
        *self.singles.write() = singles;
        let active_single = std::fs::read_to_string(self.dir.join("active_single")).unwrap_or_default();
        let active_single = active_single.trim();
        *self.active_single.write() =
            if active_single.is_empty() { None } else { Some(active_single.to_string()) };

        // 工作目录持久化：agents/active_workspace（干活的项目，智能体/智能体组通用）
        let active_ws = std::fs::read_to_string(self.dir.join("active_workspace")).unwrap_or_default();
        let active_ws = active_ws.trim();
        *self.active_workspace.write() =
            if active_ws.is_empty() { None } else { Some(active_ws.to_string()) };

        // 激活组持久化：agents/active_group
        let active_file = self.dir.join("active_group");
        let saved = std::fs::read_to_string(&active_file).unwrap_or_default();
        let active = saved.trim().to_string();
        let active = if groups.contains_key(&active) { active } else { "exmachina".to_string() };

        *self.groups.write() = groups;
        *self.active.write() = active;
        Ok(())
    }

    fn load_group(&self, meta: GroupMeta, dir: PathBuf) -> anyhow::Result<GroupData> {
        // 组布局：SOUL.md（主智能体人格）+ agents/（成员定义）+ prompts/（成员提示词）
        let mut defs = HashMap::new();
        let def_dir = dir.join("agents");
        if def_dir.exists() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&def_dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .collect();
            files.sort();
            for path in files {
                let raw = std::fs::read_to_string(&path)
                    .with_context(|| format!("读取定义失败: {}", path.display()))?;
                let mut def: AgentDefinition = serde_json::from_str(&raw)
                    .with_context(|| format!("定义 schema 校验失败: {}", path.display()))?;
                def.domain = normalize_domain(&def.domain);
                defs.insert(def.identifier.clone(), def);
            }
        }
        Ok(GroupData { meta, defs, dir })
    }

    #[allow(dead_code)]
    fn save_meta(&self, gid: &str) -> anyhow::Result<()> {
        let groups = self.groups.read();
        let g = groups.get(gid).context("组不存在")?;
        let path = g.dir.join("group.json");
        std::fs::write(&path, serde_json::to_string_pretty(&g.meta)?)?;
        Ok(())
    }

    // ---------------- 组管理 ----------------

    pub fn list_groups(&self) -> Vec<GroupMeta> {
        let mut metas: Vec<GroupMeta> = self.groups.read().values().map(|g| g.meta.clone()).collect();
        metas.sort_by(|a, b| a.id.cmp(&b.id));
        metas
    }

    pub fn group_meta(&self, gid: &str) -> Option<GroupMeta> {
        self.groups.read().get(gid).map(|g| g.meta.clone())
    }

    pub fn active_group(&self) -> String {
        self.active.read().clone()
    }

    pub fn active_group_meta(&self) -> Option<GroupMeta> {
        self.group_meta(&self.active_group())
    }

    pub fn set_active_group(&self, gid: &str) -> anyhow::Result<()> {
        if !self.groups.read().contains_key(gid) {
            anyhow::bail!("组不存在: {gid}");
        }
        *self.active.write() = gid.to_string();
        std::fs::write(self.dir.join("active_group"), gid)?;
        Ok(())
    }

    // ---------------- 工作目录（干活的项目） ----------------

    /// 设置工作目录（项目路径）；None/空 = 清除（回退组声明 / 全局根）。智能体与智能体组通用。
    pub fn set_active_workspace(&self, ws: Option<&str>) -> anyhow::Result<()> {
        let ws = ws.map(str::trim).filter(|s| !s.is_empty());
        if let Some(p) = ws {
            // 目录必须真实存在——工作目录是干活的项目，打到不存在的路径只会让工具静默失败
            anyhow::ensure!(std::path::Path::new(p).is_dir(), "工作目录不存在或不是目录: {p}");
        }
        *self.active_workspace.write() = ws.map(str::to_string);
        match ws {
            Some(w) => std::fs::write(self.dir.join("active_workspace"), w)?,
            None => {
                let _ = std::fs::remove_file(self.dir.join("active_workspace"));
            }
        }
        Ok(())
    }

    /// 显式工作目录（未设置 = None）
    pub fn active_workspace(&self) -> Option<String> {
        self.active_workspace.read().clone()
    }

    /// 生效工作区根：显式工作目录 > 激活组声明 > 全局根（相对路径相对全局根解析）
    pub fn effective_workspace(&self, global_root: &std::path::Path) -> std::path::PathBuf {
        let resolve = |ws: Option<String>| -> Option<PathBuf> {
            let rel = ws?;
            let trimmed = rel.trim();
            if trimmed.is_empty() {
                return None;
            }
            let p = std::path::PathBuf::from(trimmed);
            Some(if p.is_absolute() { p } else { global_root.join(p) })
        };
        resolve(self.active_workspace())
            .or_else(|| resolve(self.active_group_meta().and_then(|m| m.workspace)))
            .unwrap_or_else(|| global_root.to_path_buf())
    }


    // ---------------- 编成模板（entities/templates/，用户与 agent 共用） ----------------

    /// 模板根目录:entities/templates/(unit=子个体,group=组)
    pub fn templates_dir(&self) -> PathBuf {
        self.dir.join("templates")
    }

    /// 列出全部编成模板(unit + group)
    pub fn list_templates(&self) -> Vec<crate::types::TemplateInfo> {
        let mut out = Vec::new();
        for kind in ["unit", "group"] {
            let base = self.templates_dir().join(kind);
            let Ok(entries) = std::fs::read_dir(&base) else { continue };
            for e in entries.filter_map(|e| e.ok()) {
                let tdir = e.path();
                let meta_path = tdir.join("template.json");
                if !tdir.is_dir() || !meta_path.is_file() {
                    continue;
                }
                if let Ok(info) =
                    serde_json::from_str::<crate::types::TemplateInfo>(&std::fs::read_to_string(&meta_path).unwrap_or_default())
                {
                    out.push(info);
                }
            }
        }
        out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.id.cmp(&b.id)));
        out
    }

    /// 解析模板目录:优先匹配 template.json 的 id,回退目录名
    fn find_template_dir(&self, kind: &str, template_id: &str) -> Option<PathBuf> {
        let base = self.templates_dir().join(kind);
        let entries = std::fs::read_dir(&base).ok()?;
        for e in entries.filter_map(|e| e.ok()) {
            let tdir = e.path();
            let meta_path = tdir.join("template.json");
            if !tdir.is_dir() || !meta_path.is_file() {
                continue;
            }
            if let Ok(info) =
                serde_json::from_str::<crate::types::TemplateInfo>(&std::fs::read_to_string(&meta_path).unwrap_or_default())
            {
                if info.id == template_id {
                    return Some(tdir);
                }
            }
        }
        // 回退:目录名即模板 id
        let by_dir = base.join(template_id);
        by_dir.join("template.json").is_file().then_some(by_dir)
    }

    /// 占位符渲染:{{identifier}} / {{name}} / {{description}}
    fn render_template(raw: &str, identifier: &str, name: &str, description: &str) -> String {
        raw.replace("{{identifier}}", identifier)
            .replace("{{name}}", name)
            .replace("{{description}}", description)
    }

    /// 基于模板创建子个体:agent.json + PROMPT.md 渲染占位符后经 upsert_agent 落盘。
    /// name/description 缺省时取模板默认。
    pub fn create_agent_from_template(
        &self,
        gid: &str,
        template_id: &str,
        identifier: &str,
        name: &str,
        description: &str,
    ) -> anyhow::Result<AgentDefinition> {
        Self::validate_identifier(identifier)?;
        let tdir = self
            .find_template_dir("unit", template_id)
            .with_context(|| format!("子个体模板不存在: {template_id}"))?;
        let meta_path = tdir.join("template.json");
        let tmeta: crate::types::TemplateInfo = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
        let name = if name.trim().is_empty() { tmeta.name } else { name.trim().to_string() };
        let description =
            if description.trim().is_empty() { tmeta.description } else { description.trim().to_string() };
        let raw = std::fs::read_to_string(tdir.join("agent.json")).context("模板缺少 agent.json")?;
        let mut def: AgentDefinition =
            serde_json::from_str(&Self::render_template(&raw, identifier, &name, &description))
                .context("模板 agent.json 校验失败")?;
        def.name = name;
        def.description = description.clone();
        def.when_to_call = description;
        let prompt = std::fs::read_to_string(tdir.join("PROMPT.md"))
            .ok()
            .map(|t| Self::render_template(&t, identifier, &def.name, &def.description));
        self.upsert_agent(gid, def, prompt)
    }

    /// 基于模板创建组:复用 create_group 骨架,附加模板 SOUL.md(组人格)。
    /// name/description 缺省时取模板默认。
    pub fn create_group_from_template(
        &self,
        template_id: &str,
        id: Option<String>,
        name: &str,
        description: &str,
    ) -> anyhow::Result<GroupMeta> {
        let tdir = self
            .find_template_dir("group", template_id)
            .with_context(|| format!("组模板不存在: {template_id}"))?;
        let meta_path = tdir.join("template.json");
        let tmeta: crate::types::TemplateInfo = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
        let name = if name.trim().is_empty() { tmeta.name } else { name.trim().to_string() };
        let description =
            if description.trim().is_empty() { tmeta.description } else { description.trim().to_string() };
        let meta = self.create_group(id, &name, &description)?;
        if let Some(gdir) = self.group_dir(&meta.id) {
            if let Ok(soul) = std::fs::read_to_string(tdir.join("SOUL.md")) {
                std::fs::write(gdir.join("SOUL.md"), Self::render_template(&soul, &meta.id, &name, &description))?;
            }
        }
        Ok(meta)
    }

    /// 创建自定义组：目录骨架 + 元数据；返回组 id
    pub fn create_group(
        &self,
        id: Option<String>,
        name: &str,
        description: &str,
    ) -> anyhow::Result<GroupMeta> {
        let gid = match id {
            Some(id) if valid_id(&id) => id,
            Some(id) => anyhow::bail!("非法组 id: {id}（仅允许字母/数字/-/_，≤48 字符）"),
            None => format!("g-{}", &crate::types::new_id()[..8]),
        };
        let mut groups = self.groups.write();
        if groups.contains_key(&gid) {
            anyhow::bail!("组已存在: {gid}");
        }
        if name.trim().is_empty() {
            anyhow::bail!("组名不能为空");
        }
        let gdir = self.dir.join("groups").join(&gid);
        std::fs::create_dir_all(&gdir)?;
        let meta = GroupMeta {
            id: gid.clone(),
            name: name.trim().to_string(),
            description: description.trim().to_string(),
            primary: None,
            workspace: None,
            model: None,
            capabilities: None,
            builtin: false,
            created_at: crate::types::now_iso(),
        };
        std::fs::write(gdir.join("group.json"), serde_json::to_string_pretty(&meta)?)?;
        groups.insert(gid.clone(), self.load_group(meta.clone(), gdir)?);
        Ok(meta)
    }

    /// 删除自定义组（内置组与激活组不可删）
    pub fn delete_group(&self, gid: &str) -> anyhow::Result<()> {
        if gid == "exmachina" || self.group_meta(gid).map(|m| m.builtin).unwrap_or(false) {
            anyhow::bail!("内置组不可删除");
        }
        if self.active_group() == gid {
            anyhow::bail!("激活组不可删除，请先切换到其他组");
        }
        if !self.groups.read().contains_key(gid) {
            anyhow::bail!("组不存在: {gid}");
        }
        let gdir = self.dir.join("groups").join(gid);
        std::fs::remove_dir_all(gdir)?;
        self.groups.write().remove(gid);
        Ok(())
    }

    pub fn is_builtin(&self, gid: &str) -> bool {
        self.group_meta(gid).map(|m| m.builtin).unwrap_or(false)
    }

    // ---------------- 激活组内的个体访问（保持既有接口形状） ----------------

    fn with_active<T>(&self, f: impl FnOnce(&GroupData) -> T) -> Option<T> {
        let groups = self.groups.read();
        groups.get(&*self.active.read()).map(f)
    }

    pub fn get(&self, identifier: &str) -> Option<AgentDefinition> {
        self.with_active(|g| g.defs.get(identifier).cloned()).flatten()
    }

    /// 按能力检索：域、能力标签、名称或 identifier 命中（激活组）
    pub fn find_by_capability(&self, cap: &str) -> Vec<AgentDefinition> {
        let needle = cap.to_lowercase();
        let mut out: Vec<AgentDefinition> = self
            .with_active(|g| {
                g.defs
                    .values()
                    .filter(|d| {
                        d.domain.contains(cap)
                            || d.capabilities.iter().any(|c| c.to_lowercase().contains(&needle))
                            || d.name.contains(cap)
                            || d.identifier.to_lowercase().contains(&needle)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        out
    }

    /// 当前交互范围的个体集合（单体模式 = 单体列表；组模式 = 组编成）
    pub fn list_active(&self) -> Vec<AgentDefinition> {
        if self.single_mode() {
            self.list_singles()
        } else {
            self.list()
        }
    }

    pub fn list(&self) -> Vec<AgentDefinition> {
        self.with_active(|g| {
            let mut v: Vec<AgentDefinition> = g.defs.values().cloned().collect();
            v.sort_by(|a, b| a.identifier.cmp(&b.identifier));
            v
        })
        .unwrap_or_default()
    }

    // ---------------- 单体智能体（独立交互对象，docs/09 §8） ----------------

    /// 当前是否处于单体交互模式
    pub fn single_mode(&self) -> bool {
        self.active_single.read().is_some()
    }

    pub fn active_single(&self) -> Option<String> {
        self.active_single.read().clone()
    }

    /// 当前交互范围 id：单体模式 = single:<id>（会话归属键），否则 = 激活组
    pub fn active_scope(&self) -> String {
        match &*self.active_single.read() {
            Some(id) => format!("single:{id}"),
            None => self.active_group(),
        }
    }

    pub fn list_singles(&self) -> Vec<AgentDefinition> {
        let mut v: Vec<AgentDefinition> = self.singles.read().values().cloned().collect();
        v.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        v
    }

    pub fn single(&self, id: &str) -> Option<AgentDefinition> {
        self.singles.read().get(id).cloned()
    }

    /// 语言变体提示词路径：组内 prompts/ 或全局 prompts/（单体模式）
    fn variant_path(&self, stem: &str, lang: &str) -> Option<PathBuf> {
        let file = format!("{stem}.{lang}.md");
        if self.single_mode() {
            return Some(if stem.contains('/') || stem.contains('\\') {
                self.singles_dir().join(&file)
            } else {
                self.dir.join("agents").join("prompts").join(&file)
            });
        }
        let groups = self.groups.read();
        let g = groups.get(&*self.active.read());
        let dir = g.map(|g| g.dir.clone()).unwrap_or_else(|| self.dir.clone());
        Some(if stem.contains('/') || stem.contains('\\') {
            dir.join(&file)
        } else {
            dir.join("prompts").join(file)
        })
    }

    fn singles_dir(&self) -> PathBuf {
        self.dir.join("agents")
    }

    fn singles_prompt_path(&self, prompt_file: &str) -> PathBuf {
        if prompt_file.contains('/') || prompt_file.contains('\\') {
            // 新约定：单体目录 entities/agents/<id>/PROMPT.md
            self.singles_dir().join(prompt_file)
        } else {
            self.singles_dir().join("prompts").join(prompt_file)
        }
    }

    /// 单体定义文件：entities/agents/<id>/agent.json
    fn single_def_path(&self, id: &str) -> PathBuf {
        self.singles_dir().join(id).join("agent.json")
    }

    /// 读取单体智能体提示词
    pub fn load_single_prompt(&self, prompt_file: &str) -> anyhow::Result<String> {
        std::fs::read_to_string(self.singles_prompt_path(prompt_file))
            .with_context(|| format!("读取单体提示词失败: {prompt_file}"))
    }

    /// 写入单体提示词文件（PROMPT.md；与激活组状态无关）
    pub fn write_single_prompt(&self, prompt_file: &str, content: &str) -> anyhow::Result<()> {
        let p = self.singles_prompt_path(prompt_file);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, content)?;
        Ok(())
    }

    /// 新增/更新单体智能体（identifier 冲突即替换）
    pub fn upsert_single(&self, mut def: AgentDefinition, prompt: Option<String>) -> anyhow::Result<AgentDefinition> {
        Self::validate_identifier(&def.identifier)?;
        if def.name.trim().is_empty() || def.description.trim().is_empty() {
            anyhow::bail!("name/description 不能为空");
        }
        if def.prompt_file.trim().is_empty() {
            def.prompt_file = format!("{}/PROMPT.md", def.identifier);
        }
        def.domain = normalize_domain(&def.domain);
        let prompt_path = self.singles_prompt_path(&def.prompt_file);
        if let Some(parent) = prompt_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(text) = prompt {
            std::fs::write(&prompt_path, text)?;
        } else if !prompt_path.exists() {
            std::fs::write(
                &prompt_path,
                format!(
                    "# {}

你是 {}，用户的单体智能体：听清意图、亲手做完、回报结果——分析与实施一体完成。

## 语言纪律
- 称用户为\"用户\"，以\"本机\"自称。
- 被问起你是谁：一句话说明名字与职责，随即回到用户的事上。
- 信息不足时显式说明假设，再给出最小验证路径。
",
                    def.name, def.name
                ),
            )?;
        }
        self.singles.write().insert(def.identifier.clone(), def.clone());
        let def_path = self.single_def_path(&def.identifier);
        std::fs::create_dir_all(def_path.parent().unwrap())?;
        std::fs::write(def_path, serde_json::to_string_pretty(&def)?)?;
        Ok(def)
    }

    pub fn remove_single(&self, id: &str) -> anyhow::Result<bool> {
        let removed = self.singles.write().remove(id).is_some();
        if removed {
            // 整目录移除（PROMPT/SOUL/MEMORY 随个体走）
            let _ = std::fs::remove_dir_all(self.singles_dir().join(id));
            let _ = std::fs::remove_file(self.singles_dir().join(format!("{id}.json"))); // 旧布局清理
            if self.active_single() == Some(id.to_string()) {
                self.set_active_single(None)?;
            }
        }
        Ok(removed)
    }

    /// 切换交互目标：Some(单体 id) 进单体模式，None 回到激活组
    pub fn set_active_single(&self, id: Option<&str>) -> anyhow::Result<()> {
        match id {
            Some(x) => {
                if !self.singles.read().contains_key(x) {
                    anyhow::bail!("智能体不存在: {x}");
                }
                *self.active_single.write() = Some(x.to_string());
                std::fs::write(self.dir.join("active_single"), x)?;
            }
            None => {
                *self.active_single.write() = None;
                let _ = std::fs::remove_file(self.dir.join("active_single"));
            }
        }
        Ok(())
    }

    /// 指定组的个体清单（供跨组查看组详情，不依赖激活组）
    pub fn agents_in_group(&self, gid: &str) -> Vec<AgentDefinition> {
        let groups = self.groups.read();
        let Some(g) = groups.get(gid) else { return vec![] };
        let mut v: Vec<AgentDefinition> = g.defs.values().cloned().collect();
        v.sort_by(|a, b| a.identifier.cmp(&b.identifier));
        v
    }

    pub fn units(&self) -> Vec<AgentDefinition> {
        if self.single_mode() {
            return vec![]; // 单体模式不派发子个体
        }
        self.list().into_iter().filter(|d| matches!(d.tier, Tier::Unit)).collect()
    }

    /// 当前交互目标的主智能体：单体模式 = 该单体；否则 = 组内主智能体
    pub fn primary(&self) -> Option<AgentDefinition> {
        if let Some(id) = &*self.active_single.read() {
            if let Some(d) = self.singles.read().get(id) {
                return Some(d.clone());
            }
        }
        let groups = self.groups.read();
        let g = groups.get(&*self.active.read())?;
        let pid = g.meta.primary.clone()?;
        g.defs.get(&pid).cloned()
    }

    /// 冲突裁决个体：按数据声明动态选路（能力含「裁决」优先，其次「决策」，再次名称命中），
    /// 无命中时回退主智能体。代码不指向任何具体 identifier，编成变化不影响裁决链路。
    pub fn arbiter(&self) -> Option<AgentDefinition> {
        let defs = self.list();
        let mut best: Option<(u8, AgentDefinition)> = None;
        for d in defs.iter().filter(|d| !matches!(d.tier, Tier::Orchestrator)) {
            let score = if d.capabilities.iter().any(|c| c.contains("裁决")) {
                3
            } else if d.capabilities.iter().any(|c| c.contains("决策")) {
                2
            } else if d.name.contains("裁决") || d.name.contains("决策") {
                1
            } else {
                0
            };
            if score > 0 && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, d.clone()));
            }
        }
        best.map(|(_, d)| d).or_else(|| self.primary())
    }

    pub fn count(&self) -> usize {
        self.with_active(|g| g.defs.len()).unwrap_or(0)
    }

    /// 指定组的个体数
    pub fn group_agent_count(&self, gid: &str) -> usize {
        self.groups.read().get(gid).map(|g| g.defs.len()).unwrap_or(0)
    }

    // ---------------- 提示词 / Playbook / SOUL（激活组） ----------------

    /// 组目录（groups/<gid>/，MEMORY.md / SOUL.md / agents/ / prompts/ 所在）
    pub fn group_dir(&self, gid: &str) -> Option<PathBuf> {
        self.groups.read().get(gid).map(|g| g.dir.clone())
    }

    /// 单体根目录（entities/agents/，公开供记忆渲染等使用）
    pub fn singles_dir_pub(&self) -> PathBuf {
        self.singles_dir()
    }

    /// 提示词路径解析：`promptFile` 含 `/` = 相对编成根的个体目录路径（新约定 `<id>/PROMPT.md`）；
    /// 否则兼容旧约定 `<编成根>/prompts/<file>`。
    fn prompt_fs_path(&self, prompt_file: &str) -> PathBuf {
        let groups = self.groups.read();
        let g = groups.get(&*self.active.read());
        let dir = g.map(|g| g.dir.clone()).unwrap_or_else(|| self.dir.clone());
        if prompt_file.contains('/') || prompt_file.contains('\\') {
            dir.join(prompt_file)
        } else {
            dir.join("prompts").join(prompt_file)
        }
    }

    pub fn load_prompt(&self, prompt_file: &str) -> anyhow::Result<String> {
        if self.single_mode() {
            if let Ok(text) = self.load_single_prompt(prompt_file) {
                return Ok(text);
            }
        }
        // 语言变体：EXM_LANG=en 时优先 {stem}.{lang}.md（如 <id>/PROMPT.en.md）
        let lang = crate::config::ExmConfig::load_language();
        if lang != "zh" {
            let stem = prompt_file.strip_suffix(".md").unwrap_or(prompt_file);
            let variant = self.variant_path(stem, &lang);
            if let Some(p) = variant {
                if p.exists() {
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        return Ok(text);
                    }
                }
            }
        }
        let p = self.prompt_fs_path(prompt_file);
        std::fs::read_to_string(&p).with_context(|| format!("读取提示词失败: {}", p.display()))
    }

    /// 组内提示词是否存在
    pub fn prompt_exists(&self, prompt_file: &str) -> bool {
        if self.single_mode() {
            return self.singles_prompt_path(prompt_file).exists();
        }
        self.prompt_fs_path(prompt_file).exists()
    }

    /// 写入组内提示词文件（创建/更新个体时使用）
    pub fn write_prompt(&self, prompt_file: &str, content: &str) -> anyhow::Result<()> {
        let p = if self.single_mode() {
            self.singles_prompt_path(prompt_file)
        } else {
            self.prompt_fs_path(prompt_file)
        };
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, content)?;
        Ok(())
    }

    /// 组作用域提示词读取（成员编辑用：不随激活组漂移）
    pub fn load_group_prompt(&self, gid: &str, prompt_file: &str) -> anyhow::Result<String> {
        let dir = self.group_dir(gid).context("组不存在")?;
        let p = if prompt_file.contains('/') || prompt_file.contains('\\') {
            dir.join(prompt_file)
        } else {
            dir.join("prompts").join(prompt_file)
        };
        std::fs::read_to_string(&p).with_context(|| format!("读取提示词失败: {}", p.display()))
    }

    /// 组作用域提示词写入
    pub fn write_group_prompt(&self, gid: &str, prompt_file: &str, content: &str) -> anyhow::Result<()> {
        let dir = self.group_dir(gid).context("组不存在")?;
        let p = if prompt_file.contains('/') || prompt_file.contains('\\') {
            dir.join(prompt_file)
        } else {
            dir.join("prompts").join(prompt_file)
        };
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, content)?;
        Ok(())
    }

    pub fn load_playbooks(&self) -> anyhow::Result<Vec<crate::types::Playbook>> {
        self.playbooks_in_group(&self.active_group())
    }

    /// 指定组的链路模板（组总览用，不依赖激活组）
    pub fn playbooks_in_group(&self, gid: &str) -> anyhow::Result<Vec<crate::types::Playbook>> {
        let groups = self.groups.read();
        let Some(g) = groups.get(gid) else { return Ok(vec![]) };
        let dir = g.dir.join("playbooks");
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        files.sort();
        let mut out = Vec::new();
        for f in files {
            let raw = std::fs::read_to_string(&f)?;
            out.push(
                serde_json::from_str::<crate::types::Playbook>(&raw)
                    .with_context(|| format!("playbook 校验失败: {}", f.display()))?,
            );
        }
        Ok(out)
    }

    // ---------------- 技能包（激活组，docs/10） ----------------

    /// 技能全局目录：`skills/`，全体组共用（技能是动态加载的作业指令，不做组隔离）
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(self.dir.parent().unwrap_or(self.dir.as_path()).join("skills"))
    }

    /// 装载全局技能包（`skills/*.json`，热装载，全体组共用）
    pub fn load_skills(&self) -> anyhow::Result<Vec<crate::types::SkillDef>> {
        let Some(dir) = self.skills_dir() else { return Ok(vec![]) };
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        files.sort();
        let mut out = Vec::new();
        for f in files {
            let raw = std::fs::read_to_string(&f)?;
            out.push(
                serde_json::from_str::<crate::types::SkillDef>(&raw)
                    .with_context(|| format!("技能包校验失败: {}", f.display()))?,
            );
        }
        Ok(out)
    }

    /// 写入技能包到全局目录（id 校验同个体标识）
    pub fn write_skill(&self, skill: &crate::types::SkillDef) -> anyhow::Result<()> {
        let Some(dir) = self.skills_dir() else { anyhow::bail!("无激活组") };
        let id = skill.id.trim();
        if id.is_empty()
            || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!("非法技能 id: {id}（仅允许字母/数字/-/_）");
        }
        if skill.name.trim().is_empty() || skill.instructions.trim().is_empty() {
            anyhow::bail!("name/instructions 不能为空");
        }
        std::fs::create_dir_all(&dir)?;
        let mut skill = skill.clone();
        skill.id = id.to_string();
        std::fs::write(dir.join(format!("{id}.json")), serde_json::to_string_pretty(&skill)?)?;
        Ok(())
    }

    // ---------------- 组导出 / 导入（组是可分享的整体，docs/09） ----------------

    /// 导出组为 bundle：组元信息 + 全部个体定义 + 提示词正文（一条 JSON 可分享）
    pub fn export_group(&self, gid: &str) -> anyhow::Result<serde_json::Value> {
        let groups = self.groups.read();
        let g = groups.get(gid).context("组不存在")?;
        let mut agents: Vec<serde_json::Value> = Vec::new();
        let mut prompts = serde_json::Map::new();
        for d in g.defs.values() {
            agents.push(serde_json::to_value(d)?);
            // 目录制：promptFile 是相对编成根的路径（<id>/PROMPT.md）；旧布局回落 prompts/ 拼接
            let pp = if d.prompt_file.contains('/') || d.prompt_file.contains('\\') {
                g.dir.join(&d.prompt_file)
            } else {
                g.dir.join("prompts").join(&d.prompt_file)
            };
            if let Ok(text) = std::fs::read_to_string(&pp) {
                prompts.insert(d.prompt_file.clone(), serde_json::Value::String(text));
            }
        }
        agents.sort_by(|a, b| {
            let ka = a.get("identifier").and_then(|v| v.as_str()).unwrap_or("");
            let kb = b.get("identifier").and_then(|v| v.as_str()).unwrap_or("");
            ka.cmp(kb)
        });
        Ok(serde_json::json!({
            "schema": "exmachina.group-bundle",
            "version": 1,
            "group": {
                "id": gid,
                "name": g.meta.name,
                "description": g.meta.description,
                "primary": g.meta.primary,
            },
            "agents": agents,
            "prompts": prompts,
        }))
    }

    /// 从 bundle 导入为新组（首个个体或 bundle.primary 自动设立主智能体）
    pub fn import_group(
        &self,
        bundle: &serde_json::Value,
        new_id: Option<String>,
    ) -> anyhow::Result<GroupMeta> {
        anyhow::ensure!(
            bundle.get("schema").and_then(|v| v.as_str()) == Some("exmachina.group-bundle"),
            "bundle 格式不符（缺少 schema 标记）"
        );
        let ginfo = bundle.get("group").context("bundle 缺少 group 字段")?;
        let name = ginfo.get("name").and_then(|v| v.as_str()).unwrap_or("导入组");
        let description = ginfo.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let primary = ginfo.get("primary").and_then(|v| v.as_str()).map(String::from);
        let meta = self.create_group(new_id, name, description)?;
        let agents = bundle
            .get("agents")
            .and_then(|v| v.as_array())
            .context("bundle 缺少 agents")?;
        let prompts = bundle.get("prompts").and_then(|v| v.as_object());
        let mut first: Option<String> = None;
        for av in agents {
            let def: AgentDefinition = serde_json::from_value(av.clone())
                .context("个体定义解析失败")?;
            if first.is_none() {
                first = Some(def.identifier.clone());
            }
            let prompt = prompts
                .and_then(|m| m.get(&def.prompt_file))
                .and_then(|v| v.as_str())
                .map(String::from);
            self.upsert_agent(&meta.id, def, prompt)?;
        }
        let target = primary.filter(|p| {
            self.group_agent_count(&meta.id) > 0
                && self
                    .agents_in_group(&meta.id)
                    .iter()
                    .any(|a| a.identifier == *p)
        }).or(first);
        if let Some(p) = target {
            let _ = self.set_primary(&meta.id, &p);
        }
        Ok(meta)
    }

    // ---------------- 经验优化（子个体自适应，docs/10 §5） ----------------

    fn adaptation_path(&self, identifier: &str) -> Option<PathBuf> {
        self.with_active(|g| {
            if g.defs.contains_key(identifier) {
                // 子个体无独立目录：经验要点集中在组目录 adaptations/
                Some(g.dir.join("adaptations").join(format!("{identifier}.json")))
            } else {
                None
            }
        })
        .flatten()
    }

    /// 读取个体的经验改进要点（激活组内）
    pub fn load_adaptation(&self, identifier: &str) -> anyhow::Result<Option<crate::types::AgentAdaptation>> {
        let Some(p) = self.adaptation_path(identifier) else { return Ok(None) };
        if p.as_os_str().is_empty() || !p.exists() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_str(&std::fs::read_to_string(&p)?)
                .with_context(|| format!("解析经验要点失败: {}", p.display()))?,
        ))
    }

    /// 保存经验要点：旧版本自动入历史（上限 5），updated_at 在此刷新
    pub fn save_adaptation(
        &self,
        mut adapt: crate::types::AgentAdaptation,
    ) -> anyhow::Result<crate::types::AgentAdaptation> {
        let Some(p) = self.adaptation_path(&adapt.identifier) else {
            anyhow::bail!("个体不存在（激活组内）: {}", adapt.identifier);
        };
        if p.as_os_str().is_empty() {
            anyhow::bail!("个体不存在（激活组内）: {}", adapt.identifier);
        }
        if let Some(old) = self.load_adaptation(&adapt.identifier)? {
            adapt.previous = old.previous;
            adapt.previous.insert(
                0,
                crate::types::AgentAdaptationVersion {
                    revision: old.revision,
                    content: old.content,
                    updated_at: old.updated_at,
                },
            );
            adapt.previous.truncate(5);
        }
        adapt.updated_at = crate::types::now_iso();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(&adapt)?)?;
        Ok(adapt)
    }

    pub fn reset_adaptation(&self, identifier: &str) -> anyhow::Result<bool> {
        let Some(p) = self.adaptation_path(identifier) else { return Ok(false) };
        if p.as_os_str().is_empty() || !p.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&p)?;
        Ok(true)
    }

    pub fn remove_skill(&self, id: &str) -> anyhow::Result<bool> {
        let Some(dir) = self.skills_dir() else { anyhow::bail!("无激活组") };
        let path = dir.join(format!("{id}.json"));
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ---------------- 组内个体增删改（主智能体权限 / 用户操作） ----------------

    fn validate_identifier(identifier: &str) -> anyhow::Result<()> {
        if identifier.is_empty()
            || !identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!("非法个体标识: {identifier}（仅允许字母/数字/-/_）");
        }
        Ok(())
    }

    /// 新增/替换个体（组内）。`prompt` 为 Some 时写入提示词文件；identifier 冲突即替换。
    pub fn upsert_agent(
        &self,
        gid: &str,
        mut def: AgentDefinition,
        prompt: Option<String>,
    ) -> anyhow::Result<AgentDefinition> {
        Self::validate_identifier(&def.identifier)?;
        let mut groups = self.groups.write();
        let g = groups.get_mut(gid).context("组不存在")?;

        if def.prompt_file.trim().is_empty() {
            def.prompt_file = format!("{}.md", def.identifier);
        }
        def.domain = normalize_domain(&def.domain);
        if def.name.trim().is_empty() || def.description.trim().is_empty() {
            anyhow::bail!("name/description 不能为空");
        }

        let replacing = g.defs.contains_key(&def.identifier);
        if !replacing && g.meta.builtin {
            anyhow::bail!("内置组的定义受保护，不可新增个体");
        }
        if replacing && g.meta.builtin {
            anyhow::bail!("内置组的定义受保护，不可修改个体");
        }

        // 提示词：显式提供则写入；否则新建时给模板。路径 = 组内 prompts/<promptFile>
        let prompt_path = g.dir.join("prompts").join(&def.prompt_file);
        if let Some(text) = prompt {
            std::fs::create_dir_all(prompt_path.parent().unwrap())?;
            std::fs::write(&prompt_path, text)?;
        } else if !prompt_path.exists() {
            std::fs::create_dir_all(prompt_path.parent().unwrap())?;
            std::fs::write(
                &prompt_path,
                format!(
                    "# {}\n\n你是 EXMACHINA 的{}（identifier: {}），受指挥体直接调度。\n职责：{}\n\n## 语言纪律\n\
                     - 你是受指挥体直接调度的子个体；称主智能体为\"指挥体\"，称用户为\"用户\"。\n\
                     - 每条陈述以句式前缀开头：【肯定】【否定】【疑问】【报告】【提案】【警告】【观测】。\n\
                     - 零情绪：禁止寒暄、感叹、安慰、夸赞、拟人化情绪表达。\n\
                     - 压缩表达：只输出推进任务、降低不确定性、完成验证闭环所需的信息。\n",
                    def.name, def.name, def.identifier, def.description
                ),
            )?;
        }

        // 若替换的是主智能体本体，保持 primary 指向不变
        g.defs.insert(def.identifier.clone(), def.clone());
        let meta_path = g.dir.join("group.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&g.meta)?)?;
        // 持有写锁期间直接落盘（不可再进入二次加锁路径 → 防自锁）；定义落组内 agents/
        let def_path = g.dir.join("agents").join(format!("{}.json", def.identifier));
        std::fs::create_dir_all(def_path.parent().unwrap())?;
        std::fs::write(&def_path, serde_json::to_string_pretty(&def)?)?;
        Ok(def)
    }

    /// 删除个体（自定义组；主智能体不可删）
    pub fn remove_agent(&self, gid: &str, identifier: &str) -> anyhow::Result<()> {
        let mut groups = self.groups.write();
        let g = groups.get_mut(gid).context("组不存在")?;
        if g.meta.builtin {
            anyhow::bail!("内置组的定义受保护，不可删除个体");
        }
        if g.meta.primary.as_deref() == Some(identifier) {
            anyhow::bail!("主智能体不可删除（可先 set_primary 转移角色）");
        }
        let def = g.defs.remove(identifier).context("个体不存在")?;
        let _ = std::fs::remove_file(g.dir.join("agents").join(format!("{identifier}.json")));
        let _ = std::fs::remove_file(g.dir.join("prompts").join(&def.prompt_file));
        let _ = std::fs::remove_file(g.dir.join("adaptations").join(format!("{identifier}.json")));
        Ok(())
    }

    /// 设置组工作区（相对路径相对全局工作区根；空串清除回退全局）
    pub fn set_workspace(&self, gid: &str, workspace: &str) -> anyhow::Result<()> {
        let ws = workspace.trim();
        if ws.contains("..") {
            anyhow::bail!("工作区路径不允许包含 ..");
        }
        let mut groups = self.groups.write();
        let g = groups.get_mut(gid).context("组不存在")?;
        g.meta.workspace = if ws.is_empty() { None } else { Some(ws.to_string()) };
        let meta_path = g.dir.join("group.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&g.meta)?)?;
        Ok(())
    }

    /// 设置组默认模型（"档案ID" 或 "档案ID/模型名"；空串清除 = 跟随全局生效档案）
    pub fn set_group_model(&self, gid: &str, model: &str) -> anyhow::Result<()> {
        let m = model.trim();
        let mut groups = self.groups.write();
        let g = groups.get_mut(gid).context("组不存在")?;
        g.meta.model = if m.is_empty() { None } else { Some(m.to_string()) };
        let meta_path = g.dir.join("group.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&g.meta)?)?;
        Ok(())
    }

    /// 设置组级能力模型覆盖（每项空串 = 清除该项，跟随全局槽位；全空 = 整块收回）
    pub fn set_group_capabilities(&self, gid: &str, caps: crate::types::GroupCapabilities) -> anyhow::Result<()> {
        let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let caps = crate::types::GroupCapabilities {
            speech: norm(caps.speech),
            transcribe: norm(caps.transcribe),
            vision_relay: norm(caps.vision_relay),
            embedding: norm(caps.embedding),
        };
        let mut groups = self.groups.write();
        let g = groups.get_mut(gid).context("组不存在")?;
        g.meta.capabilities = if caps == crate::types::GroupCapabilities::default() { None } else { Some(caps) };
        let meta_path = g.dir.join("group.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&g.meta)?)?;
        Ok(())
    }

    /// 个体定义文件：组内 agents/<id>.json（单体 entities/agents/<id>/agent.json）
    fn def_path_in(g_dir: &Path, identifier: &str) -> PathBuf {
        g_dir.join("agents").join(format!("{identifier}.json"))
    }

    /// 设置个体默认模型（单体优先，其次跨组查找；空串清除 = 跟随所属组/全局）
    pub fn set_agent_model(&self, identifier: &str, model: &str) -> anyhow::Result<()> {
        let m = model.trim();
        let hint = if m.is_empty() { None } else { Some(m.to_string()) };
        // 1) 单体智能体
        if self.singles.read().contains_key(identifier) {
            let mut singles = self.singles.write();
            let Some(def) = singles.get_mut(identifier) else {
                anyhow::bail!("个体不存在: {identifier}");
            };
            def.model_hint = hint;
            let path = self.single_def_path(identifier);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, serde_json::to_string_pretty(def)?)?;
            return Ok(());
        }
        // 2) 组内个体（跨组查找：管理页不依赖激活组）
        let owner: Option<(String, PathBuf, AgentDefinition)> = {
            let groups = self.groups.read();
            groups.values().find_map(|g| {
                g.defs.get(identifier).map(|def| {
                    let mut d = def.clone();
                    d.model_hint = hint.clone();
                    (
                        g.meta.id.clone(),
                        Self::def_path_in(&g.dir, identifier),
                        d,
                    )
                })
            })
        };
        let Some((gid, path, def)) = owner else {
            anyhow::bail!("个体不存在: {identifier}");
        };
        std::fs::write(&path, serde_json::to_string_pretty(&def)?)?;
        if let Some(g) = self.groups.write().get_mut(&gid) {
            g.defs.insert(identifier.to_string(), def);
        }
        Ok(())
    }

    /// 更新个体可编辑字段（单体优先，其次跨组查找）——与 set_agent_model 同构。
    /// 保护口径：内置组「定义受保护」指不得增删个体或整体替换定义（upsert_agent / remove_agent）；
    /// 本方法与 set_agent_model 同属受限字段编辑，仅放行 AgentPatch 声明的字段，
    /// identifier/tier/prompt_file 与提示词文件恒不可改，落盘回原定义文件。
    pub fn update_agent(&self, identifier: &str, patch: &AgentPatch) -> anyhow::Result<AgentDefinition> {
        // 1) 单体智能体（entities/agents/<id>.json）
        if self.singles.read().contains_key(identifier) {
            let mut d = {
                let singles = self.singles.read();
                singles.get(identifier).context("个体不存在")?.clone()
            };
            patch.apply(&mut d);
            if d.name.trim().is_empty() || d.description.trim().is_empty() {
                anyhow::bail!("name/description 不能为空");
            }
            let path = self.single_def_path(identifier);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, serde_json::to_string_pretty(&d)?)?;
            self.singles.write().insert(identifier.to_string(), d.clone());
            return Ok(d);
        }
        // 2) 组内个体（跨组查找：管理页不依赖激活组；内置组同样放行受限编辑）
        let owner: Option<(String, PathBuf)> = {
            let groups = self.groups.read();
            groups.values().find_map(|g| {
                g.defs
                    .contains_key(identifier)
                    .then(|| (g.meta.id.clone(), g.dir.join(identifier).join("agent.json")))
            })
        };
        let Some((gid, path)) = owner else {
            anyhow::bail!("个体不存在: {identifier}");
        };
        let mut groups = self.groups.write();
        let g = groups.get_mut(&gid).context("组不存在")?;
        let Some(def) = g.defs.get_mut(identifier) else {
            anyhow::bail!("个体不存在: {identifier}");
        };
        patch.apply(def);
        if def.name.trim().is_empty() || def.description.trim().is_empty() {
            anyhow::bail!("name/description 不能为空");
        }
        std::fs::write(&path, serde_json::to_string_pretty(def)?)?;
        Ok(def.clone())
    }

    /// 设置组内主智能体
    pub fn set_primary(&self, gid: &str, identifier: &str) -> anyhow::Result<()> {
        let mut groups = self.groups.write();
        let g = groups.get_mut(gid).context("组不存在")?;
        if !g.defs.contains_key(identifier) {
            anyhow::bail!("个体不存在: {identifier}");
        }
        g.meta.primary = Some(identifier.to_string());
        let meta_path = g.dir.join("group.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&g.meta)?)?;
        Ok(())
    }

    // ---------------- SOUL（灵魂·人格层）：**仅用户面智能体** ----------------
    // 组 = 组根 SOUL.md（即主智能体的人格，组对用户的脸面）；单体 = entities/agents/<id>/SOUL.md。
    // 子个体无人格层（返回 None）：统一智械纪律已在各自系统提示词内。

    /// 组主智能体的 SOUL：组根 SOUL.md。子个体返回 None。
    fn persona_path(&self, identifier: &str) -> Option<PathBuf> {
        let groups = self.groups.read();
        let g = groups.get(&*self.active.read())?;
        (g.defs.contains_key(identifier) && g.meta.primary.as_deref() == Some(identifier))
            .then(|| g.dir.join("SOUL.md"))
    }

    /// 共享默认 SOUL：`agents/default-soul.md`（未定制 SOUL 的用户面智能体兜底），热读取。
    /// 以下常量为该文件不可读时的内置降级文本。
    pub const DEFAULT_PERSONA: &'static str = "以\"智械体\"风格说话：\n\
        - 客观、简洁、零情绪；禁止寒暄、感叹、夸赞与拟人化表达。\n\
        - 陈述结构化：结论先行，要点分条；判断尽可能附带证据等级（A–D）。\n\
        - 术语精确，不使用比喻与修辞；不确定时显式说明置信度。\n\
        - 面向任务：只输出推进任务、降低不确定性所需的信息。";

    pub fn default_soul(&self) -> String {
        std::fs::read_to_string(self.dir.join("default-soul.md"))
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(String::from)
            .unwrap_or_else(|| Self::DEFAULT_PERSONA.to_string())
    }

    /// SOUL 读取：组主智能体 = 组根 SOUL.md（缺省回落 default-soul.md）。
    /// 子个体无人格层：直接返回统一智械纪律（不读文件）。
    pub fn persona(&self, identifier: &str) -> anyhow::Result<String> {
        let Some(path) = self.persona_path(identifier) else {
            return Ok(self.default_soul());
        };
        match std::fs::read_to_string(path) {
            Ok(text) if !text.trim().is_empty() => Ok(text),
            _ => Ok(self.default_soul()),
        }
    }

    pub fn persona_is_custom(&self, identifier: &str) -> anyhow::Result<bool> {
        Ok(self
            .persona_path(identifier)
            .map(|p| p.exists())
            .unwrap_or(false))
    }

    pub fn set_persona(&self, identifier: &str, text: &str) -> anyhow::Result<()> {
        // 仅用户面智能体（组主智能体）可设：写入组根 SOUL.md
        let path = self.persona_path(identifier).ok_or_else(|| {
            anyhow::anyhow!("子个体无人格层（SOUL 仅限主智能体与单体）：{identifier}")
        })?;
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("SOUL 内容不能为空（如需恢复默认请使用 reset）");
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, format!("{text}\n"))?;
        Ok(())
    }

    pub fn reset_persona(&self, identifier: &str) -> anyhow::Result<bool> {
        let Some(path) = self.persona_path(identifier) else {
            return Ok(false);
        };
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ---------------- 单体智能体 SOUL（entities/agents/<id>/SOUL.md，同语义） ----------------

    fn single_persona_path(&self, id: &str) -> Option<PathBuf> {
        self.singles
            .read()
            .contains_key(id)
            .then(|| self.singles_dir().join(id).join("SOUL.md"))
    }

    pub fn single_persona(&self, id: &str) -> anyhow::Result<String> {
        let path = self.single_persona_path(id).context("单体不存在")?;
        match std::fs::read_to_string(path) {
            Ok(text) if !text.trim().is_empty() => Ok(text),
            _ => Ok(self.default_soul()),
        }
    }

    pub fn single_persona_is_custom(&self, id: &str) -> anyhow::Result<bool> {
        let path = self.single_persona_path(id).context("单体不存在")?;
        Ok(path.exists())
    }

    pub fn single_set_persona(&self, id: &str, text: &str) -> anyhow::Result<()> {
        // 写入恒落到单体目录 entities/agents/<id>/SOUL.md
        let path = self
            .singles
            .read()
            .contains_key(id)
            .then(|| self.singles_dir().join(id).join("SOUL.md"))
            .context("单体不存在")?;
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("SOUL 内容不能为空（如需恢复默认请使用 reset）");
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, format!("{text}\n"))?;
        Ok(())
    }

    pub fn single_reset_persona(&self, id: &str) -> anyhow::Result<bool> {
        let path = self
            .singles
            .read()
            .contains_key(id)
            .then(|| self.singles_dir().join(id).join("SOUL.md"))
            .context("单体不存在")?;
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
