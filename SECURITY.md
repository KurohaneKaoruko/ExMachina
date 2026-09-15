# Security Policy | 安全策略

## Reporting a Vulnerability | 报告漏洞

**English**: If you discover a security vulnerability, please **do not** file a public Issue. Report it privately via [GitHub Security Advisories](https://github.com/KurohaneKaoruko/ExMachina/security/advisories/new). We will respond within 48 hours.

**中文**：如果你发现安全漏洞，请**不要**在公开 Issue 中提交。通过 [GitHub Security Advisories](https://github.com/KurohaneKaoruko/ExMachina/security/advisories/new) 私密报告。我们会在 48 小时内响应。

## Supported Versions | 支持版本

| Version | Supported |
|---------|-----------|
| dev | ✅ Active development | 活跃开发 |
| other | ❌ Not supported | 不支持 |

## Security Features | 安全特性

| Feature | Description |
|---------|-------------|
| Terminal command approval | 可配置 off / risky / always 三档闸门 + 前缀白名单 |
| Backend auth | `authKey` 非空时 REST/WS 全量要求 X-Auth-Key |
| Chat allowlist | 通道可声明 allowedChats 限制交互来源 |
| Audit logging | 全部工具调用落库可查 |
| Key safety | API Key 不回显（掩码）、不进 git、存储在本地 `.exmachina/config.json` |

## Known Limitations | 已知限制

- Distributed worker nodes (`exm worker`) currently use plain WebSocket (no TLS) — suitable for trusted LAN only | 分布式执行节点当前使用纯 WebSocket（无 TLS），仅适合可信局域网
- WebUI has no built-in TLS — deploy behind a reverse proxy (nginx/caddy) with HTTPS | WebUI 默认无 TLS，建议部署在反向代理后启用 HTTPS
