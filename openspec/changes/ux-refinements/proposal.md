# Proposal

## Why

能力补齐（capability-completion）落地后，平台功能面已完整，但实际使用暴露出一批**功能缺口与易用性短板**：过程数据（思维链/工具轨迹）不落盘导致刷新即丢、运行中的轮次无法停止只能干等、工具审计与用量统计有 API 无界面、通道与定时任务配置后无法快速验证、`channel:` 前缀的通道会话与普通会话混排难以区分。这些短板让日常使用频频受挫，是当前投入产出比最高的一轮打磨。

## What Changes

- **会话体验（session-ux）**：会话列表支持搜索；`channel:` 前缀的通道会话带平台标识并可一键过滤；会话可导出为 Markdown（含过程数据标记）。
- **过程透明与控制（process-transparency）**：思维链与工具轨迹**服务端落盘**（随消息存储），刷新/换端后仍可展开回看；新增**停止运行中轮次**能力（中断当前执行并落审计事件）。
- **运行可见性（ops-visibility）**：新增**工具审计页**（按个体/工具/时间过滤，回看每次调用的参数与结果摘要）；**用量统计面板回归**（复用既有 `/api/limits/stats`，嵌入设置页用量分类）；审批列表支持跳转到来源会话。
- **接入与自动化易用性（integration-ux）**：通道卡片新增**连通测试**按钮（逐平台探针）；定时任务列表显示**下次执行时间**并提供常用 cron 模板；通道会话内 `/stop` 指令可停止当前轮次。

## Capabilities

### New Capabilities

- `session-ux`: 会话管理易用性——搜索、通道会话标识与过滤、Markdown 导出
- `process-transparency`: 过程透明与控制——思维链/工具轨迹服务端留存、停止运行中轮次
- `ops-visibility`: 运行可见性——工具审计页、用量统计面板、审批到会话的追溯
- `integration-ux`: 接入与自动化易用性——通道连通测试、cron 下次执行与模板、`/stop` 指令

### Modified Capabilities

（无——项目主规格库尚未归档既有能力，本变更全部为新增能力的 ADDED 需求）

## Impact

- **crates/exm-core**：`types.rs`（ChatMessage 增 thinking/toolCalls 可选字段）、`store.rs`（消息携带过程数据；新增轮次取消信号）、`tools.rs`（取消令牌接入工具循环检查点）
- **crates/exm-gateway**：`lib.rs`（消息 API 返回过程数据、停止轮次端点、通道测试端点）、`singles.rs`/`platform.rs`（cron 下次执行计算导出）、通道卡片测试探针（逐平台）
- **webui/src**：`views/ChatView.tsx`（停止按钮、导出、通道会话标识）、`views/AutomationsView.tsx`（下次执行/模板）、`views/ChannelsView.tsx`（测试按钮）、新增 `views/ActivityView` 扩展或独立审计视图、`components/UsageBar`（历史数据渲染复用）
- **不在范围内**：多用户/角色体系、Web Push 通知、思维链跨会话全局搜索、审批流权限改造
