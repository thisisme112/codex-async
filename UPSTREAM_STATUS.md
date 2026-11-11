这是 2026-11-11 对官方提交 e58bf8f5233b51e7cd1679d947e3028c1ed91b06 的首次预检查记录。

发现 39 个文件冲突，可用分支未被推进。首次追上官方版本需要接口适配；配置定时同步不等于这些冲突已解决。后续状态请以 Actions 最新运行报告为准。

# Codex async upstream synchronization

Status: **conflict**

base: `9eddac40d57dd21eedabe063c4676fdc631ba0f4`
upstream: `e58bf8f5233b51e7cd1679d947e3028c1ed91b06`
error: ``

Conflicts (usable branch was not changed):

- `codex-rs/app-server/src/in_process.rs`
- `codex-rs/code-mode-host/src/grpc/mod.rs`
- `codex-rs/code-mode-host/src/grpc/service_tests.rs`
- `codex-rs/code-mode-host/src/grpc/session.rs`
- `codex-rs/code-mode-host/src/host_tests.rs`
- `codex-rs/code-mode-host/src/lib.rs`
- `codex-rs/code-mode-host/src/peer_tests.rs`
- `codex-rs/code-mode-host/tests/grpc.rs`
- `codex-rs/code-mode-host/tests/grpc_tcp.rs`
- `codex-rs/code-mode-host/tests/grpc_timing.rs`
- `codex-rs/code-mode-runtime/src/session_runtime/mod.rs`
- `codex-rs/code-mode/src/grpc_session/mod.rs`
- `codex-rs/code-mode/src/remote_session/connection.rs`
- `codex-rs/core/src/session/handlers.rs`
- `codex-rs/core/src/session/mcp.rs`
- `codex-rs/core/src/session/mod.rs`
- `codex-rs/core/src/session/turn.rs`
- `codex-rs/core/src/session/turn_input.rs`
- `codex-rs/core/src/tasks/mod.rs`
- `codex-rs/core/src/thread_manager.rs`
- `codex-rs/core/src/tools/approvals_tests.rs`
- `codex-rs/core/src/tools/code_mode/execute_handler.rs`
- `codex-rs/core/src/tools/code_mode/mod.rs`
- `codex-rs/core/src/tools/handlers/unified_exec.rs`
- `codex-rs/core/src/tools/handlers/unified_exec/exec_command.rs`
- `codex-rs/core/src/tools/network_approval.rs`
- `codex-rs/core/src/tools/network_approval_tests.rs`
- `codex-rs/core/src/tools/parallel.rs`
- `codex-rs/core/src/tools/runtimes/zsh_fork/unix_escalation.rs`
- `codex-rs/core/src/tools/runtimes/zsh_fork/unix_escalation_tests.rs`
- `codex-rs/core/src/tools/spec_plan.rs`
- `codex-rs/core/src/unified_exec/mod.rs`
- `codex-rs/core/src/unified_exec/process_manager.rs`
- `codex-rs/core/tests/common/test_codex.rs`
- `codex-rs/core/tests/suite/mod.rs`
- `codex-rs/tui/src/lib.rs`
- `codex-rs/tui/tests/suite/directory_trust.rs`
- `codex-rs/tui/tests/suite/focus_palette.rs`
- `codex-rs/tui/tests/suite/reconnect.rs`
