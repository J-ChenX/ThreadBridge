# 贡献指南

欢迎提交可复现的问题、文档修正和 Pull Request。较大的协议、数据库或接入方式调整，先开 Issue 说明问题、预期行为和验收方法，便于确认项目边界。

## 开始开发

1. Fork 仓库并克隆，创建用途明确的分支。
2. 按 [开发与发布](docs/开发与发布.md)准备固定 Rust 工具链、Python、JDK 17 和 Android SDK 35。
3. 阅读 [产品约束](docs/产品约束.md)、[架构](docs/ARCHITECTURE.md)以及涉及模块的合同。
4. 实现范围集中的改动，修复问题时补充能验证行为的回归测试。

常规开发不需要发布签名。Android 使用 debug 包，与正式安装的包名分离；不要索取或上传维护者的 keystore。

## 提交前验证

Rust 和服务端改动：

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --locked
python3 tests/run.py --integration-only
```

Android 改动：

```sh
android/gradlew -p android :app:testDebugUnitTest --no-daemon --no-parallel --console=plain
android/gradlew -p android :app:assembleDebug :app:lintDebug --no-daemon --no-parallel --console=plain
```

测试只能使用合成数据、临时数据库和隔离服务。不要把测试连接到真实 Hub 或发送真实 Codex 任务。涉及界面、迁移或真机行为时说明实际验证环境和未验证范围。文档改动检查路径、链接、命令和示例，不要求运行无关测试。

## 提交 Pull Request

- 说明触发问题、改动后的行为及验证结果；需要截图时使用合成数据并去除私密信息。
- 更新行为涉及的模块文档，保留 Cargo.lock、Gradle Wrapper 和 Room schemas。
- 不提交 `local/`、`artifacts/`、构建产物、日志、数据库、服务凭证、实际地址或签名材料。
- 不以 fork 或新 ID 替换原对话续聊；不自动重发 unknown 结果；不绕过采集范围和删除策略。
- CI 通过后由维护者审核；依赖更新不自动合并。

提交贡献表示你有权按本项目的 [MIT 许可证](LICENSE)提供该改动。请遵守 [行为准则](CODE_OF_CONDUCT.md)。漏洞报告请遵循 [安全策略](SECURITY.md)，不要在公开 Issue 中披露凭证或攻击细节。
