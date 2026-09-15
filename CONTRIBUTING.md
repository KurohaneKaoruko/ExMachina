# Contributing Guide | 贡献指南

Thank you for your interest in EXMACHINA! | 感谢你关注 EXMACHINA！

## Getting Started | 快速上手

```bash
git clone https://github.com/KurohaneKaoruko/ExMachina.git
cd ExMachina
npm install && npm run build:webui
cargo test --workspace
```

All tests passing means you're ready to develop. | 全部测试通过即可开始开发。

## Code Style | 代码风格

- **Rust**: Standard rustfmt defaults; comments in Chinese | 标准 rustfmt 默认配置；注释用中文
- **TypeScript**: React function components + Hooks; state via zustand | React 函数组件 + Hooks；状态用 zustand
- **Commit messages**: `type: description` format (feat / fix / docs / refactor / test / chore) | 用英文写 `类型: 描述` 格式

## Architecture Principles | 架构原则

1. **Contract first | 契约先行** — Update `docs/` contracts before changing data structures | 改动数据结构前先更新契约文档
2. **Zero anthropomorphism | 零拟人化** — AI output follows structured statement protocol | AI 的输出遵循结构化陈述协议
3. **Roster as data | 编制即数据** — Agent definitions live in `agents/`, not hardcoded | 智能体定义放在 `agents/` 目录
4. **Performance first | 性能优先** — Targets low-power devices; avoid heavy dependencies | 面向低性能设备，避免重量级依赖

## Directory Structure | 目录结构

```
crates/exm-core/     # Core runtime (scheduler / contracts / memory / tools) | 核心运行时
crates/exm-gateway/   # HTTP/WS gateway (REST API + WebUI hosting) | 网关
crates/exm-cli/       # CLI (exm / exmachina dual entry) | 命令行工具
webui/                # WebUI (React 19 + antd 6 + Vite)
agents/               # Agent roster data (JSON + Markdown prompts) | 智能体编成数据
docs/                 # Contract docs (single source of truth) | 契约文档
scripts/              # Install / quickstart scripts | 安装/启动脚本
```

## Submitting a Pull Request | 提交 PR

1. Fork this repository | Fork 本仓库
2. Create a feature branch: `git checkout -b feat/your-feature` | 创建特性分支
3. Ensure tests pass: `cargo test --workspace && npm run build:webui` | 确保测试通过
4. Submit a PR with a clear description | 提交 PR 并描述改动

### PR Checklist | PR 检查清单

- [ ] `cargo test --workspace` passes | 测试通过
- [ ] `cargo check --workspace` no warnings | 无警告
- [ ] `npm run build:webui` passes | WebUI 构建通过
- [ ] Data structure changes documented in `docs/` | 数据结构变更已更新文档
- [ ] New config items added to `config_schema()` | 新配置项已入 schema

## Reporting Issues | 报告 Issue

- **Bug report**: Reproduction steps + expected vs actual behavior | 描述复现步骤 + 预期行为 + 实际行为
- **Feature request**: Use case + desired interaction | 描述使用场景 + 期望交互方式
- **Security issues**: Do NOT file publicly — see [SECURITY.md](./SECURITY.md) | 请勿公开提交

## Code of Conduct | 行为准则

- Be respectful to all participants | 尊重所有参与者
- Keep technical discussions focused | 保持技术讨论聚焦
