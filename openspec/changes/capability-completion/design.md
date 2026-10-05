# Design

## Context

现状基线（代码证据见提案阶段的探查结论）：工具面已有 read/edit/grep/glob/filesystem/terminal/web_search/web_fetch/schedule/browser/computer/agent_manage 十二内置工具 + 声明式自定义工具 + MCP 客户端接入；执行安全为"审批三档 + 命令白名单 + bubblewrap/作业对象沙箱 + 路径规范化"分层；通道有九平台适配器（统一 `Channel`/`ChannelRun` 接口）但媒体出站全缺、入站仅 TG 语音；主动行为仅 cron/at + 全局单实例心跳；记忆为 FsDb 倒排 + 混合打分但无模型侧工具；消息为 append-only jsonl 无 undo/分支；MCP 仅客户端；桌面壳仅窗口创建与网关子进程管理。

差距分析（本变更动机的逐项证据）：

| 能力簇 | 已有 | 明确空白 |
|---|---|---|
| 编码工作台 | edit 工具带检查点、工具卡级 diff、只读文件树/预览、变更清单（命令轨迹推断，自认不完备） | patch 工具、版本控制口径的工作区 diff、可编辑保存、git 面板 |
| 通道交互 | 九平台文本收发、TG 语音转写入站、WebUI 语音/图片 | 出站媒体全缺、入站图片/文件缺失、无 typing、一次性全文回复、无用户身份（白名单即全权）、群聊任意消息触发 |
| 主动行为 | 五段 cron + at、全局心跳、调度器 20s tick | 无事件触发源、cron 结果不推送通道、心跳无个体粒度 |
| 记忆/会话 | 混合检索、六类条目、自动归纳写入、检查点、断点续跑 | 模型无记忆工具、无消息 undo/编辑重发/分支 |
| 扩展/治理 | MCP 客户端、声明式自定义工具、token 台账 | 无服务端暴露、无速率限制/配额 |
| 桌面 | Tauri 壳 + 网关捆绑 + PWA | 无托盘/通知/全局快捷键 |

## Goals / Non-Goals

**Goals:**
- 六簇能力全部达到 spec 可验收，且不破坏既有行为（新配置段默认值 = 现状，群聊门控除外——其默认值显式写入配置）
- 全部新增执行路径复用既有安全分层（审批/沙箱/白名单/审计），不出现旁路
- 原创性约束：不引用任何外部同类产品名称（验收见 tasks）

**Non-Goals:**
- 不做语音实时通话、不做平台直播/帖子类 API
- 不做分布式调度补偿（错过触发仍跳过）、不做多网关限流协同（单进程口径）
- 不引入 git2 等重依赖；git 能力经白名单 shell 调用
- 不改 WebUI 构建链与 i18n 框架，不做移动端专门适配

## Decisions

**D1 patch 工具：自实现统一 diff 应用器**
解析标准 unified diff，逐文件校验上下文行（精确匹配，允许行尾空白容差），任一失败整体拒绝；应用按顺序内存暂存后一次性落盘并统一落检查点。备选：引入第三方 patch crate（生态弱、格式约束不可控）或引导模型多次调用 edit（长改动易碎、token 放大）。`ToolName` 枚举新增 `patch`，默认纳入 coding/ops 个体白名单。

**D2 工作区 diff：git --porcelain 经白名单通道**
`status --porcelain` + 逐文件 `diff` 由网关后端经终端白名单同口径执行（不新增进程模型），结果缓存至变更视图刷新。非 git 仓库回退"自最近检查点被写工具触碰文件"（检查点记录已含文件清单，无推断）。备选 git2 库被 Non-Goals 排除。

**D3 编辑器保存走工具同路径**
WebUI 保存 = 网关后端以"写类工具"身份写盘：路径沙箱 `resolve_safe` 同函数、检查点同落点、审计同 jsonl。避免出现第二套写入口径。

**D4 通道媒体：能力矩阵 + 出站器**
`Channel` 接口增加能力声明（`image_out/file_out/voice_out/typing` 布尔位），网关出站器按矩阵选择直发或降级；语音出站复用 TTS 能力模型产物。入站：各适配器下载媒体至 `.exmachina/inbox/<channel>/`，图片进既有 `image_stash` stage，语音走既有 `transcribe`，文件附路径说明。九平台逐一实现并有降级单测。

**D5 身份：配对码 + 档案绑定 + 裁决前置**
配对码由网关签发（短码、TTL 10 分钟、单次有效），绑定关系落 `identities.json`（per-channel 外部 id → 用户档案 + 角色）。裁决点前置于既有审批闸门：先 per-user 角色裁决、再进入 exec_approval。群聊门控在适配器入口归一化"提及/回复"信号后过滤，开关为通道级配置（新装默认开、升级迁移默认关以保持现行为——迁移显式写明）。

**D6 事件触发器与 cron 同路**
文件监听用 `notify`（去抖默认 1s 可配），webhook 事件源独立路径+独立密钥；两者与 cron/at 复用同一"注入会话"执行通路（组切换 + 会话复用 + prompt 注入），不新建执行模型。cron 结果推送 = `CronJob` 增 `notify_channels` 字段，完成事件出站器发送摘要（受限流约束）。

**D7 记忆工具三件套**
`memory_read/memory_write/memory_link` 三工具，薄封装 MemoryStore 既有入口（recall/remember/link），写入标注 `source=agent`；组隔离在工具层强制；默认仅主智能体可见，个体经编成 JSON 的 tools 白名单授权。

**D8 undo/分支：游标快照，不改 jsonl 性质**
每轮次结束记录"轮次快照"（消息游标 + 检查点 id 链）。undo = 空闲校验 → 游标回退 → 上下文重建 → 可选 checkpoint restore → 审计事件 + 归档视图。编辑重发 = undo 至该轮 + 重执行。fork = 新会话复制历史视图 + `parent_session/upto` 元数据。备选：消息级 COW 存储（复杂度高，P1 不做）。

**D9 MCP server：同面复用**
stdio 模式 = CLI 子命令 `exm mcp serve`（网关外独立进程，回连网关 REST/内部调用）；HTTP 模式 = 网关 `/mcp` 挂载。两者共用 `ToolGateway` 的 spec 与执行入口，ACL（组/会话/工具清单）配置化，默认关闭。审计加来源标注。

**D10 限流：令牌桶中间件 + 台账配额**
网关中间件按维度键（`channel:<id>:<user>` / `user:<id>` / `key:<id>`）令牌桶限流；配额读既有 usage 台账计数，超限拒新轮次。热生效：阈值存配置热段（沿用设置页 schema 驱动，保存即生效）。

**D11 桌面：官方插件 + 事件桥**
`tauri-plugin-notification` / `tauri-plugin-global-shortcut` / tray-icon（tauri feature）。壳内 WS 已订阅事件流，审批/完成/cron 事件映射为原生通知；零新事件通路。关窗驻留默认开，托盘退出才终止网关子进程。

## Risks / Trade-offs

- [九平台媒体 API 差异大，出站实现量大] → 能力矩阵先行定义 + 每平台"直发/降级"双路径单测；P0 先落 telegram/discord/slack 三平台，其余 P1
- [MCP server 扩大攻击面] → 默认关闭、ACL 白名单最小暴露、审批沙箱审计全量生效、外部调用独立审计标注
- [undo 与进行中任务竞态] → 空闲强校验，处理中一律拒绝；文件联动回滚仅覆盖检查点口径内的变更
- [文件监听风暴/递归触发] → 去抖 + 会话事件队列上限 + 排除 glob（默认排除 `.exmachina/`、`target/`、`node_modules/`）
- [群聊门控改变现行为] → 升级迁移默认关 + 配置显式开关 + doctor 提示
- [变更体量大] → P0/P1 分期闸门（见 tasks），每期 `cargo test` 全绿 + e2e 冒烟 + `npm run build:webui` 通过才进入下一期
- [限流误伤编排内部调用] → 限流仅作用于外部入口（通道/WebUI/REST/CLI 远程），组内派发不计费不限流

## Migration Plan

1. 配置版本 +1，`migrate_file` 追加全部新段默认值；升级迁移将既有通道的群聊门控显式置为 `off`（保持行为），新装默认 `on`
2. 存储无破坏性变更：轮次快照/绑定关系/触发器均为新增集合，旧数据零迁移
3. 回滚策略：各能力独立开关，整体回滚 = 配置回退上一版本 + 重启；无数据不可逆操作
4. 发布顺序：P0（编码工作台 + 通道身份门控 + 限流基础）→ 验收 → P1（媒体双向、主动行为、记忆工具、undo/分支、MCP server、桌面）

## Open Questions

- 语音出站格式按平台差异（如长音频分段）留实现期按平台文档定，不影响 spec
- 事件 webhook 的载荷映射规则 DSL 形态（JSON Path vs 模板）在实现期定，spec 仅约束"映射可配置"
- 通知定位视图的深链方案（前端路由参数）实现期定
