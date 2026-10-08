# Proposal

## Why

功能面完整后，日常使用的「手感」成为主要短板：切会话丢输入草稿、代码无法一键复制、刷新后离开的页面丢失、过程关键事件（审批/完成）没有浏览器级提醒、会话列表信息密度低（无预览无时间）。这些细节决定平台能否成为日常主力工具，且全部为低风险的前后端小改动，适合一次性批量补齐。

## What Changes

- **对话人体工学（chat-ergonomics）**：输入草稿按会话自动保留；输入框上箭头召回最近一条用户消息；代码块一键复制；消息正文一键复制；会话列表显示最后一条消息预览与相对时间
- **浏览器通知（web-notifications）**：审批请求、轮次完成、定时任务完成三类事件可选触发浏览器通知（需用户授权）；点击通知跳转对应会话；设置页提供按类别开关
- **应用路由与外壳（app-shell-routing）**：视图切换写入 URL hash（`#/chat` 等），刷新后保持在当前页面；支持 `#/chat/<sessionId>` 会话深链
- **视觉统一（visual-polish）**：列表加载改为骨架屏；空状态图标与文案统一；通知/审批等横幅样式统一；滚动条与选中色收口（延续 ux-refinements 的打磨层）

## Capabilities

### New Capabilities

- `chat-ergonomics`: 对话人体工学——草稿保留、消息/代码复制、上箭头召回、会话列表预览与相对时间
- `web-notifications`: 浏览器通知——三类关键事件的系统级提醒、授权开关、点击定位
- `app-shell-routing`: 应用外壳路由——hash 视图路由与会话深链、刷新保持
- `visual-polish`: 视觉统一——骨架屏加载、空状态与横幅样式收口

### Modified Capabilities

（无——均为新增能力的 ADDED 需求）

## Impact

- **webui/src**：`store.ts`（草稿存储、通知触发、路由状态）、`views/ChatView.tsx`（预览/时间/召回/复制）、`components/Markdown.tsx`（代码复制）、`App.tsx`（hash 路由）、`views/SettingsView.tsx`（通知开关）、新增 `components/SkeletonList.tsx`
- **crates/exm-gateway**：无后端改动（通知完全由前端基于既有 WS 事件触发；会话列表预览由既有 messages 接口支撑，仅取每会话最后一条时可复用列表接口扩展 `lastMessage` 字段——`lib.rs` 的 `list_sessions` 附加末条摘要）
- **不在范围内**：Web Push（服务端推送）、移动端原生能力、多标签页状态同步
