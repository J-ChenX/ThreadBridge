# 测试入口

测试使用合成身份、临时 SQLite、回环监听和 mock Codex，不发送真实任务消息。

| 位置 | 范围 |
|---|---|
| `crates/threadbridge/src/` | Rust 协议、持久化、鉴权、发送恢复与隔离 proxy fixture |
| `tests/python/` | 完成捕获、用户消息、健康、collection、多机与 resume 单元回归 |
| `tests/integration/` | 真实回环 HTTP/WS、queue/resume mock、独立收件、关联和重启 |
| `android/app/src/test/` | Android JVM 同步与显示策略 |
| `tests/android/` | 专用 emulator-5554 和合成 Hub 的实际界面测试 |

```sh
cargo build --release --locked
python3 tests/run.py
# 可只运行一层
python3 tests/run.py --unit-only
python3 tests/run.py --integration-only --binary target/release/threadbridge
```

CI 在 Python 3.10 与 3.14 分别执行完整回归。捕获 CLI 和 mock 子进程使用当前解释器，而非固定 `/usr/bin/python3`；部署机的 Python 路径仍由其服务配置决定。

installed proxy 的 Rust fixture 默认忽略，它依赖本机 Codex CLI；单独运行仍只操作隔离 socket。常规 CI 不依赖该 CLI。

## Android 像素预览

需自己的 Android SDK 和专用 `threadbridge-test` AVD；当前预览脚本使用 `local/android-sdk/platform-tools/adb`。服务与 fixture 只监听 loopback，预览固定 1200 × 2670、480 dpi、手势导航。准备 release 二进制和专用模拟器，再按顺序运行：

```sh
local/android-sdk/emulator/emulator -avd threadbridge-test -no-window -no-audio -no-snapshot -gpu swiftshader_indirect -port 5554
python3 tests/android/qa_android_fixture018.py
# fixture 持续提供合成 Hub，在另一终端启动预览
python3 scripts/android_preview.py
```

打开 `http://127.0.0.1:8898`。在专用模拟器安装自己的 APK 并配对合成 Hub，运行 `seed_preview_data.py` 填充用于 UI 展示的合成对话；`qa_android_ui018.py` / `qa_android_ui019.py` 依赖已配对 fixture 与相应数据文件，只用于当前兼容界面的回归。输出写入忽略的 `artifacts/`，不作为真实手机验收。

移动测试文件后必须更新 mock 子进程源码路径。集成测试中采用空 HOME/CODEX_HOME 和固定 mock，不沿用真实服务凭证；不要用生产地址替换测试 loopback。
