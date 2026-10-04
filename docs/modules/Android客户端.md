# Android 客户端

## 当前实现

M01 使用 Kotlin、Compose 与 Room，只显示用户消息、最终回复和状态。当前应用为 `dev.threadbridge`、0.1.9-test、versionCode 10；debug 包添加 `.debug`。Room schema 为版本 3，版本 1/2/3 导出位于 `android/app/schemas/`。

配对支持地址和一次性码、二维码。Android Keystore AES-GCM 保护凭证；系统备份与迁移排除凭证及缓存。正式连接要求 HTTPS；私人 IP HTTP 仅在显式局域网测试时可用。关闭重定向，不向跳转地址转发凭证。

发送前持久保存不可变请求；网络失败显示提交待确认。先查原请求，再按有效期和同一快照重试；unknown 不自动重发。离线可读缓存并编辑草稿。已确认发送通过用户消息显示，不重复放在助手回复后的回执中；未确认/失败保留状态和操作。

## 同步与缓存

前台仅一组同步会话，事件唤醒合并、退避重连和周期补核对。后台关闭连接，恢复前台重新鉴权。正文请求失败不阻断回执查询；命令按单调状态合并。配对、清除、草稿和同步使用代际及串行门，旧会话不能写入新账户缓存。

正文分页使用 `(ordinal,id)`；分段绑定消息版本，最多缓存 128 段。消息缓存最多 5000 条、首段正文预算约 16 MiB；草稿与待确认请求独立保护。历史继续按页加载，不要求一次渲染全部内容。

Room 1→2 增加 chunks，2→3 增加主机缓存。collection generation 改变时，成功同步后清除旧消息、分段、对话、草稿、待确认记录和游标，保留凭证及新主机列表。长按删除共享同步副本，不删除原 Codex 对话；服务端 tombstone 防止回填。

## 呈现与通知

主页按设备折叠分类，空设备可见。系统浅色/深色统一中性色；用户右侧气泡，助手直接排版。输入栏默认紧凑，获焦点动画展开；键盘收起、导航或菜单打开时清除焦点并折叠，草稿保留。对话菜单提供同步、回到最新、更早消息；有历史游标时可从电脑加载。连接状态和设置在主页。

CommonMark 与 GFM 表格渲染为可选择 Compose 文本；不用 WebView，不执行 HTML/脚本或加载远程图片。链接仅在用户点击后交给系统。

后台通知当前沿用 ntfy，`threadbridge://thread/{id}` 仅导航，再经鉴权加载正文。APK 直接 UnifiedPush/FCM、通知栏回复和远程审批尚未开放。

## 验证

JVM 用例位于 `android/app/src/test/`，覆盖缓存同步门、消息格式、回执、默认连接迁移和呈现策略。`tests/android/` 使用专用模拟器和合成 Hub 验证实际 UI；`scripts/android_preview.py` 提供 loopback 像素预览。模拟器数据与真实 Hub 隔离。

`testDebugUnitTest`、`lintDebug`、`assembleDebug` 不需要 release 签名。`scripts/build-apk.sh` 验证 release、Lint、签名和 16 KiB 对齐。小米 Android 16、扫码、系统重启、移动网络、锁屏通知、真实键盘和覆盖升级仍需真机验收。
