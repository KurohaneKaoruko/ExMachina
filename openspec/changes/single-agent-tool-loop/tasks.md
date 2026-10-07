# Tasks

## 1. 执行器:单体直接工具循环

- [x] 1.1 `AgentRuntime::chat_execute` 骨架:系统提示 + 历史 + 工具 specs 组装,24 步上限循环,「无工具调用的文本」即终止并返回最终文本
- [x] 1.2 工具调用处理:复用 `run_tool_calls`(只读并发/写串行)回灌;文本协议兜底(Mock/不支持原生调用端点)沿用 `parse::as_tool_call`
- [x] 1.3 停止检查点:每步循环头检查 `round_trace::is_cancelled`,终止并带错误返回
- [x] 1.4 用量与模型记录:`record_usage` 累计 prompt/completion 与最终使用的模型名
- [x] 1.5 图片输入:视觉能力三态(直附 / 转述注记 / 忽略注记),与规划路径行为一致

## 2. 编排:单体轮分支与清理

- [x] 2.1 `run_round` 顶部单体分支:调用新的 `single_direct_round`(组模式路径不动)
- [x] 2.2 `single_direct_round`:组装系统提示(PROMPT + SOUL)、历史窗口(list_messages 40 条按角色映射)、模型链(`unit_chain_for`)、流式事件(`orchestrator.token` / `orchestrator.thinking`)、最终回复 `add_message_full` 落库与 `run.finished` 事件
- [x] 2.3 移除 `SINGLE_CONTRACT` 常量及 plan()/converge() 中的单体分支与残留注释
- [x] 2.4 轮次模式消费:单体下 `take_session_mode` 照常取走并忽略(direct/full 无差异)

## 3. 前端

- [ ] 3.1 ChatView 轮次模式选择器:单体模式下隐藏「集群」项(或置灰并提示)
- [ ] 3.2 过程呈现确认:直接循环的思考/正文分轨、工具折叠栏与组模式复用同一渲染(如有单体特判则移除)

## 4. 测试与验收

- [ ] 4.1 Mock 通道验证:带工具 specs 的请求下 Mock 返回纯文本使循环一轮收敛;不符则在 Mock 补最简行为
- [ ] 4.2 冒烟用例更新:`single_agent_target_switch_and_l0_direct` 改名并断言直接循环语义(无任务图 + 消息归属单体 + 无工具问答单轮收敛)
- [ ] 4.3 新增工具循环用例:Mock 下产生工具调用 → 回灌 → 收敛,断言工具轨迹与用量留痕
- [ ] 4.4 回归:组模式规划/派发/收束冒烟全绿;`cargo test --workspace` 与 `npm run build:webui` 通过
- [ ] 4.5 部署实测:Machina 对话执行真实工具任务(如读文件并总结),过程分轨与最终回复符合预期
