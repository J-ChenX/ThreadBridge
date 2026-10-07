# 续桥 · ThreadBridge

[![CI](https://github.com/J-ChenX/ThreadBridge/actions/workflows/ci.yml/badge.svg)](https://github.com/J-ChenX/ThreadBridge/actions/workflows/ci.yml)
[![Release APK](https://github.com/J-ChenX/ThreadBridge/actions/workflows/release.yml/badge.svg)](https://github.com/J-ChenX/ThreadBridge/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Android 8.0+](https://img.shields.io/badge/Android-8.0%2B-green.svg)](https://github.com/J-ChenX/ThreadBridge/releases)

**把电脑上的 Codex 对话接到 Android 手机上，查看进展并继续原对话。**

ThreadBridge 是面向个人的自部署工具：电脑运行 Hub 和接入组件，手机安装 APK 并配对。可连接多台电脑，按设备、项目组织对话，读取用户消息、图片、助手过程说明和最终回复，并按原 thread ID 继续对话。项目独立维护，与 OpenAI 官方产品无隶属关系。

当前 Android 版本为 **0.1.21-test**，核心 CLI 包版本为 **0.1.0**。已有四机接入及指定原对话续聊的运行记录；完整日常使用、24–72 小时稳定性、真机锁屏推送和资源预算仍需验收。指定版本、单次续聊或模拟器通过不能代表所有 Codex 版本均兼容。

[下载 APK](https://github.com/J-ChenX/ThreadBridge/releases) · [安装与配置](docs/运行与安装.md) · [报告问题](https://github.com/J-ChenX/ThreadBridge/issues/new/choose) · [参与贡献](CONTRIBUTING.md)

## 可以做什么

- 查看对话正文和图片，展开过程说明，直接阅读最终回复。
- 按设备 → 项目 → 对话浏览，支持标题搜索、置顶、收藏和未读状态。
- 在原对话继续发送；在已验证启用的设备和项目中创建新对话。
- 离线阅读已缓存内容并保留草稿，联网后由用户发送。
- 使用一次性配对码、二维码或手机访问密钥连接自己的 Hub。

默认不会开启未验证的写能力。队列提交与 Codex 执行分开显示；不确定发送保持 `unknown`，不能靠重发消除。新对话采集使用持久 collection 策略；删除操作只影响 ThreadBridge 副本，保留原 Codex 记录。

当前不支持手机发送附件、运行中追加/打断、远程审批或通知栏回复。它不提供任意终端或远程桌面能力。手机读取的是已授权同步的内容；工具调用、工具输出和内部推理不进入对话列表。

## 下载与开始使用

1. 打开 [Releases](https://github.com/J-ChenX/ThreadBridge/releases)，选择最新测试版，下载 **Assets** 中的 `ThreadBridge-*.apk`；源码压缩包不是安装包。
2. 在 Android 8.0 及以上设备安装。旧版使用同签名 APK 直接覆盖升级；不要通过卸载绕过升级问题。
3. 在电脑构建并配置 ThreadBridge Hub/接入组件，按 [运行与安装](docs/运行与安装.md)生成配对码或手机访问密钥。
4. 手机连接自己的 Hub。公开 APK 不预置个人服务器或凭证；公网使用 HTTPS，HTTP 仅用于显式启用的受信任局域网测试。

**APK 需要电脑端服务配合运行。** 只安装 APK 不会自动部署 Hub，也不会升级电脑服务。首次建议从文档中的 [隔离单机演示](docs/运行与安装.md#隔离单机演示)验证安装和配对，再配置真实 Codex 接入。

下载校验文件并与 APK 放在同一目录，可在支持 `sha256sum` 的电脑上校验：

```sh
sha256sum --check ThreadBridge-0.1.21.apk.sha256
```

## 工作方式

```mermaid
flowchart LR
    C[电脑上的 Codex] --> A[接入组件 / Agent]
    A <-->|鉴权与同步| H[ThreadBridge Hub]
    H <-->|HTTPS / 配对| P[Android 手机]
```

Rust 提供 Hub、Agent、SQLite 存储、完成采集、用户轮次读取、跨机运维与受限 resume 工作器；Kotlin/Compose/Room 提供手机客户端。Python 用于独立集成测试、Android 工具和两个旧路径启动器。详细协议与权限边界见 [架构入口](docs/ARCHITECTURE.md)。

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

Rust 工具链固定为 1.98.1；Rust 单元回归在 Linux/Windows 运行，独立 Python 集成测试仅使用标准库，CI 使用 Python 3.14；Android 使用 JDK 17、SDK 35 和 Gradle Wrapper 8.11.1。

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

Release APK 使用 `scripts/build-apk.sh`，需要原签名文件与密码环境变量，输出 `artifacts/ThreadBridge-0.1.21.apk` 和 SHA-256。推送匹配 `versionName` 的版本标签（例如 `v0.1.21-test`）后，GitHub Actions 自动构建、校验并上传到 [Releases](https://github.com/J-ChenX/ThreadBridge/releases)；首次 Secrets 配置见 [开发与发布](docs/开发与发布.md#自动发布-apk)。源码仓库不包含 APK、实际服务配置或签名材料。覆盖升级必须沿用原签名。

## 使用与维护

- [产品约束](docs/产品约束.md)与[架构入口](docs/ARCHITECTURE.md)
- [构建、安装、配对与恢复](docs/运行与安装.md)
- [多机接入与统一启停](docs/多机接入.md)
- [Codex 接入边界](docs/modules/Codex接入.md)、[通信与存储](docs/modules/通信与存储.md)、[Android 客户端](docs/modules/Android客户端.md)
- [开发、验证与 GitHub 发布](docs/开发与发布.md)及[测试入口](tests/README.md)

## 最近版本

完整公开版本记录见 [CHANGELOG](CHANGELOG.md)，下载和每次发布的提交列表见 [Releases](https://github.com/J-ChenX/ThreadBridge/releases)。

0.1.19 修正每个设备项目的预览与“展开全部”入口；读完退出未读预览的对话仍可展开访问，一周本机清理策略保留。图片使用可双指/双击缩放的全屏查看器。虚拟滚动现状和已落实的图片/分组优化见 [滚动与图片性能评估](docs/滚动与图片性能评估.md)。

0.1.20 修复临时公网入口失效后的已验证地址迁移和本机打包配置遗漏；WebSocket 受限时保留 HTTP 同步并显示明确连接诊断，覆盖升级保留原配对与本机数据。

0.1.21 加入设置中的地址与手机访问密钥验证保存，并支持首次使用密钥连接；固定 cpolar 子域名的独立持久隧道配置与验收见 [运行与安装](docs/运行与安装.md#固定-cpolar-入口与手动配置)。实际部署域名和账号令牌不提交源码。

## 贡献、支持与许可证

Bug 和功能建议使用 [Issue 模板](https://github.com/J-ChenX/ThreadBridge/issues/new/choose)。提交改动前阅读 [贡献指南](CONTRIBUTING.md)和[行为准则](CODE_OF_CONDUCT.md)；使用问题见 [支持说明](SUPPORT.md)。安全漏洞通过 [私密安全报告](https://github.com/J-ChenX/ThreadBridge/security/advisories/new)反馈，详见 [安全策略](SECURITY.md)。

本项目采用 [MIT 许可证](LICENSE)。第三方依赖遵循各自许可证。
