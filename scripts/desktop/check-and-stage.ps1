param(
    [string]$Repository = 'thisisme112/codex-async',
    [string]$StageRoot = 'E:\Codex\codex-async-staged',
    [string]$ScriptCommit = ''
)
$ErrorActionPreference = 'Stop'
$headLine = git ls-remote --heads "https://github.com/$Repository.git" refs/heads/async-main
if ($LASTEXITCODE -ne 0 -or -not $headLine) { throw 'Unable to read the current async-main commit' }
$head = ($headLine -split '\s+')[0].ToLowerInvariant()
if ($head -notmatch '^[0-9a-f]{40}$') { throw 'Invalid remote commit ID' }
if ($ScriptCommit -and $ScriptCommit -cne $head) { throw 'async-main advanced while loading updater scripts; retry on the new commit' }
$package = Get-AppxPackage -Name OpenAI.Codex | Sort-Object Version -Descending | Select-Object -First 1
if (-not $package) { throw 'Installed Codex package not found' }
$candidate = Join-Path $StageRoot ("desktop-$($package.Version)-$($head.Substring(0,12))")
$candidateInfo = Join-Path $candidate 'WINDOWS_BUILD_INFO.json'
if (Test-Path -LiteralPath $candidateInfo) {
    $existing = Get-Content -LiteralPath $candidateInfo -Raw | ConvertFrom-Json
    if ($existing.source_commit -ceq $head) { Write-Output "Candidate already prepared: $candidate"; return }
    throw 'Existing candidate does not match the current source'
}
if (Test-Path -LiteralPath $candidate) { throw "Incomplete candidate requires inspection: $candidate" }
$rawRuns = gh run list --repo $Repository --workflow async-windows-build.yml --limit 100 --json databaseId,status,conclusion,event
if ($LASTEXITCODE -ne 0) { throw 'Unable to query GitHub Actions runs' }
$runs = @($rawRuns | ConvertFrom-Json | Where-Object { $_.status -eq 'completed' -and $_.conclusion -eq 'success' -and $_.event -in @('push','workflow_run','workflow_dispatch') })
New-Item -ItemType Directory -Path $StageRoot -Force | Out-Null
foreach ($run in $runs) {
    $rawArtifacts = gh api "repos/$Repository/actions/runs/$($run.databaseId)/artifacts"
    if ($LASTEXITCODE -ne 0) { throw "Unable to inspect run $($run.databaseId)" }
    $artifacts = $rawArtifacts | ConvertFrom-Json
    if (-not @($artifacts.artifacts | Where-Object { $_.name -eq 'async-windows-build' -and -not $_.expired }).Count) { continue }
    $download = Join-Path $StageRoot ("download-$($run.databaseId)")
    if (-not (Test-Path -LiteralPath $download)) {
        gh run download $run.databaseId --repo $Repository --name async-windows-build --dir $download
        if ($LASTEXITCODE -ne 0) { throw "Download failed for run $($run.databaseId)" }
    }
    $infoPath = Join-Path $download 'WINDOWS_BUILD_INFO.json'
    if (-not (Test-Path -LiteralPath $infoPath)) { throw 'Windows artifact has no build identity' }
    $info = Get-Content -LiteralPath $infoPath -Raw | ConvertFrom-Json
    if ($info.source_commit -cne $head) { continue }
    & (Join-Path $PSScriptRoot 'prepare-windows.ps1') -SourceApp (Join-Path $package.InstallLocation 'app') -BackendDirectory $download -ExpectedCommit $head -OutputDirectory $candidate
    return
}
Write-Output "No validated Windows build for async-main $head is available; the current installation is unchanged."
