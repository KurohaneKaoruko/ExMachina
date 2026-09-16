//! 个体注册表 —— 多组实现（docs/09）
//!
//! 组（Group）是隔离与切换的基本单位：
//!   - 内置默认组 = `agents/`（definitions/ prompts/ playbooks/ personas/），builtin=true，定义受保护；
//!   - 自定义组   = `agents/groups/<gid>/`（同构布局 + group.json），主智能体拥有组内增删改权限。
//! 切换组即热切换：指挥体的规划/派发/提示词/人设全部落在激活组内。
//! 分布式实现可替换本文件（接口一致）。

use crate::types::{normalize_domain, AgentDefinition, GroupMeta, Tier};
use anyhow::Context;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct GroupData {
    pub meta: GroupMeta,
    pub defs: HashMap<String, AgentDefinition>,
    pub dir: PathBuf,
}

pub struct LocalRegistry {
    dir: PathBuf,
    groups: RwLock<HashMap<String, GroupData>>,
    active: RwLock<String>,
    /// 单体智能体（独立于任何组，直接作为交互对象；agents/singles/）
    singles: RwLock<HashMap<String, AgentDefinition>>,
    /// 当前交互目标：None = 激活组；Some(id) = 单体智能体
    active_single: RwLock<Option<String>>,
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
        std::fs::create_dir_all(dir.join("definitions"))?;
        let reg = LocalRegistry {
            dir,
            groups: RwLock::new(HashMap::new()),
            active: RwLock::new("default".to_string()),
            singles: RwLock::new(HashMap::new()),
            active_single: RwLock::new(None),
        };
        reg.reload()?;
        Ok(reg)
    }

    pub fn reload(&self) -> anyhow::Result<()> {
        let mut groups = HashMap::new();

        // 内置默认组：agents/ 根目录
        let default_meta_path = self.dir.join("group.json");
        let default_meta = if default_meta_path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&default_meta_path)?)
                .context("解析 agents/group.json 失败")?
        } else {
            let meta = GroupMeta {
                id: "default".into(),
                name: "智械集群".into(),
                description: "内置默认组：全连结指挥体 + 子个体集群（定义受保护）".into(),
                primary: Some("exmachina-orchestrator".into()),
                workspace: None,
                model: None,
                capabilities: None,
                builtin: true,
                created_at: crate::types::now_iso(),
            };
            std::fs::write(&default_meta_path, serde_json::to_string_pretty(&meta)?)?;
            meta
        };
        groups.insert(
            default_meta.id.clone(),
            self.load_group(default_meta, self.dir.clone())?,
        );

        // 自定义组：agents/groups/<gid>/
        let groups_dir = self.dir.join("groups");
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
                groups.insert(meta.id.clone(), self.load_group(meta, gdir)?);
            }
        }
        if groups.is_empty() {
            anyhow::bail!("未装载到任何智能体组");
        }
        if !groups.contains_key("default") {
            anyhow::bail!("缺少内置默认组");
        }

        // 单体智能体：agents/singles/*.json（独立交互对象，不属于任何组）
        let mut singles = HashMap::new();
        let singles_dir = self.dir.join("singles");
        if singles_dir.exists() {
            for entry in std::fs::read_dir(&singles_dir)?.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.extension().map(|x| x == "json").unwrap_or(false) {
                    let def: AgentDefinition = serde_json::from_str(&std::fs::read_to_string(&p)?)
                        .with_context(|| format!("单体定义校验失败: {}", p.display()))?;
                    singles.insert(def.identifier.clone(), def);
                }
            }
        }
        *self.singles.write() = singles;
        let active_single = std::fs::read_to_string(self.dir.join("active_single")).unwrap_or_default();
        let active_single = active_single.trim();
        *self.active_single.write() =
            if active_single.is_empty() { None } else { Some(active_single.to_string()) };

        // 激活组持久化：agents/active_group
        let active_file = self.dir.join("active_group");
        let saved = std::fs::read_to_string(&active_file).unwrap_or_default();
        let active = saved.trim().to_string();
        let active = if groups.contains_key(&active) { active } else { "default".to_string() };

        *self.groups.write() = groups;
        *self.active.write() = active;
        Ok(())
    }

    fn load_group(&self, meta: GroupMeta, dir: PathBuf) -> anyhow::Result<GroupData> {
        let mut defs = HashMap::new();
        let def_dir = dir.join("definitions");
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
        for sub in ["definitions", "prompts", "personas"] {
            std::fs::create_dir_all(gdir.join(sub))?;
        }
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
        if gid == "default" || self.group_meta(gid).map(|m| m.builtin).unwrap_or(false) {
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
            return Some(self.dir.join("singles").join("prompts").join(file));
        }
        self.with_active(|g| g.dir.join("prompts").join(file))
    }

    fn singles_dir(&self) -> PathBuf {
        self.dir.join("singles")
    }

    fn singles_prompt_path(&self, prompt_file: &str) -> PathBuf {
        self.singles_dir().join("prompts").join(prompt_file)
    }

    /// 读取单体智能体提示词
    pub fn load_single_prompt(&self, prompt_file: &str) -> anyhow::Result<String> {
        std::fs::read_to_string(self.singles_prompt_path(prompt_file))
            .with_context(|| format!("读取单体提示词失败: {prompt_file}"))
    }

    /// 新增/更新单体智能体（identifier 冲突即替换）
    pub fn upsert_single(&self, mut def: AgentDefinition, prompt: Option<String>) -> anyhow::Result<AgentDefinition> {
        Self::validate_identifier(&def.identifier)?;
        if def.name.trim().is_empty() || def.description.trim().is_empty() {
            anyhow::bail!("name/description 不能为空");
        }
        if def.prompt_file.trim().is_empty() {
            def.prompt_file = format!("{}.md", def.identifier);
        }
        def.domain = normalize_domain(&def.domain);
        let dir = self.singles_dir();
        std::fs::create_dir_all(dir.join("prompts"))?;
        let prompt_path = self.singles_prompt_path(&def.prompt_file);
        if let Some(text) = prompt {
            std::fs::write(&prompt_path, text)?;
        } else if !prompt_path.exists() {
            std::fs::write(
                &prompt_path,
                format!(
                    "# {}

你是 EXMACHINA 的单体智能体 {}（identifier: {}），独立直接服务于用户，不经指挥体调度。
职责：{}

## 语言纪律
- 称用户为\"用户\"，以\"本机\"自称。
- 独立完成任务职责范围内的全部工作；信息不足时显式说明假设。
",
                    def.name, def.name, def.identifier, def.description
                ),
            )?;
        }
        self.singles.write().insert(def.identifier.clone(), def.clone());
        std::fs::write(dir.join(format!("{}.json", def.identifier)), serde_json::to_string_pretty(&def)?)?;
        Ok(def)
    }

    pub fn remove_single(&self, id: &str) -> anyhow::Result<bool> {
        let prompt_file = self.singles.read().get(id).map(|d| d.prompt_file.clone());
        let removed = self.singles.write().remove(id).is_some();
        if removed {
            if let Some(pf) = prompt_file {
                let _ = std::fs::remove_file(self.singles_prompt_path(&pf));
            }
            let _ = std::fs::remove_file(self.singles_dir().join(format!("{id}.json")));
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

    // ---------------- 提示词 / Playbook / 人设（激活组） ----------------

    pub fn load_prompt(&self, prompt_file: &str) -> anyhow::Result<String> {
        if self.single_mode() {
            if let Ok(text) = self.load_single_prompt(prompt_file) {
                return Ok(text);
            }
        }
        // 语言变体：EXM_LANG=en 时优先 {stem}.{lang}.md（如 exmachina-orchestrator.en.md）
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
        let groups = self.groups.read();
        let g = groups.get(&*self.active.read()).context("无激活组")?;
        let p = g.dir.join("prompts").join(prompt_file);
        std::fs::read_to_string(&p).with_context(|| format!("读取提示词失败: {}", p.display()))
    }

    /// 组内提示词是否存在
    pub fn prompt_exists(&self, prompt_file: &str) -> bool {
        self.with_active(|g| g.dir.join("prompts").join(prompt_file).exists()).unwrap_or(false)
    }

    /// 写入组内提示词文件（创建/更新个体时使用）
    pub fn write_prompt(&self, prompt_file: &str, content: &str) -> anyhow::Result<()> {
        let groups = self.groups.read();
        let g = groups.get(&*self.active.read()).context("无激活组")?;
        let p = g.dir.join("prompts").join(prompt_file);
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

    /// 技能全局目录：`agents/skills/`，全体组共用（技能是动态加载的作业指令，不做组隔离）
    fn skills_dir(&self) -> Option<PathBuf> {
        Some(self.dir.join("skills"))
    }

    /// 装载全局技能包（`agents/skills/*.json`，热装载，全体组共用）
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
            let pp = g.dir.join("prompts").join(&d.prompt_file);
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
                g.dir.join("adaptations").join(format!("{identifier}.json"))
            } else {
                PathBuf::new()
            }
        })
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

        // 提示词：显式提供则写入；否则新建时给模板
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
        // 持有写锁期间直接落盘（不可再进入 persist_def 等二次加锁路径 → 防自锁）
        let def_path = g.dir.join("definitions").join(format!("{}.json", def.identifier));
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
        let _ = std::fs::remove_file(g.dir.join("definitions").join(format!("{}.json", identifier)));
        let _ = std::fs::remove_file(g.dir.join("prompts").join(&def.prompt_file));
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
            let path = self.singles_dir().join(format!("{identifier}.json"));
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
                        g.dir.join("definitions").join(format!("{identifier}.json")),
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

    // ---------------- 人设（说话风格，组内隔离） ----------------

    fn persona_path(&self, identifier: &str) -> Option<PathBuf> {
        let groups = self.groups.read();
        let g = groups.get(&*self.active.read())?;
        if !g.defs.contains_key(identifier) {
            return None;
        }
        Some(g.dir.join("personas").join(format!("{identifier}.md")))
    }

    /// 默认人设：智械体风格（结构化陈述、零情绪、证据分级）
    pub const DEFAULT_PERSONA: &'static str = "以\"智械体\"风格说话：\n\
        - 客观、简洁、零情绪；禁止寒暄、感叹、夸赞与拟人化表达。\n\
        - 陈述结构化：结论先行，要点分条；判断尽可能附带证据等级（A–D）。\n\
        - 术语精确，不使用比喻与修辞；不确定时显式说明置信度。\n\
        - 面向任务：只输出推进任务、降低不确定性所需的信息。";

    pub fn persona(&self, identifier: &str) -> anyhow::Result<String> {
        let path = self
            .persona_path(identifier)
            .context("个体不存在（激活组内）")?;
        match std::fs::read_to_string(path) {
            Ok(text) if !text.trim().is_empty() => Ok(text),
            _ => Ok(Self::DEFAULT_PERSONA.to_string()),
        }
    }

    pub fn persona_is_custom(&self, identifier: &str) -> anyhow::Result<bool> {
        let path = self
            .persona_path(identifier)
            .context("个体不存在（激活组内）")?;
        Ok(path.exists())
    }

    pub fn set_persona(&self, identifier: &str, text: &str) -> anyhow::Result<()> {
        let path = self
            .persona_path(identifier)
            .context("个体不存在（激活组内）")?;
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("人设内容不能为空（如需恢复默认请使用 reset）");
        }
        std::fs::write(&path, format!("{text}\n"))?;
        Ok(())
    }

    pub fn reset_persona(&self, identifier: &str) -> anyhow::Result<bool> {
        let path = self
            .persona_path(identifier)
            .context("个体不存在（激活组内）")?;
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
