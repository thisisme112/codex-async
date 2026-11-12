# Codex 异步增强版

本仓库在社区完成通知实现上保留异步命令回调，补充 `wait_for_tasks` 事件等待工具，并在模型的基础提示中说明用法。

可用分支为 **async-main**。最初的官方 `main` 保留为 fork 创建时的参考快照，日常请使用默认分支 `async-main`。

首次官方更新的 39 个文件冲突已处理，首次适配基线为 `e58bf8f5233b51e7cd1679d947e3028c1ed91b06`；当前已同步提交以 `.github/async-fork-state.json` 为准，各组验证记录见 [UPSTREAM_STATUS.md](UPSTREAM_STATUS.md)。每次合并须通过相关测试和构建后才能推进可用分支。

## 自动同步

[查看同步任务和运行报告](https://github.com/thisisme112/codex-async/actions/workflows/async-upstream-sync.yml)。每天北京时间 **03:23** 尝试同步 `openai/codex:main`，也可以在 Actions 中手动运行。

流程先在独立临时工作树合并。没有冲突、异步功能契约保留后，才构建 Linux CLI 并运行相关核心单元测试与异步集成测试。全部通过才以原子、非强制推送更新 `async-main` 和 `upstream/source`；验证期间有人更新可用分支，会拒绝覆盖。

冲突、功能契约变化、编译或测试失败都会阻止更新。Actions 的 Summary 和 `async-sync-report` 下载项会留下结果。遇到架构变化仍需要人工适配，不能保证所有官方更新都能自动合并。

已人工审阅的冲突可记录到 `.github/async-fork-resolutions.json`：只有共同基线、我方、官方三个文件 blob 和冲突文本全部一致，才使用对应的合并结果；其他版本继续停止并报告。向 `async-main` 推送提交信息带 `[sync-upstream]` 的同步修复后，会自动重试受保护的合并、验证和发布流程。普通打包或版本设置更新只运行发布流程，避免同时启动两次 CLI 编译。

官方的 `.github/workflows/` 入口不随源代码合并，本仓库只运行自己的同步工作流，避免触发官方整套 CI。官方其他源代码、构建脚本、测试和本地 action 均保留。`upstream/source` 是排除这些工作流入口后的官方源代码快照，提交历史仍关联真实官方提交。

同步检查只需要很少资源；有新改动且可以合并时才进行完整验证。运行时、核心异步行为、CLI/TUI 分别在 3 个标准 Ubuntu runner 并行验证，各自编译并发 2、最多两核心，全部成功后才能发布；本地验证默认并发 1。不使用付费大型 runner。合并检查与验证任务只有只读权限，最后的发布任务才有仓库写权限。没有额外 PAT。

Cargo 下载和第三方依赖的编译产物会跨次缓存，缓存键包含固定 Rust 工具链、依赖锁文件及构建环境。CLI 验证与发布共用 Ubuntu 22.04 缓存，核心测试、运行时按 Ubuntu 24.04 分组隔离。仅默认分支成功构建后写入缓存，适配分支可以读取默认分支缓存，避免大量分支各存一份。剔除 workspace 自身产物、源代码解压目录和庞大的 Rust incremental 目录，不提高 GitHub 的缓存付费上限。首次实际编译仍需要建立缓存；依赖或工具链变化时可能部分失效，提速幅度以缓存命中后的实际运行记录为准。

缓存或工作流配置、文档变化不会重新发布相同成品；发布前比较 Rust、安装脚本及实际构建输入，全部相同就沿用已有 Release。需要重建工作流环境变化时，可手动勾选 `force_rebuild`。临时安装包只保留 1 天，已成功发布后清理临时副本，不重复上传验证时生成的同一个包；Release 下载长期保留。

验证日志每分钟记录一次构建心跳、可用内存和磁盘空间，不调用模型。依赖准备复用 runner 现有库，缺失的 libcap 开发头文件按官方 Bazel 锁定来源校验后补齐，仍链接系统运行库。依赖日志和验证日志都会保留 7 天，失败后也可查看。

GitHub 有时会暂停长期无活动仓库的定时工作流，可在 Actions 页面恢复。

## 手动检查

在干净、具有完整 Git 历史的 checkout 中：

```bash
python3 -m unittest discover -s scripts/async_fork -p 'test_*.py' -v
python3 scripts/async_fork/sync.py check
python3 scripts/async_fork/sync.py prepare --output /tmp/codex-async-sync
```

`prepare` 不更新可用分支。`promote` 只用于已完成编译和测试的候选版本，不应绕过验证手工调用。手动 Actions 运行时勾选 `check_only` 可以只检查合并情况，不编译、不发布。需要验证手工处理冲突的分支时，选择该分支并勾选 `verify_only`，保持 `check_only` 不勾选；此模式编译和测试选中提交，但不会自动发布。`integration/official-*` 适配分支的代码或验证脚本推送也会自动触发同样的只读验证。仅调整异步测试夹具时，可在 `integration/async-*` 分支只复验异步集成测试，沿用生产代码相同的其他成功验证记录；这个专项分支也不会自动发布。

## 构建与使用

[下载已编译的 Linux x86_64 安装包](https://github.com/thisisme112/codex-async/releases)。下载 `codex-async-linux-x86_64.tar.gz`，完整解压后运行：

```bash
bash ./install.sh
~/.local/bin/codex-async
```

默认安装到用户目录，不需要 sudo，保留原来的官方 `codex` 命令。需要 Ubuntu 22.04+ 或 Debian 12+ 等 glibc 2.35+ 系统，以及 libssl3/libcap2。发布页附带 SHA256 校验值和源码提交信息。

使用 `dev-small` 配置，功能完整但未进行 release 优化。程序、安装包和 Release 标题跟随已合入源码所对应的官方版本，保留 `alpha`、`beta` 等标识；不会把尚未合入的新版标签用于旧源码。版本会在编译前按官方发布方式写入构建副本及本地 workspace 锁定条目，不更新外部依赖。`VERSION` 记录官方版本，`BUILD_ID`、`BUILD_INFO.json` 单独记录分支提交和来源。Release 标签用 `rust-v<官方版本>+async.<提交号>` 区分同一官方版本的不同构建；安装时也按构建标识隔离，允许更新同版本的异步补丁。

官方同步验证成功后会自动发布安装包，优先直接复用版本、源码都匹配的验证产物，不重复编译。

依赖配置见原仓库 `docs/install.md`。在仓库根目录组装包含配套工具的包：

```bash
just assemble-codex-package --target x86_64-unknown-linux-gnu --package-dir /tmp/codex-async-package
/tmp/codex-async-package/bin/codex
```

请保留完整目录。异步通知需要本地交互式 CLI 持续运行，关闭后暂不恢复通知。`wait_for_tasks` 等待完成事件，`write_stdin` 保留输入和进度读取；收到完成通知后读取输出和退出码再分析。基础提示用于引导模型减少轮询，不能保证模型完全不查询。

遵循官方默认行为，中断会终止活动 Code Mode cell，已终止的 cell 不会再触发完成通知。显式关闭 `code_mode_interrupt` 时，仍存活的 cell 会保留完成状态，等用户继续后再通知。

最初试用版的 93 项相关单元测试、36 项集成测试已通过本地模拟模型验证；这不代表后续任何官方合并候选都已通过验证。每个候选以其 Actions 结果为准。原始已验证源码信息见 `ASYNC_BUILD.md`。
