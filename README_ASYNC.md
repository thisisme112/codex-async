# Codex 异步增强版

本仓库在社区完成通知实现上保留异步命令回调，补充 `wait_for_tasks` 事件等待工具，并在模型的基础提示中说明用法。

可用分支为 **async-main**。最初的官方 `main` 保留为 fork 创建时的参考快照，日常请使用默认分支 `async-main`。

当前首次预检查发现 **39 个文件冲突**，已保留可用版本；记录见 [UPSTREAM_STATUS.md](UPSTREAM_STATUS.md)。需要先完成一次接口适配，之后每次兼容更新才会自动推进。

## 自动同步

[查看同步任务和运行报告](https://github.com/thisisme112/codex-async/actions/workflows/async-upstream-sync.yml)。每天北京时间 **03:23** 尝试同步 `openai/codex:main`，也可以在 Actions 中手动运行。

流程先在独立临时工作树合并。没有冲突、异步功能契约保留后，才构建 Linux CLI 并运行相关核心单元测试与异步集成测试。全部通过才以原子、非强制推送更新 `async-main` 和 `upstream/source`；验证期间有人更新可用分支，会拒绝覆盖。

冲突、功能契约变化、编译或测试失败都会阻止更新。Actions 的 Summary 和 `async-sync-report` 下载项会留下结果。遇到架构变化仍需要人工适配，不能保证所有官方更新都能自动合并。

官方的 `.github/workflows/` 入口不随源代码合并，本仓库只运行自己的同步工作流，避免触发官方整套 CI。官方其他源代码、构建脚本、测试和本地 action 均保留。`upstream/source` 是排除这些工作流入口后的官方源代码快照，提交历史仍关联真实官方提交。

同步检查只需要很少资源；有新改动且可以合并时才进行完整验证。使用公开仓库的标准 Ubuntu runner，构建并发 1、最多两核心；不使用付费大型 runner，也不上传大型编译缓存或二进制。合并检查与验证任务只有只读权限，最后的发布任务才有仓库写权限。没有额外 PAT。

GitHub 有时会暂停长期无活动仓库的定时工作流，可在 Actions 页面恢复。

## 手动检查

在干净、具有完整 Git 历史的 checkout 中：

```bash
python3 -m unittest discover -s scripts/async_fork -p 'test_*.py' -v
python3 scripts/async_fork/sync.py check
python3 scripts/async_fork/sync.py prepare --output /tmp/codex-async-sync
```

`prepare` 不更新可用分支。`promote` 只用于已完成编译和测试的候选版本，不应绕过验证手工调用。手动 Actions 运行时勾选 `check_only` 可以只检查合并情况，不编译、不发布。

## 构建与使用

依赖配置见原仓库 `docs/install.md`。在仓库根目录组装包含配套工具的包：

```bash
just assemble-codex-package --target x86_64-unknown-linux-gnu --package-dir /tmp/codex-async-package
/tmp/codex-async-package/bin/codex
```

请保留完整目录。异步通知需要本地交互式 CLI 持续运行，关闭后暂不恢复通知。`wait_for_tasks` 等待完成事件，`write_stdin` 保留输入和进度读取；收到完成通知后读取输出和退出码再分析。基础提示用于引导模型减少轮询，不能保证模型完全不查询。

最初试用版的 93 项相关单元测试、36 项集成测试已通过本地模拟模型验证；这不代表后续任何官方合并候选都已通过验证。每个候选以其 Actions 结果为准。原始已验证源码信息见 `ASYNC_BUILD.md`。
