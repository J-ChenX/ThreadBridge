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

CI 使用 Python 3.14 执行回归。集成测试调用原生 capture/resume CLI；mock 子进程使用所选 Python。Rust 模块单元测试在 Linux/Windows 运行，发送和进程所有权 fixture 只使用隔离模拟器，未证明任意真实 Codex 或四机长期运行。

installed proxy 的 Rust fixture 默认忽略，它依赖本机 Codex CLI；单独运行仍只操作隔离 socket。常规 CI 不依赖该 CLI。

常规 Rust fixture 通过符号链接运行固定 mock 文件，临时目录只写数据，避免并行创建并执行脚本时的 `Text file busy`；mock 使用 PATH 中选定的 Python。

`test_thread_titles.py` 验证原生名称经采集、远端快照和 Hub 到达手机 HTTP API，单独改名推进刷新事件，重放幂等，并验证本机名称同步、原 ID/消息修订/未读时间保持及无命令或完成通知。Rust 单元回归补充旧版名称索引、缺失字段、UTF-8 长名称与发现窗口之外的旧副本更新。

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

`test_visible_history.py` 验证图片人类消息、重复文字项、进行中过程说明、原始排序、最终回复去重、图片接口鉴权和副本清理；工具/推理不进入消息。`ConversationAttachmentsTest` 验证附件标记与正文分离。可见历史集成检查同一时间戳的新过程说明推进消息变化标记、完成轮次 ID 保持不变、重放不推进及清理状态；主页 JVM 检查进行中消息未读、毫秒级排序与旧已读格式兼容。清理集成覆盖缺失/损坏策略提前拒绝且不创建备份；Rust 恢复检查 18 MiB 图片摘要与图片改动后计划失效。`test_android_migrations.py` 用真实声明的迁移 SQL 验证 Room 1/2/3/4/5→6 与导出 schema 一致，并保留旧缓存和草稿；不替代真机升级验收。

安装当前签名包后运行 `python3 tests/android/qa_visible_history.py`，在专用模拟器验证图片缩略图、用户正文、过程说明、最终回复、图片预览与重启缓存，不发送真实对话。

0.1.15 签名包运行 `qa_android_ui015.py`：专用模拟器/合成 Hub 验证设备项目层级、最近 3 个与未读并集、项目折叠、标题搜索预览之外的结果、置顶收藏重启持久化、标准时间戳、主页唯一连接入口、空白首次发送及新原 ID 切换、深色/大字体截图；发送只进入合成 Hub 生成的回执，不调用真实 Codex。`test_create_conversation.py` 通过真实 loopback Hub/capture bridge 和固定 mock CLI 验证持久创建、首轮完成回执、原 ID 映射、项目、搜索和重启无重发。Room 迁移覆盖 1/2/3/4→5。

0.1.16：qa_android_ui016.py 使用原签名升级Room6，验证设备/项目双层吸顶、注册项目名称、过程展开收起且最终回复保留、连续同角色时间段及同主题新建选择页（含深色、大字体），不提交命令。test_project_alignment.py 验证只读规划、只对齐既有ID、项目改名/删除、其他主机隔离与正文/配对/策略/请求不变。

qa_android_ui016_boundaries.py 验证跨项目及跨设备时主页/侧边栏固定栏切换；test_runtime_alignment.py 以 fake systemctl 注入首个启动失败，验证全部原 active 服务恢复、配置回退、inactive peer 与 unknown 请求保留、活动发送阻止切换。

0.1.17 的 qa_android_ui017.py 验证短过程、超过视口的长过程与历史轮次向下展开，过程栏位置保持、正文在其下方、收起恢复原位置，不提交任何命令。

0.1.18：qa_android_polish018.py 使用 ffmpeg/ffprobe 对模拟器展开录屏采样，检查过程栏中间帧稳定、透明色插值发暗及胶囊范围；覆盖连续反向点击、系统动画关闭、375dp 小屏、2倍字体深色、横屏、平板横竖屏、键盘焦点、52dp/100dp 输入栏、48dp 发送触控区域、键盘收起/重启保留草稿及无命令提交，并复验图标菜单、置顶/收藏两态和搜索选中反馈（还原测试标记）。render_icon_audit.py 从 Kotlin 直接导出浅深色、14/18/20/24dp 的图标审查 SVG。qa_android_ui016_boundaries.py 同时检查项目固定栏不覆盖设备栏，可在2倍字体配置下复验。qa_android_ui016.py/017.py 可用 THREADBRIDGE_QA_VERSION 指定当前兼容版本重验。

0.1.19：`qa_android_library019.py` 用专用模拟器验证每个项目最近 3 个与未读并集、展开全部/收起、两级折叠、侧边栏共享展开状态；特别构造十分钟前的旧未读对话，打开变为已读后检查 Room 对话仍存在并可展开访问。一周清理按原策略删除本机副本，同一修订不被同步回填，主动标题搜索可重新访问，合成 Hub 原数据和命令数不变。`qa_android_images019.py` 使用自绘 2400×1600 测试图，通过实际签名 APK 验证全屏、缩放控件、双击、真实双指缩放、平移、复位、5 倍边界、返回/重开、深色外观、375dp/两倍字体及横屏；ImageGestures.java 只在专用 API 35 模拟器的 shell 中注入手势，不进入 APK；Pillow 使用桌面内置 Python。`ImageViewerGeometryTest` 验证缩放焦点、适配留白/平移边界、采样及复位，`ConversationTimelineTest` 包含一万条同角色连续消息排序回归。

0.1.20：`PublicTestConnectionTest` 检查已知旧入口精确匹配、目标 HTTPS、主机身份和其他配对隔离，`ConnectionDiagnosticsTest` 检查错误分类及响应/凭证文本不进入 UI。`qa_android_connection020.py` 在专用模拟器使用实际签名 APK 和隔离 HTTP 代理，拒绝 WebSocket 后验证仍显示已连接、无推送时新对话自动轮询到达；模拟 HTML 404、成功但无效的数据、401，检查具体诊断、自动恢复和缓存/草稿/加密凭证保留，合成 Hub 命令数不变。脚本只临时修改模拟器的合成服务器地址，结束后恢复，不接触生产凭证。生产入口只能以无凭证请求验证路由/TLS；真机的自动迁移结果仍需覆盖安装确认。

0.1.21：`ConnectionConfigurationTest` 验证 HTTPS/私人 IP、嵌入凭证/查询参数/错误端口拒绝、空密钥沿用、请求头注入拒绝及不同 collection 隔离。`qa_android_settings021.py` 在实际签名 APK 和合成 Hub 上验证设置取消、空密钥切换地址、错误密钥和不同库拒绝、新密钥显示/隐藏和加密保存、同库缓存/草稿保留、重新配对确认，以及深色/375dp 两倍字体/横屏/关闭动画；不发送远端命令，生产凭证与生产库不参与测试。固定公网入口需独立核对保留记录、正常 DNS/TLS 无凭证鉴权响应、单服务重启后地址仍一致和原其他隧道继续运行。

`qa_android_keyentry021.py` 另使用没有现存数据的独立 debug 包，验证首次地址/密钥连接直接进入主页且侧边栏关闭、未经过配对交换、重启后加密凭证可用以及命令数不变；结束删除该测试包及合成手机身份，实际签名 release 包的数据不受影响。
