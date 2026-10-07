# Proposal

## Why

EXMACHINA 的核心回路（编成、派发、回流、裁决、三账）与安全分层（审批/沙箱/白名单）已经落地，文件读写/编辑/检索/终端、浏览器、Computer Use、MCP 接入、worker 节点、九平台通道、cron 与心跳等工具面完整；但对照成熟的编码智能体与个人智能助理产品，在**编码工作流闭环、通道交互体验、主动行为、自持记忆与历史管控、工具对外暴露与用量治理、桌面集成**六个方向仍有明确空白（详见 design.md 的差距分析），制约其成为日常主力智能体平台。本次变更一次性补齐六簇能力，分 P0/P1 两期落地。

## What Changes

- **编码工作台闭环**：新增 `patch` 工具（统一 diff 应用，含上下文校验与失败回滚）；工作区级变更对比（基于 git 的 diff 快照与变更清单核实，替代"从命令轨迹挑路径"的尽力而为）；WebUI 文件编辑与保存（受路径沙箱与审批闸门约束）；git 面板（状态/分支/日志/暂存，经 terminal 白名单通道执行）
- **通道媒体双向**：出站图片/文件/语音发送（按平台能力矩阵降级为链接或文本摘要）；入站图片/文件解析并入多模态暂存；typing 指示；长回复分段推送
- **通道身份与门控**：配对码绑定外部用户身份；per-user 角色（admin/member）与审批权限继承；群聊 @唤醒门控（仅提及/回复触发执行，白名单内私聊不受限）
- **主动行为增强**：文件监听触发器（workspace 路径匹配 → 注入会话）；通用 webhook 事件源（通道 webhook 之外的事件入口）；cron 任务结果按订阅推送至通道；心跳支持 per-agent 粒度
- **自持记忆与会话管控**：智能体可调用的记忆检索/写入工具（受组隔离与去重约束）；消息级撤销（undo 恢复到指定轮次，联动既有文件检查点）；用户编辑重发；会话分支派生（fork 历史到新会话）
- **平台扩展与用量治理**：MCP server 模式（把内置工具面按会话/组范围经 stdio/HTTP 暴露给外部 MCP 客户端，复用审批与沙箱）；速率限制与配额（per-channel / per-user / per-api-key 三维度，超限行为可配置）
- **桌面集成**：系统托盘（状态/显隐/退出）、原生通知（审批请求、任务完成、cron 推送）、全局快捷键（唤起/收起窗口）
- **原创性约束（硬性）**：全部能力以原生中性措辞命名；代码、注释、文档、UI 文案与提示词中不得出现任何外部同类参考产品的名称，验收包含禁用词全仓零命中检索（清单见 tasks.md）

## Capabilities

### New Capabilities

- `coding-workbench`: 编码工作流闭环——patch 工具、工作区变更对比、文件编辑保存、git 面板
- `channel-media`: 通道媒体双向传输与实时反馈——出站/入站媒体、typing、分段推送
- `channel-identity`: 通道用户身份与访问治理——配对绑定、角色权限、群聊唤醒门控
- `proactive-triggers`: 事件驱动的主动行为——文件监听、webhook 事件源、cron 推送、per-agent 心跳
- `agent-memory-tools`: 智能体自持记忆——模型可调用的记忆检索/写入/关联工具
- `session-history-control`: 会话历史管控——消息撤销、编辑重发、分支派生
- `mcp-server`: 工具面对外暴露——内置工具以 MCP 协议提供服务
- `usage-limits`: 用量治理——多维速率限制与配额
- `desktop-shell`: 桌面系统集成——托盘、原生通知、全局快捷键

### Modified Capabilities

（无——项目尚无既有 spec，本变更全部为新建能力）

## Impact

- **crates/exm-core**：`types.rs`（ToolName 新增 patch/memory_read/memory_write 等）、`tools.rs`（patch 工具、记忆工具、速率限制钩子）、`store.rs`（消息轮次快照与分支元数据）、`memory.rs`（暴露受控读写入口）、`mcp.rs`（server 模式）、`config.rs`（触发器/配对/配额/推送等新配置段，沿用 schema 驱动 UI 惯例）
- **crates/exm-gateway**：`platform.rs`（媒体出站、typing、门控、配对、cron 推送）、`lib.rs`（新增 REST：undo/分支/文件保存/git 状态/diff、WS 事件扩展）、各通道适配器（媒体发送能力矩阵、入站媒体解析）
- **webui/src**：`CodingView.tsx`（文件编辑器、工作区 diff、git 面板）、`ChatView.tsx`（编辑重发/分支入口）、设置页新增身份与配额管理
- **desktop/src-tauri**：`main.rs`（托盘、通知、全局快捷键，Tauri v2 插件）
- **新依赖**：`notify`（文件监听）；git 操作经 terminal 白名单 shell 调用（不引入 git2 重依赖）；Tauri notification/global-shortcut/tray 插件
- **风险**：九平台媒体 API 差异大，出站能力必须按矩阵降级；MCP server 与网关同进程的端口/stdio 治理；桌面新能力仅 Tauri 壳生效（PWA 不受影响）；本变更体量大，依赖 tasks.md 的分期与验收闸门控制
