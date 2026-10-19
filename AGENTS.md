# ThreadBridge 开发与交付规则

本文件适用于整个仓库。开始开发、修改打包流程或交付产物前必须读取；用户明确指令优先。版本、包名、签名与升级规则的详细说明见 `docs/开发与发布.md` 和 `docs/运行与安装.md`。

## 版本基线

- Android 版本的唯一事实来源是 `android/release.properties`，分别记录待交付版本与最近已交付版本、versionCode、原签名指纹及上一版 APK 哈希。先读取它，再核对上一版真实 APK；不得根据旧 README、目录名称或历史聊天猜版本。
- Gradle 必须读取该文件，禁止重新写入独立的版本常量；README、安装和模块文档只引用版本源及通用产物名，不另存“当前版本”数值，历史版本说明应明确属于历史。日常兼容更新默认递增修订号，versionCode 必须严格递增；需要调整主/次版本时说明理由。用户指定版本时遵循用户要求，但不能降低 Android 升级所需的 versionCode。
- 已交付版本登记不能因“构建成功”自动前移。实际交付后更新已交付版本及真实 APK 哈希，并同时登记下一个候选版本，保持已交付/候选状态明确；未交付的候选版本不能写成已发布事实。

## 安装包交付是完成条件

- 修改 Android 用户可见功能、修复手机问题或用户要求 APK 时，默认交付能覆盖升级原应用的正式 release 包。debug 包仅用于内部回归；除非用户明确要求调试包，否则不能以 debug APK、临时预览包或改名后的测试包结束任务。
- 正式交付必须使用 `scripts/build-apk.sh PREVIOUS.apk`。必须核验真实上一版 APK 与已登记的版本、versionCode、包名、签名和哈希一致；没有上一版或原签名材料时明确报告具体阻塞，不得退回 debug 交付、生成替代签名、卸载原应用或仅修改文件名。
- 最终 APK 必须为 `dev.threadbridge`、非 debuggable、稳定三段版本号，文件名必须是 `ThreadBridge-<versionName>.apk`，内部版本必须与版本源一致；versionName 和 versionCode 均须高于已交付版，签名必须保持一致。
- 只有完整通过测试、release 构建、release Lint、签名/16 KiB 对齐以及 `verify-release.py --previous` 检查，并生成 `delivery_approved: true` 验证报告和 SHA-256 后，才可在最终答复中作为安装包交付。不得手工复制 debug APK 到正式产物路径，不得绕过门禁，也不得覆盖已有同版本产物。
- 最终答复应给出版本号、版本化 APK、SHA-256/验证报告链接及实际验证范围。区分代码完成、安装包交付、电脑服务部署和真机安装；未执行的操作不能声称完成。不要将打包授权扩展为自动上传 GitHub、更新生产服务或卸载手机应用的授权。
- 仅修改文档、发布流程或其测试且未修改产品功能的任务，可用对应流程回归作为交付，不要求顺带发布新版产品；明确说明没有发布安装包。

## 验证与工作区保护

- 发布规则、版本源或打包脚本改动必须运行 `python3 tests/test_release_policy.py`；修改 Gradle 版本读取时构建实际 debug APK，并运行 `release_policy.py --check-debug-apk` 核对真实元数据。CI 必须保留这两项门禁。
- 所有其他检查按 `docs/开发与发布.md` 选择与变更相关的验证。Android debug/release 的 KSP 不并行写同一 Room schema。
- 生产数据、签名、密码、配对凭证、构建产物留在忽略目录；日志中不得输出秘密。保留工作区已有未提交修改，禁止为了发布重置它们或提交其他任务的内容。
