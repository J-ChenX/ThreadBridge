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

`test_visible_history.py` 验证图片人类消息、重复文字项、进行中过程说明、原始排序、最终回复去重、图片接口鉴权和副本清理；工具/推理不进入消息。`ConversationAttachmentsTest` 验证附件标记与正文分离。可见历史集成检查同一时间戳的新过程说明推进消息变化标记、完成轮次 ID 保持不变、重放不推进及清理状态；主页 JVM 检查进行中消息未读、毫秒级排序与旧已读格式兼容。清理集成覆盖缺失/损坏策略提前拒绝且不创建备份；Rust 恢复检查 18 MiB 图片摘要与图片改动后计划失效。`test_android_migrations.py` 用声明的迁移 SQL 验证 Room 1～8 及两种已交付 Room 7 形态进入 Room 9，与导出 schema 一致，保留所有旧字段、缓存、草稿和未确认请求，并拒绝版本低于已保存的历史 schema；不替代实际签名包升级验收。

启动升级回归：`python3 tests/android/qa_android_startup_upgrade.py --serial emulator-5554 artifacts/ThreadBridge-<version>.apk` 只允许专用、可 root 的模拟器。先安装实际签名包并确认启动，再分别覆盖安装历史 1.0.1、1.0.2、1.0.3 和当前候选包；期间使用离线合成数据库，检查实际 Room 打开、Activity 可见、重复冷启动，以及每个历史字段、消息、草稿、未确认请求和连接偏好保持。先与隔离合成 Hub 配对后增加 `--require-paired`，要求所有升级前后都直接显示设备列表与缓存对话，并保留加密连接偏好。测试不卸载应用、不访问生产服务、不发送上游命令；结束恢复模拟器网络。历史 1.0.2 的真实缓存版本为 8，不能以仓库当时的版本 6/7 代替正式 APK 的数据库事实。API 35 与 Android 16/API 36 均执行此回归。

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

2026-10-10：创建回归覆盖没有桌面控制 socket 时的 stdio 初始化、thread/name/set 落盘后再 queue、创建/落盘回执丢失时不重发与不可用执行器在意图前拒绝。实际 CLI 使用隔离 CODEX_HOME 和拒绝连接的 loopback 模型服务验证新 ID 未落盘时 queue 拒绝、命名落盘后 queue 接收，不触发真实模型调用。ConversationScopeTest 验证设备/项目批量已读覆盖旧折叠项、主机与未知项目隔离、新消息重新未读；回执测试验证未知提交清空输入与明确拒绝保留输入。

`qa_android_bulk_read_delete.py` 使用新安装的独立 debug 包、专用模拟器和合成 Hub/HTTP 代理，验证分组已读、重启保存、新消息重新未读、多选取消、批量删除部分失败与重试、未知发送清空输入、明确拒绝保留输入及成功创建 ID 切换；小屏深色/2 倍字体/关闭动画与横屏截图输出至忽略的 artifacts/。运行前启动 qa_android_fixture018.py，不使用生产凭证，不调用真实模型。

服务身份回归：Rust `hub_identity_is_unique_persistent_and_does_not_reset_legacy_data` 与 `integration/smoke.py` 验证真实配对、主机列表和重启均返回一致 `hub_id`，用于阻止新手机包搭配缺少身份接口的服务端发布。

压缩记录回归覆盖 `.jsonl` 转 `.jsonl.zst` 后的完成水位、原生用户消息、可见过程、幂等与健康失败自动恢复；解压字节预算、会话身份和目录边界必须继续拒绝不合法来源。Windows 安装目录回归验证同一安装目录的唯一候选发现与歧义拒绝，发现路径不授予新版本写权限。

交互问题：`test_questions.py` 使用隔离原生记录、真实 loopback Hub/HTTP 验证首条最终回复之前的问题投影、选项、原问题 ID 回复、不可变请求去重、电脑回答后状态同步、过期问题拒绝和删除后不重建。Rust `questions::tests` 检查工具白名单、人类元数据、异步 ACK 不代表回答、独立多问题、阻塞响应、未完成源不回退已回答状态；远端测试检查问题状态进入 snapshot revision 和清理保留原始文件。`qa_android_questions.py` 使用实际 debug APK 与临时合成 Hub，在专用模拟器验证显示、选项/回答、入列不提前完成、重启和自适应布局；仅生成一次明确合成命令，不访问生产服务。

发布门禁：`python3 tests/test_release_policy.py` 使用标准库和隔离的构建/SDK fixture 验证版本源回退、测试后缀、错误包名、debuggable、错误签名/基线哈希、版本与文件名不一致、缺失上一版、检查失败不生成正式 APK，以及不可覆盖已有产物。成功路径须经过 JVM、assembleRelease/lintRelease、正式验收和完整的版本化 APK/校验值/报告输出。runner 的集成入口和 CI 都执行此回归；Android CI 另用真实 SDK 检查实际 debug APK 的内部版本，避免 Gradle 与版本源漂移。模拟工具不替代正式签名及覆盖升级验收。

远程桌面项目：`desktop_projects::tests` 覆盖服务器项目为空时使用发起端明确归属、执行主机映射隔离、项目改名/删除、多桌面冲突、状态损坏保留、重复同步幂等及正文/原 ID/消息活动/请求不变；`desktop_project_changes_are_sent_even_without_body_changes` 验证没有正文变化和数据库下载时，新项目与名称变化仍进入真实 snapshot RPC。`test_desktop_projects.py` 使用固定合成 SSH 响应和真实 fleet/Hub/手机 HTTP 接口，验证正文快照未变化时的分类、改名、旧 helper 缺字段时保留、明确移除、原执行身份与消息活动不变；不登录真实主机，不提交命令。

`python3 tests/android/qa_android_images_status.py` 使用专用模拟器、实际 debug APK 和临时合成 HTTP 服务检查图片首次读取、同步阻塞时内存命中、进程重启后磁盘命中、点击空白关闭、精确原图下载、运行/完成/离线状态、具体健康告警及恢复；同时记录小屏深色大字号、关闭动画、横屏和平板截图。不会连接生产服务或提交命令。

`test_create_conversation.py` 通过真实 Hub/bridge 与隔离 CLI 验证创建、命名和 `turn/start` 首轮直发，检查真实 turn ID、无关完成轮次不能结算、丢失启动回执后保留原 ID、原文关联及重启不重发。Rust 新建回归验证无桌面 socket 提前拒绝、旧 queue 账本不重放、远端项目目录及 owner/执行设备/路径校验；`NewConversationProjectsTest` 验证无缓存对话的远端项目可选、不同设备同路径不合并及名称优先级。

`test_queue_steer_receipts.py` 验证首轮电脑引导追加一个 LF 后的旧回执修复，包括已导入最终回复的重新核对；没有 queue ID、错误 revision、重复输入及额外空白均不结算。`test_phone_steering.py` 通过真实 Hub/bridge 和固定 CLI fixture 验证手机显式引导、同一队列项删除、同一 active turn 接受、最终回复前恢复发送、真实 turn 完成、未知回执和重启不重发。Rust steering 模块覆盖来源校验、空闲/错误轮次/队列被修改时不操作、删除/引导回执丢失、幂等及未开始请求过期；JVM 检查按钮可用条件、回执证据及单向状态更新。`qa_android_steering.py` 只在专用模拟器使用自有 debug 合成数据，验证真实按钮、深色大字体、下一条草稿、发送按钮恢复、最终回复和重启；不操作正式应用或真实上游任务。

传输回归包含 WebSocket 消息边界、漂亮格式 JSON、超限/二进制拒绝和旧 JSONL 兼容。`cargo test --workspace --locked installed_proxy_attaches_to_isolated_control_fixture -- --ignored` 用已安装官方 proxy 连接隔离 WebSocket fixture，只执行只读请求，不启动 daemon 或模型任务。真实执行验收须另行区分新建任务所在公开执行器与桌面原任务归属，fixture 通过不能代替此验证。
