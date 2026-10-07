# Design

## Context

单体模式现状:`run_round → plan()` 以 OrchestratorPlan JSON 契约驱动,`L0 || nodes 为空` 时直接把 `plan.final_answer` 作为回复落库——**全程无工具循环**。子个体的执行器 `AgentRuntime::execute` 已具备完整工具循环(工具 specs 下发、只读并发/写串行、SyncReport 契约校验、多 Key 粘性、流式分轨),但其用户消息形态与 SyncReport 输出契约为派发场景专用,不能直接复用于单体对话。

约束:
- 单体无人格层之外的地位差异:`primary()` 单体模式返回单体,`load_prompt` 单体模式优先读 `entities/agents/<id>/PROMPT.md`——SOUL 拼接逻辑与现规划路径一致
- 现有过程体验依赖 `orchestrator.token` / `orchestrator.thinking` 事件与 `round_trace`(思考/工具轨迹留存),WebUI 已按此渲染
- 冒烟测试 `single_agent_target_switch_and_l0_direct` 断言单体对话不产生任务图、消息归属单体——直接循环天然满足,但用例需补充工具循环场景

## Goals / Non-Goals

**Goals:**
- 单体对话获得与成熟单 agent 一致的自主工具循环(一次实现,单体与未来复用方受益)
- 与组模式共用同一执行底座(runtime),避免两套工具循环实现漂移
- 过程可观测、用量、停止、审批闸门与既有体验零回归

**Non-Goals:**
- 不改变组模式的规划/派发/收束流程
- 不做单体会话的跨会话压缩/摘要(沿用 ensure_compacted 现状,历史仅做窗口截断)
- 不引入新的 UI 组件(轮次模式选择器对单体隐藏「集群」语义由前端小调整完成)

## Decisions

### D1:在 `AgentRuntime` 新增 `chat_execute`,与 `execute` 并列而非改造它
`execute` 的用户消息形态(DispatchOrder JSON)与输出契约(SyncReport 校验/重写)为派发专用;改造它会波及组派发路径。`chat_execute` 复用私有件(`call_llm`、`run_tool_calls`、`image_note`、`record_usage`),仅替换外围契约:**无 SyncReport 校验**,「无工具调用的文本输出」即为终点。备选「execute 加模式参数」被否:单方法双契约可读性差、回归面大。

### D2:循环上限与终止语义
`max_steps = max(order.constraints.max_steps, 24)` 不适用(无 order),chat_execute 固定上限 **24 步**;超限报错文案「超过最大步数仍未完成回复」,由调用方落系统消息。停止检查点沿用 `round_trace::is_cancelled`,每步循环头检查。

### D3:历史窗口 = 最近 40 条存储消息按角色映射
`store.list_messages(session_id, 40)` → `User → user`、`Orchestrator → assistant`(statements 拼接),`Unit/System` 消息跳过。本轮用户输入已在上一步落库,窗口天然含本轮;40 条为经验初值,后续可配置化(不阻塞本变更)。备选「摘要 + 窗口」被否:单体会话摘要压缩是独立能力,不搭车。

### D4:流式与留痕——中间步文本可见,最终步落库
每步文本增量按 `orchestrator.token` 可见流式(与收束阶段一致),思维增量走 `orchestrator.thinking` 并入 round_trace;循环收敛后的最终文本以 `add_message_full` 落库(附 drain 出的思维链与工具轨迹)。工具调用的过程留痕沿用 round_trace 的工具记录。备选「中间步藏入思维轨」被否:单体对话用户期待看到工作过程(与 opencode 一致)。

### D5:run_round 分支点在最前,单体模式完全不进 plan()
`if registry.single_mode() { return single_direct_round(...) }`。轮次模式 `take_session_mode` 照常消费(direct/full 在单体下无差异,选择器由前端对单体隐藏「集群」项);`SINGLE_CONTRACT` 常量与规划路径中的单体分支删除。

### D6:模型链与图片
链 = `unit_chain_for(def)`(单体专属 model_hint → 组默认 → 全局);图片沿用规划路径的三态:视觉能力直附 / 转述模型转写 / 明确无视觉则忽略并注记。

## Risks / Trade-offs

- **Mock 通道兼容**:冒烟用 Mock Provider,需确认 Mock 在带工具 specs 的请求下返回纯文本(无工具调用)使循环一轮收敛——若 Mock 行为不符,在 Mock 侧补最简工具调用样例
- **中间叙述文本 vs 落库不一致**:流式可见的中间叙述在重载后消失(仅最终文本落库)——接受,与主流单 agent 应用一致;后续可为中间叙述引入独立消息类型
- **40 条窗口的 token 上限**:长消息场景仍可能超窗——接受初版截断策略,超限属模型侧报错,可观测
- **单体长任务与渠道超时**:渠道(qqbot)侧如依赖本轮同步回包,长循环需依赖既有异步事件推送——沿用现状,不新增机制
