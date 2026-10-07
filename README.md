# 续桥 · ThreadBridge

在 Android 手机上查看电脑 Codex 对话的用户消息和最终回复，并按原 thread ID 继续对话。Rust 统一提供 Hub、Agent、持久账本、完成采集、用户轮次读取、跨机运维与受限 resume 工作器；Kotlin/Compose 提供手机客户端。Python 仅用于独立集成测试、Android 工具和两个旧路径启动器。

当前 Android 版本为 **0.1.21-test**，核心 CLI 包版本为 **0.1.0**。已有四机接入及指定原对话续聊的运行记录；完整日常使用、24–72 小时稳定性、真机锁屏推送和资源预算仍需验收。指定版本、单次续聊或模拟器通过不能代表所有 Codex 版本均兼容。

## 项目结构

| 目录 | 内容 |
|---|---|
| `crates/threadbridge/` | Rust CLI、Hub、Agent、协议、SQLite、通知和接入适配器 |
| `android/` | Android 应用、JVM 测试、Room schema 和 Gradle Wrapper |
| `scripts/` | 旧 notify/systemd 路径启动器、APK 构建与校验、Android 预览 |
| `tests/integration/` | 合成数据、回环 Hub 与 mock Codex 的收发和恢复测试 |
| `tests/android/` | 专用模拟器的合成数据与实际界面 QA |
| `docs/` | 当前产品约束、架构、模块合同、部署和维护方法 |
| `deploy/` | 不含实际凭证的配置及 systemd/TLS 示例 |
| `artifacts/` | 本机构建输出与验收截图，不提交 Git |
| `local/` | 本机 SDK、运行配置、数据库、备份和签名，不提交 Git |

## 构建与验证

Rust 工具链固定为 1.98.1；Rust 单元回归在 Linux/Windows 运行，独立 Python 集成测试仅使用标准库，CI 覆盖 3.10 与 3.14；Android 使用 JDK 17、SDK 35 和 Gradle Wrapper 8.11.1。

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --locked
python3 tests/run.py --integration-only
```

Android 无签名回归：

```sh
export JAVA_HOME=/path/to/jdk17
export ANDROID_HOME=/path/to/android-sdk
export PATH="$JAVA_HOME/bin:$PATH"
android/gradlew -p android :app:testDebugUnitTest --no-daemon --no-parallel
android/gradlew -p android :app:assembleDebug :app:lintDebug --no-daemon --no-parallel
```

Release APK 使用 `scripts/build-apk.sh`，需要原签名文件与密码环境变量，输出 `artifacts/ThreadBridge-0.1.21.apk` 和 SHA-256。安装包通过 GitHub Releases 单独分发；源码仓库不包含 APK、实际服务配置或签名材料。覆盖升级必须沿用原签名。

## 使用与维护

- [产品约束](docs/产品约束.md)与[架构入口](docs/ARCHITECTURE.md)
- [构建、安装、配对与恢复](docs/运行与安装.md)
- [多机接入与统一启停](docs/多机接入.md)
- [Codex 接入边界](docs/modules/Codex接入.md)、[通信与存储](docs/modules/通信与存储.md)、[Android 客户端](docs/modules/Android客户端.md)
- [开发、验证与 GitHub 发布](docs/开发与发布.md)及[测试入口](tests/README.md)

默认不会开启未验证的写能力。队列提交与 Codex 执行分开显示；不确定发送保持 unknown，不能靠重发消除。新对话采集使用持久 collection 策略；删除操作只影响 ThreadBridge 副本，保留原 Codex 记录。

0.1.19 修正每个设备项目的预览与“展开全部”入口；读完退出未读预览的对话仍可展开访问，一周本机清理策略保留。图片使用可双指/双击缩放的全屏查看器。虚拟滚动现状和已落实的图片/分组优化见 [滚动与图片性能评估](docs/滚动与图片性能评估.md)。

0.1.20 修复临时公网入口失效后的已验证地址迁移和本机打包配置遗漏；WebSocket 受限时保留 HTTP 同步并显示明确连接诊断，覆盖升级保留原配对与本机数据。

0.1.21 加入设置中的地址与手机访问密钥验证保存，并支持首次使用密钥连接；固定 cpolar 子域名的独立持久隧道配置与验收见 [运行与安装](docs/运行与安装.md#固定-cpolar-入口与手动配置)。实际部署域名和账号令牌不提交源码。
