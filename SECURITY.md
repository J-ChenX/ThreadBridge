# 安全策略

## 支持范围

项目处于测试阶段，维护重点为最新发布版本及 `main` 分支，不提供旧版本长期维护或响应时间保证。修复发布后，请同步更新 APK 和涉及的电脑端组件。

## 私密报告漏洞

请通过 GitHub 的 [Report a vulnerability](https://github.com/J-ChenX/ThreadBridge/security/advisories/new)提交私密报告。说明受影响版本、前提条件、复现步骤、预期影响和建议修复方法；复现材料使用合成数据。

不要把访问密钥、Codex 对话正文、真实数据库、签名材料或可利用细节放入公开 Issue。意外泄露的凭证应先撤销或轮换，再处理公开副本；删除一条消息不等于凭证已失效。

普通安装问题和不涉及安全的 Bug 使用 [Issue 模板](https://github.com/J-ChenX/ThreadBridge/issues/new/choose)。

## 部署与信任边界

- Hub、执行电脑、手机以及 TLS 终止处均属于部署者需要管理的信任范围。
- 公网使用有效 HTTPS 和应用鉴权；局域网 HTTP 测试仅限受信任网络。
- 隧道域名使用 HTTPS 不代表中继无法读取正文，需核验 TLS 终止位置。
- 访问密钥、keystore、数据库和备份保存在受保护路径，不提交 Git。
- 续聊和新建依赖目标 Codex 版本及权限验证；unknown 发送不能自动重试。

配置、备份和恢复方法见 [运行与安装](docs/运行与安装.md)，接入限制见 [Codex 接入](docs/modules/Codex接入.md)。
