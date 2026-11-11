# Codex async completion trial: event waits and capability guidance

This file records the original trial build. Current upstream integration and validation are tracked in [UPSTREAM_STATUS.md](UPSTREAM_STATUS.md).

Base: mkXultra/codex af1f26f7250a4ed9be075d0a38ee1a056ae4e9ac.
This rebuild restores the prior trial runtime changes from the recorded implementation: terminal progress reads preserve completion, successful direct handoff is typed, and initial exec output reports the registered notification. The original scratch checkout was pruned, so its unpublished commits cannot be used as a Git parent.

## Model contract

Live interactive completion-enabled threads add a stable capability fragment to the request copy of the base instructions, including resumed threads. Stored base instructions and inherited user configuration are preserved. The fragment names the tools, explains automatic continuation, and tells the model not to poll solely to check completion. The wait tool is registered only when completion wake is enabled and unified execution is available. Ordinary noninteractive clients retain their existing tools and behavior.

`wait_for_tasks({session_ids:[...]})` suspends a tool call until all selected tracked executions finish. It uses completion events, with no timer or intermediate model request. It returns IDs; `write_stdin` retrieves output and exit status afterward. A user interruption ends the wait without terminating the command. Waits capture execution UUIDs so reused numeric IDs cannot complete an earlier wait.

Terminal reads and writes keep their existing semantics. The stable capability prompt and tool descriptions direct the model to event waits or idle completion instead of polling. This preserves progress diagnostics for long-running servers and interactive commands. Progress reads do not retire completion subscriptions. Long-running servers and input-dependent commands should not use a completion wait.

## Scope

Requires a running local interactive CLI. Completion is not persisted across shutdown/restart. The model can still run other shell commands; this is not an absolute prohibition on arbitrary polling. This development build uses the community snapshot, not current official main. Tests use a local mock model, not a live account. Existing approval and sandbox decisions remain in place.

## Source recovery

The package includes a Git patch against the exact base commit. Clone the community repository, fetch that base, check it out, and apply the patch with `git am`. Keep the whole binary package directory together. Build provenance, validation results, and checksums accompany the archive.
