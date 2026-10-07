# Design

## Context

前端为 React + zustand 单页应用（无路由库，视图切换是 `App.tsx` 里的 `useState`）；后端会话列表接口 `GET /api/sessions` 返回会话元数据但不含末条消息；WS 事件已区分会话（`sessionId`）且事件类型齐全（审批/收束/定时任务完成），但前端只在应用内呈现；Markdown 渲染为自研组件（无代码复制按钮）。Rust 层本轮原则上不动，唯一例外：会话列表接口附加末条消息预览字段。

## Goals / Non-Goals

**Goals:**
- 草稿、路由状态、通知开关全部本地持久化（localStorage / zustand persist），刷新与重启浏览器不丢
- 通知基于既有 WS 事件，零后端推送依赖；前台当前会话不弹
- 会话列表一次请求同时带回末条预览（避免 N+1 逐会话拉消息）
- 全部新 UI 走既有视觉语言（等宽辅助字、2px 圆角、强调色系）

**Non-Goals:**
- URL 路由库（react-router）——只做 hash 同步，不引入依赖
- 服务端通知推送 / 离线推送
- 消息级分页与虚拟滚动（列表量级暂不需要）
- 国际化新增语言

## Decisions

**D1 草稿按会话存内存 + sessionStorage**
`store.drafts: Record<sessionId, { text, imagesCount }>`——切会话时把当前输入写入 drafts，选中会话时回填；同时镜像到 sessionStorage（刷新不丢，跨标签不同步是可接受边界）。附件为 data URL 体积大，仅保留文本草稿（附件丢失在输入区有计数提示兜底）。备选 localStorage 被否：多标签互相覆盖草稿的风险大于收益。

**D2 hash 路由 = 视图状态单向同步到 URL**
`App.tsx` 的 `view` state 为唯一事实源：`setView` 时同步 `location.hash = "#/" + view`；加载时从 hash 读初始视图（白名单校验，非法回落 chat）；`hashchange` 事件驱动回退/前进。会话深链 `#/chat/<sessionId>`：解析后存为「待选会话」，`init()` 完成后选中；不存在则回落最近会话。不引入 react-router（视图数量少、无嵌套路由需求）。

**D3 通知 = WS 事件 + Notification API + localStorage 偏好**
`web-notifications` 完全在前端实现：`store.handleEvent` 中对 `approval.required` / `run.finished` / `cron.finished` 三类事件，在「通知已授权 + 总开关开 + 类别开关开 + (页面隐藏 或 事件会话 ≠ 当前会话)」时调用 `new Notification(...)`；`onclick` 聚焦窗口并切换到对应会话。偏好存 localStorage（`exm.notify` JSON）。授权状态用 `Notification.permission`。备选「轮询 + Service Worker」被否：页面内通知不需要 SW，避免 PWA 缓存复杂度。

**D4 会话列表末条预览：`list_sessions` 附加字段**
`lib.rs` 的 `list_sessions` 在返回前批量查询每个会话的最后一条消息（复用 `store.list_messages(id, 1)`，每会话一次微读，会话量级 ≤200 可接受；后续量大再引入末条缓存集合）。响应增 `lastMessagePreview`（末条陈述纯文本，截断 80 字）与 `lastActiveAt`（取 `updated_at`）。前端会话列表渲染预览行 + 相对时间（`相对时间` 为纯前端函数：刚刚/N 分钟前/N 小时前/昨天/N 天前/日期）。

**D5 复制能力统一走 Clipboard API**
`navigator.clipboard.writeText` + 失败降级 `document.execCommand("copy")`（非安全上下文如纯 IP HTTP 时Clipboard API 不可用，降级保证可用）。Markdown 组件为每个代码块包一层容器并注入复制按钮；消息复制按钮放在消息卡头部操作区。

**D6 骨架屏 = 纯 CSS 组件，无库**
`SkeletonList` 组件：N 个灰阶占位条（shimmer 动画纯 CSS），按 `size` 变体适配列表/卡片两种形态。首个接入点是会话列表与个体清单；其余视图后续渐进替换。

## Risks / Trade-offs

- [hash 与视图状态双向同步的回环] → 单向数据流：hash 变化只写 view，view 变化只写 hash，`hashchange` 里比对去重；深链会话在 init 后一次性消费
- [会话列表 N 次末条微读] → 单会话读 1 行 JSONL 成本极低；会话上限 200，总开销可控；量级增长后再引入缓存
- [通知过度打扰] → 默认只开「审批请求」，完成类默认关；前台当前会话静默
- [草稿跨标签不同步] → sessionStorage 语义如实告知（单标签一致）
