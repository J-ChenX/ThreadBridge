# Codex 接入

## 路线与职责

M04 将已完成轮次、用户输入与发送回执转换为 ThreadBridge 投影；不按标题匹配目标，不 fork 替代原 ID。完成通知与桌面私有工具管道是不同接口，不将宿主调用身份作为服务凭证。

通知采集与远端轮询使用原生 SQLite 索引中的 `thread_source` 或旧 `source` 元数据排除 `subagent`，过滤发生在正文读取及健康失败登记之前。`agent_created_thread` 是普通对话，继续保留。通知入口应传入 `--user-turn-index`；JSONL `--title-index` 只负责名称，不能代替原生身份索引。

既有子代理副本使用 `cleanup-subagents --db HUB --capture-db CAPTURE --index STATE --host HOST` 只读规划；存在待清理记录时，规划与执行均先验证已有 collection 策略，缺失或损坏时在创建备份和改动副本前拒绝，不自动创建 cutoff。`--apply --backup-dir NEW_DIRECTORY` 先备份，再持久排除、删除采集正文和 Hub 副本、发布手机删除事件，并清除这些记录的采集失败。保留原 Codex、配对身份和必要发送去重账本。执行前停止对应同步和队列服务，执行后恢复。手机下一次同步会删除对应缓存并读取更新的健康状态。

手机的手动同步刷新 Hub 已保存的副本与采集健康状态，不会重新运行电脑端失败的用户消息采集；因此电脑仍有失败时，点击同步不会自行消除提示。子代理副本清理同时移除其失败，正常用户对话的真实失败仍保留。

健康接口将已离线且超过 30 秒未更新的正常快照放入 `offline_hosts`，设备离线继续由设备列表显示，不使其他电脑的正常同步永久报警。离线设备存在失败、overflow 或投影错误时仍放入 `hosts`；在线设备健康状态过期或没有可用监控时继续提示尚未确认。原始检查时间不改写，兼容现有 Android 健康提示逻辑。

原生索引标记 `archived=1` 的对话也不进入新采集。已有归档副本使用 `cleanup-archived`，参数、备份及删除事件机制与 `cleanup-subagents` 相同，按原生归档状态选择记录，不按名称合并对话；未归档的同名对话仍是独立 ID。清理仅影响续桥副本，对已清理的 ID 持久排除，原 Codex 归档继续保留。

| 路线 | 当前实现与限制 |
|---|---|
| 完成通知 | `threadbridge capture --catalog/--all-tasks` 选择范围；`native_capture.rs` 保存确定的 thread/turn/final，不保存 notify 原始输入 |
| 当前轮次用户消息 | `native_input.rs` 经显式本机索引读取，校验 session、turn、完成记录和 `user.text` / `user.image`；这是安装版持久格式适配，非通用公开导出 API |
| 独立读取 | `capture-import` / `capture-sync` 将捕获库幂等投影到 Hub；Codex 离线不影响已保存内容读取 |
| 桌面协同队列 | `capture-bridge --allow-queue --verified-version ...` 按原 ID 入列；可能需要桌面对话激活才消费，不承诺无人值守 |
| 原 ID resume 候选 | `threadbridge resume` 在独立短期 grant/owner 租约下核验原 ID、cwd、revision、CLI 版本、sandbox 与审批后发送；不同于默认 queue 路线 |
| Rust proxy Adapter | `adapter.rs` 仅连接已有 App Server，写能力需精确版本验证；默认控制 socket 是否可用由具体部署决定 |

`doctor [--socket PATH] [--thread ID]` 委托已安装的 `codex app-server proxy --sock` 诊断已有控制端点，不启动 daemon。每 RPC 3 秒、256 KiB、32 通知预算；输出仅含状态与数量，不输出标题/正文/凭证。退出 0 表示只读探测通过，2 为接入失败，64 为参数错误；探测通过不代表 G01 完成。

## 捕获、健康与关联

单条 UTF-8 正文最多 256 KiB。保存预算、剩余磁盘和健康状态受限；错误明确报告，不以截断正文冒充成功。`.health` 双槽状态保留捕获意图和成功/失败，初始意图不可写时不能声称记录可靠。标题索引只读元数据，不能代替身份核验。

身份有效且完成通知明确携带 null 或空字符串最终回复时，没有正文需要保存，返回 `ignored_empty_reply`，不创建正文或缺失用户输入告警；缺失字段、非法身份仍报告错误。历史回填仅在同一轮次同时具有空的 `final_answer`、无错误且明确为空的 `task_complete.last_agent_message`，并且没有已保存正文时，清除旧的 `missing_reply_identity` 误报。其他失败保留，手机刷新只能重新读取电脑状态，无法自行修复采集错误。

通知入口和远端读取均受 `collection.rs` 的 cutoff、排除 ID、删除 ID 和 generation 约束。跨进程锁串行保护策略变更与捕获；策略无法核对时停止。可见历史回填只将源记录中明确 `final_answer` 且匹配 `task_complete.last_agent_message` 的成功轮次补入最终回复，不推测未完成结果。

通知路径通过 `native_input.rs` 读取明确完成的当前轮次人类输入。图片消息要求所有内容项均带有人类元数据，支持重复文字项，剔除界面的附件包装文字；系统附加内容与工具结果不作为人类输入。读取失败时助手正文仍保存，健康状态说明用户消息缺失。`captured_user_messages` 与 `captured_turn_order` 支持准确顺序；Hub 在用户输入晚于回复到达时重新关联。

当前 queue/resume 发送人类原文，不附加内部 UUID。队列确认使用持久命令账本、准确输入摘要、发送时间、前轮 revision、同 host/thread 和唯一活动候选；条件不足保持未确认。电脑同时提交相同原文仍缺上游端到端请求 ID 证明。`capture --store-candidates` 保留有界多标记 UUID 候选，供历史兼容回归使用；resume 按权威 turn ID 关联，不给人类输入添加内部标记。

## 可见历史补录

`capture-backfill --database CAPTURE --index STATE --thread ID` 在 collection、来源及归档过滤后读取原生历史，只补录人类文字、内联 PNG/JPEG/WebP 图片、助手 `commentary` 和已确认最终回复。图片每张最多 2 MiB、源库图片合计 64 MiB；过程说明合计 32 MiB；超限报告错误。工具调用、工具输出、推理及文件修改卡片排除。原生记录保持只读。

`capture-sync ... --native-index STATE` 按原生文件变化补录已捕获对话，包含尚未结束或被中断轮次中的可见文字，不将其升级成完成结果。消息 ID、原始时间与最终回复的既有 `notify:turn` ID 保持稳定，重复回填不会重复显示。新增附件、过程说明及导入游标纳入副本删除和 collection 重置；`GET /v1/threads/{id}/messages/{message}/images/{image}` 要求手机鉴权并验证消息仍存在。

健康恢复摘要逐行读取源库，使用类型与长度分隔的原始字节计算 SHA-256，图片不展开为 JSON 数字数组；预算按原始字节计入。摘要格式升级后，旧恢复计划因绑定不一致拒绝执行，需重新 prepare；照片内容变化同样使已准备计划失效。远端数据库的 32 MiB 总限额仍适用，不因源图片单独预算 64 MiB 而扩容。

## resume 权限边界

grant 必须显式绑定原 thread/host/cwd、CLI 版本、权限、工具兼容确认、执行移交和短期有效期。先验证 `thread/read` / `thread/resume` 返回同一 ID、原工作目录和 directInput；最近权威 revision 不符不发送。它可启动一个新的官方 stdio App Server 执行器，不能称为原桌面实时执行器。

旧 queue 未决或竞争工作器存在时拒绝移交。resume 工作器持有同一个退出信号等待器，处理期间收到退出信号时先完成当前处理再释放租约，不因下一次轮询重建等待器而丢失信号。发送意图先落盘；需要用户审批、动态工具不可用、回执中断或来源不明时停止并保留 unknown，不自动审批或重投。只有匹配的 completed turn 与明确 final_answer 才直接捕获；其他阶段不能晋升为最终正文。默认 sandbox 为 read-only、网络关闭；workspace-write 仅限显式 grant 的工作目录。

## 验证

`cargo test --workspace --locked` 检查协议、目标核验和回执。`python3 tests/run.py --integration-only` 检查真实 Rust CLI、mock queue/resume、回环 HTTP、独立保存、重启、关联歧义和不重复发送。隔离 fixture 不调用真实 Codex。

任何新 Codex 版本、执行路线或主机必须分别验证原 ID、上下文、工具、审批、并发和重启恢复。安装版持久记录兼容性、桌面 queue 调度、公网及真机表现属于当前限制；指定对话曾续聊成功不能取消这些验证要求。

## 项目与手机新建

项目归属由原生已登记项目及根目录匹配线程 cwd，兼容 worktree；项目 ID、名称、根目录进入独立 `captured_projects`、`captured_project_names` 及 `captured_project_catalog` 副本表，不导入旧对话正文。无元数据的旧来源显示“项目待同步”。`new_threads.rs` 在已验证启用 queue 的设备侧执行 `thread/start`，继承桌面既有权限并以真实原 ID queue 第一条人类文字；持久意图先于创建，原 ID 落库先于 queue，创建/queue 未知时不重试。原始 Codex 记录不因手机缓存清理而修改。手机接口与查询合同见 [通信与存储](通信与存储.md#项目与手机新建)。新建需要匹配版本的 Hub、bridge 和远端 helper，APK 安装不会升级电脑服务。
