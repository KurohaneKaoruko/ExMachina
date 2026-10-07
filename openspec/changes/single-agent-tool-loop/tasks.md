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

- [x] 3.1 轮次模式入口确认:选择器仅存在于 API 层,WebUI 无该项;单体下 set_session_mode 已被消费忽略(2.4),语义目标达成
- [x] 3.2 过程呈现确认:直接循环复用 orchestrator.token/thinking 事件与 round_trace 留痕,ChatView 无单体特判,零改动

## 4. 测试与验收

- [x] 4.1 Mock 通道验证:补确定性分支(「触发工具循环」→ 文本协议工具调用 → 回灌收敛),与既有用例零碰撞
- [x] 4.2 冒烟用例更新:改名「单体直接工具循环_目标切换与会话归属」,断言无任务图 + 消息归属单体 + 单轮收敛
- [x] 4.3 新增用例「单体直接循环_工具回灌与过程留痕」:terminal 调用 → 回灌 → 收敛,断言轨迹留痕;顺带修复存量 store 持久化 bug(put→jsonl 原位替换)与 execute_named 轨迹缺失
- [x] 4.4 回归:冒烟 21/22(唯一失败为无 CDP 浏览器的环境用例);修复 dev 存量测试编译债后 workspace 全编译,lib 层暴露 3 个与本变更无关的存量失败(记忆/MCP ACL,dev 上从未跑过)——记录为后续债务
- [x] 4.5 部署实测:真实模型(MiniMax)两轮验证——第一轮 read 不存在路径 → 诚实上报 ⚠ 异常并 ⏸ 待确认;第二轮多轮上下文延续,read 真实文件 → 组名/主智能体/内置状态全部正确,轨迹留痕,机械协议(▸/表格)输出
