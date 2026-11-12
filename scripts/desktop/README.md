# Windows desktop candidates

The upstream synchronization workflow verifies the Windows backend before it advances `async-main`. A separate Windows workflow retains binaries and a manifest for the exact validated commit. It builds direct changes to `async-main` and reuses the validated candidate after an automatic merge.

On the Windows machine, `bootstrap.ps1` fetches the exact current `async-main` commit, extracts that commit's updater scripts without changing the checkout, and runs its `check-and-stage.ps1`. The latter compares the current `async-main` commit and installed Codex package version, downloads the matching successful Windows artifact, checks every binary against its manifest and prepares an isolated desktop candidate. The source application and the current enhanced desktop remain untouched. GitHub CLI authentication and access to the repository are required. Artifacts expire after seven days; if no matching artifact remains, the updater requests a new validated build unless one is already running or the latest build failed.

```powershell
& 'E:\Codex\codex-async-auto-current\scripts\desktop\bootstrap.ps1'
```

The panel patch searches the copied application for the specific composer, terminal, output synchronization and tray contracts it needs. It stops when a contract changes rather than guessing at minified code. The candidate has a `start-candidate.ps1` launcher. The package's executable integrity marker is updated for the changed archive, so the copy no longer has the original vendor signature.

This script **stages** candidates; it does not install one, change shortcuts, close a running session or claim that the desktop interface has been visually validated. The current panel's cross-window live output is still under investigation. Enable automatic activation only after an actual window test verifies the counter, output, close and resize interactions against the candidate.
