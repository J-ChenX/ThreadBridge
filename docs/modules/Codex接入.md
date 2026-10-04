# Codex 接入

## 路线与职责

M04 将已完成轮次、用户输入与发送回执转换为 ThreadBridge 投影；不按标题匹配目标，不 fork 替代原 ID。完成通知与桌面私有工具管道是不同接口，不将宿主调用身份作为服务凭证。

| 路线 | 当前实现与限制 |
|---|---|
| 完成通知 | `scripts/capture_catalog.py` 选择白名单或全任务；`capture_completion.py` 保存确定的 thread/turn/final，不保存 notify 原始输入 |
| 当前轮次用户消息 | `scripts/capture_user_turn.py` 经显式本机索引读取，校验 session、turn、完成记录和 `user.text`；这是安装版持久格式适配，非通用公开导出 API |
| 独立读取 | `capture-import` / `capture-sync` 将捕获库幂等投影到 Hub；Codex 离线不影响已保存内容读取 |
| 桌面协同队列 | `capture-bridge --allow-queue --verified-version ...` 按原 ID 入列；可能需要桌面对话激活才消费，不承诺无人值守 |
| 原 ID resume 候选 | `scripts/resume_dispatch.py` 在独立短期 grant/owner 租约下核验原 ID、cwd、revision、CLI 版本、sandbox 与审批后发送；不同于默认 queue 路线 |
| Rust proxy Adapter | `adapter.rs` 仅连接已有 App Server，写能力需精确版本验证；默认控制 socket 是否可用由具体部署决定 |

`doctor [--socket PATH] [--thread ID]` 委托已安装的 `codex app-server proxy --sock` 诊断已有控制端点，不启动 daemon。每 RPC 3 秒、256 KiB、32 通知预算；输出仅含状态与数量，不输出标题/正文/凭证。退出 0 表示只读探测通过，2 为接入失败，64 为参数错误；探测通过不代表 G01 完成。

## 捕获、健康与关联

单条 UTF-8 正文最多 256 KiB。保存预算、剩余磁盘和健康状态受限；错误明确报告，不以截断正文冒充成功。`.health` 双槽状态保留捕获意图和成功/失败，初始意图不可写时不能声称记录可靠。标题索引只读元数据，不能代替身份核验。

通知入口和远端读取均受 `collection_policy.py` 的 cutoff、排除 ID、删除 ID 和 generation 约束。跨进程锁串行保护策略变更与捕获；策略无法核对时停止。没有收到通知不能补造完成事件；官方 notify 不投递的轮次可能缺失。

`capture_user_turn.py` 只读取明确完成的当前轮次人类输入，拒绝系统附加内容、工具结果和其他轮次。读取失败时助手正文仍保存，健康状态说明用户消息缺失。`captured_user_messages` 与 `captured_turn_order` 支持准确顺序；Hub 在用户输入晚于回复到达时重新关联。

当前 queue/resume 发送人类原文，不附加内部 UUID。队列确认使用持久命令账本、准确输入摘要、发送时间、前轮 revision、同 host/thread 和唯一活动候选；条件不足保持未确认。电脑同时提交相同原文仍缺上游端到端请求 ID 证明。`capture_completion_v014.py` 保留多标记 UUID 候选，供历史兼容回归和 resume 最终回复路径使用；不会启用旧 catalog 入口。

## resume 权限边界

grant 必须显式绑定原 thread/host/cwd、CLI 版本、权限、工具兼容确认、执行移交和短期有效期。先验证 `thread/read` / `thread/resume` 返回同一 ID、原工作目录和 directInput；最近权威 revision 不符不发送。它可启动一个新的官方 stdio App Server 执行器，不能称为原桌面实时执行器。

旧 queue 未决或竞争工作器存在时拒绝移交。发送意图先落盘；需要用户审批、动态工具不可用、回执中断或来源不明时停止并保留 unknown，不自动审批或重投。只有匹配的 completed turn 与明确 final_answer 才直接捕获；其他阶段不能晋升为最终正文。默认 sandbox 为 read-only、网络关闭；workspace-write 仅限显式 grant 的工作目录。

## 验证

`cargo test --workspace --locked` 检查协议、目标核验和回执。`python3 tests/run.py` 检查实际解释器、mock queue/resume、回环 HTTP、独立保存、重启、关联歧义和不重复发送。隔离 fixture 不调用真实 Codex。

任何新 Codex 版本、执行路线或主机必须分别验证原 ID、上下文、工具、审批、并发和重启恢复。安装版持久记录兼容性、桌面 queue 调度、公网及真机表现属于当前限制；指定对话曾续聊成功不能取消这些验证要求。
