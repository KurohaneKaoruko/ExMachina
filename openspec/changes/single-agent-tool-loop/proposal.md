# Proposal

## Why

单体智能体(Machina 及用户新建单体)在对话中走 `plan() → L0 直答` 路径:**整个回合没有任何工具循环**——`chat` 里的单体无法执行 read/edit/terminal 等任何工具,其 PROMPT.md 中的编码工作流、终端纪律完全是"纸上谈兵"。成熟单 agent 实现(opencode、claude code 等)的核心形态正是「系统提示 + 会话历史 + 工具循环 + 自主决策」,本变更把单体补齐到该形态。此前已完成的契约中立化(移除强制 L0/禁派发)只解决了提示词偏向,执行层仍是空缺。

## What Changes

- 新增 `AgentRuntime::chat_execute`:单体专用直接执行循环(系统提示 + 会话历史 + 工具循环 → 最终文本回复),不产出 SyncReport/OrchestratorPlan
- `run_round` 在单体模式下分支到直接执行轮,跳过 plan()/派发语义;轮次模式(direct/full)在单体下统一为工具循环
- 会话历史窗口化注入(store 最近 N 条映射为 ChatMessage),当前用户输入附图片(视觉能力判定沿用规划路径)
- 工具面:按单体自身 allowlist + 自定义工具 + MCP 可见性下发,复用 run_tool_calls(只读并发/写串行)
- 过程可观测对齐现有体验:思考/正文分轨流式、round_trace 留痕、用量记账、停止检查点
- 模型链沿用 `unit_chain_for`(单体专属 → 组默认 → 全局档案)
- **移除**:单体模式对 `SINGLE_CONTRACT` 输出契约的依赖(规划路径不再被单体使用)

## Capabilities

### New Capabilities
- `single-agent-direct-loop`: 单体智能体的直接工具循环执行——对话轮内系统提示 + 历史 + 工具循环,自主决策直至产出最终回复;含历史窗口、图片输入、停止检查点、过程分轨与用量记账

### Modified Capabilities

- 无(现有规划/派发能力仅限组模式,行为不变;`SINGLE_CONTRACT` 常量随之删除,属实现细节)

## Out of Scope

- 组模式的规划/派发/收束流程(保持不变)
- 权限审批中心 UI、渠道扩展(telegram/discord)、会话内 @文件引用——后续候选变更
- 单体自定义模型路由之外的调度策略
