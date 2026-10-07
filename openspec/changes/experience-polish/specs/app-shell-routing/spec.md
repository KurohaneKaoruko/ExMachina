# Spec Delta

## Purpose

应用外壳路由：视图与会话状态进入 URL（hash），刷新/分享/后退不再丢位置。

## ADDED Requirements

### Requirement: hash 视图路由

侧栏切换视图 SHALL 同步写入 URL hash（如 `#/groups`）；页面加载时 SHALL 按 hash 恢复视图，非法 hash 回落到默认视图（对话）；浏览器前进/后退 SHALL 在视图间导航。

#### Scenario: 刷新保持视图

- **WHEN** 用户停留在「智能体组」页并刷新
- **THEN** 刷新后仍停留在智能体组视图

#### Scenario: 前进后退

- **WHEN** 用户在多个视图间切换后点浏览器后退
- **THEN** 返回上一个视图

### Requirement: 会话深链

会话视图 SHALL 支持 `#/chat/<sessionId>` 形式的深链：加载时选中该会话；会话不存在时 SHALL 回落到对话视图的最近会话并给出提示。

#### Scenario: 深链打开会话

- **WHEN** 用户以 `#/chat/<存在的会话id>` 打开应用
- **THEN** 应用直接进入对话视图并选中该会话

#### Scenario: 深链会话不存在

- **WHEN** 深链中的会话 id 不存在
- **THEN** 回落到对话视图并选中最近会话
