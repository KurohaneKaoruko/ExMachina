# 安全策略

## 报告漏洞

如果你发现安全漏洞，请**不要**在公开 Issue 中提交。

通过 [GitHub Security Advisories](https://github.com/KurohaneKaoruko/ExMachina/security/advisories/new) 私密报告，或在邮件中联系仓库所有者。

我们会在收到报告后 48 小时内响应。

## 支持版本

| 版本 | 支持状态 |
|------|---------|
| dev  | ✅ 活跃开发 |
| 其他  | ❌ 不支持 |

## 安全特性

- **终端命令审批**：可配置 off / risky / always 三档闸门 + 前缀白名单
- **后台鉴权**：`authKey` 非空时 REST/WS 全量要求 X-Auth-Key
- **会话白名单**：通道可声明 allowedChats 限制交互来源
- **审计日志**：全部工具调用落库可查
- **密钥安全**：API Key 不回显（掩码）、不进 git、存储在本地 `.exmachina/config.json`

## 已知限制

- 分布式执行节点（exm worker）当前使用纯 WebSocket（无 TLS），仅适合可信局域网
- WebUI 默认无 TLS，建议部署在反向代理（nginx/caddy）后启用 HTTPS
