# 贡献指南

感谢你关注 EXMACHINA！本文档指导你如何参与贡献。

## 快速上手

```bash
git clone https://github.com/KurohaneKaoruko/ExMachina.git
cd ExMachina
npm install && npm run build:webui
cargo test --workspace
```

全部测试通过即可开始开发。

## 开发规范

### 代码风格

- **Rust**：遵循标准 Rust 风格（rustfmt 默认配置）；注释用中文
- **TypeScript**：React 函数组件 + Hooks；状态用 zustand
- **提交信息**：用英文写 `type: description` 格式（feat / fix / docs / refactor / test / chore）

### 架构原则

1. **契约先行**：改动数据结构前先更新 `docs/` 中的契约文档
2. **零拟人化**：AI 的输出遵循结构化陈述协议，不要引入情绪化表达
3. **编制即数据**：智能体定义放在 `agents/` 目录，不是硬编码
4. **性能优先**：本项目面向低性能设备，避免引入重量级依赖

### 目录结构

```
crates/exm-core/     # 核心运行时（调度 / 契约 / 记忆 / 工具网关）
crates/exm-gateway/   # HTTP/WS 网关（REST API + WebUI 托管）
crates/exm-cli/       # 命令行工具（exm / exmachina 双入口）
webui/                # WebUI（React 19 + antd 6 + Vite）
agents/               # 智能体编成数据（JSON + Markdown 提示词）
docs/                 # 契约文档（唯一事实源）
scripts/              # 安装/启动脚本
```

## 提交 Pull Request

1. Fork 本仓库
2. 创建特性分支：`git checkout -b feat/your-feature`
3. 确保测试通过：`cargo test --workspace && npm run build:webui`
4. 提交 PR 并描述改动内容

### PR 检查清单

- [ ] `cargo test --workspace` 通过
- [ ] `cargo check --workspace` 无警告
- [ ] `npm run build:webui` 通过
- [ ] 如涉及数据结构变更，已更新 `docs/` 对应文档
- [ ] 如新增配置项，已加入 `config_schema()`

## 报告 Issue

- **Bug 报告**：描述复现步骤 + 预期行为 + 实际行为
- **功能建议**：描述使用场景 + 期望的交互方式
- **安全问题**：请勿公开提交，参见 [SECURITY.md](./SECURITY.md)

## 行为准则

- 尊重所有参与者
- 保持技术讨论聚焦
