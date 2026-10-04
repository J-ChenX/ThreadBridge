# 测试入口

测试使用合成身份、临时 SQLite、回环监听和 mock Codex，不发送真实任务消息。

| 位置 | 范围 |
|---|---|
| `crates/threadbridge/src/` | Rust 协议、持久化、鉴权、采集、用户轮次、collection、健康、跨机/恢复、resume 与隔离 fixture |
| `tests/integration/` | 真实回环 HTTP/WS、queue/resume mock、独立收件、关联和重启 |
| `tests/fixtures/` | Rust 子进程测试共用的固定 mock CLI；每个测试单独保存配置和调用记录 |
| `android/app/src/test/` | Android JVM 同步与显示策略 |
| `tests/android/` | 专用 emulator-5554 和合成 Hub 的实际界面测试 |

```sh
cargo build --release --locked
python3 tests/run.py
# 可只运行一层
python3 tests/run.py --unit-only
python3 tests/run.py --integration-only --binary target/release/threadbridge
```

CI 在 Python 3.10 与 3.14 分别执行完整回归。集成测试调用原生 capture/resume CLI；mock 子进程使用所选 Python。Rust 模块单元测试在 Linux/Windows 运行，发送和进程所有权 fixture 只使用隔离模拟器，未证明任意真实 Codex 或四机长期运行。

installed proxy 的 Rust fixture 默认忽略，它依赖本机 Codex CLI；单独运行仍只操作隔离 socket。常规 CI 不依赖该 CLI。

常规 Rust fixture 通过符号链接运行固定 mock 文件，临时目录只写数据，避免并行创建并执行脚本时的 `Text file busy`；mock 使用 PATH 中选定的 Python。

## Android 像素预览

需自己的 Android SDK 和专用 `threadbridge-test` AVD；当前预览脚本使用 `local/android-sdk/platform-tools/adb`。服务与 fixture 只监听 loopback，预览固定 1200 × 2670、480 dpi、手势导航。准备 release 二进制和专用模拟器，再按顺序运行：

```sh
local/android-sdk/emulator/emulator -avd threadbridge-test -no-window -no-audio -no-snapshot -gpu swiftshader_indirect -port 5554
python3 tests/android/qa_android_fixture018.py
# fixture 持续提供合成 Hub，在另一终端启动预览
python3 scripts/android_preview.py
```

打开 `http://127.0.0.1:8898`。在专用模拟器安装自己的 APK 并配对合成 Hub，运行 `seed_preview_data.py` 填充用于 UI 展示的合成对话；`qa_android_ui018.py` / `qa_android_ui019.py` 依赖已配对 fixture 与相应数据文件，只用于当前兼容界面的回归。安装 0.1.14 签名包后运行 `python3 tests/android/qa_android_ui014.py`，验证设备列表标题、设备吸顶、最近 5 个预览与所有未读展开、蓝点已读/未读跨重启保存、新消息设备置顶。`qa_android_draft014.py` 在合成 Hub 暂停时验证冷启动输入、排队草稿恢复及重连后的持久化。对话回归以 `THREADBRIDGE_QA_VERSION=0.1.14-test THREADBRIDGE_QA_HOME_TITLE=设备列表 THREADBRIDGE_QA_CHAT_ONLY=1 python3 tests/android/qa_android_ui013.py` 运行；0.1.13 签名包可运行 `python3 tests/android/qa_android_ui013.py`，验证主页/对话底部刷新无事件推送时也能获取更新、顶部两次分页读到最早消息、闲置 3 秒隐藏和服务暂停时即时跳转、草稿保留、简化菜单及输入框展开收起。上一版：安装 0.1.12 签名包后运行 `python3 tests/android/qa_android_ui012.py`，验证输入框的 52dp/100dp 展开收起、32dp/12dp 侧边间距和底部留白、多行自然增高、键盘收起和进程重启后的草稿、浮动菜单及长正文跳转；同时捕获上下渐隐、深色和大字体截图。测试只读写隔离 Hub 与专用模拟器的合成数据，不提交消息；主页用例清理自己的合成对话及本机已读测试标记，保留配对和其他草稿。输出写入忽略的 `artifacts/`，不作为真实手机验收。

移动测试文件后必须更新 mock 子进程源码路径。集成测试中采用空 HOME/CODEX_HOME 和固定 mock，不沿用真实服务凭证；不要用生产地址替换测试 loopback。

resume worker 集成在 mock 轮次进行中发送 SIGINT，确认最终捕获、回执和租约释放完成后退出。

`test_visible_history.py` 验证图片人类消息、重复文字项、进行中过程说明、原始排序、最终回复去重、图片接口鉴权和副本清理；工具/推理不进入消息。`ConversationAttachmentsTest` 验证附件标记与正文分离。可见历史集成检查同一时间戳的新过程说明推进消息变化标记、完成轮次 ID 保持不变、重放不推进及清理状态；主页 JVM 检查进行中消息未读、毫秒级排序与旧已读格式兼容。清理集成覆盖缺失/损坏策略提前拒绝且不创建备份；Rust 恢复检查 18 MiB 图片摘要与图片改动后计划失效。`test_android_migrations.py` 用真实声明的迁移 SQL 验证 Room 1/2/3→4 与导出 schema 一致，并保留旧缓存和草稿；不替代真机升级验收。

安装当前签名包后运行 `python3 tests/android/qa_visible_history.py`，在专用模拟器验证图片缩略图、用户正文、过程说明、最终回复、图片预览与重启缓存，不发送真实对话。
