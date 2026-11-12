$ErrorActionPreference = 'Stop'
$global:XkTestHead = '1111111111111111111111111111111111111111'
$global:XkTestRuns = '[]'
$global:XkDispatchCount = 0
function git { $global:LASTEXITCODE=0; "$global:XkTestHead`trefs/heads/async-main" }
function Get-AppxPackage { [pscustomobject]@{Version='1.2.3.4'; InstallLocation='unused'} }
function gh {
    $global:LASTEXITCODE=0
    if ($args[0] -eq 'run' -and $args[1] -eq 'list') { return $global:XkTestRuns }
    if ($args[0] -eq 'workflow' -and $args[1] -eq 'run') { $global:XkDispatchCount++; return }
    throw "Unexpected mock GH command: $args"
}
$root = Join-Path ([IO.Path]::GetTempPath()) ("xk-stage-test-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $root | Out-Null
$script = Join-Path $PSScriptRoot 'check-and-stage.ps1'
& $script -StageRoot (Join-Path $root 'missing')
if ($global:XkDispatchCount -ne 1) { throw 'Missing artifact did not request exactly one build' }
$global:XkTestRuns = '[{"databaseId":7,"status":"in_progress","conclusion":"","event":"push","headSha":"1111111111111111111111111111111111111111"}]'
& $script -StageRoot (Join-Path $root 'running')
if ($global:XkDispatchCount -ne 1) { throw 'A duplicate build was requested' }
$global:XkTestRuns = '[{"databaseId":8,"status":"completed","conclusion":"failure","event":"push","headSha":"1111111111111111111111111111111111111111"}]'
try { & $script -StageRoot (Join-Path $root 'failed'); throw 'Expected failure was not reported' }
catch { if ($_.Exception.Message -notmatch 'failed in run 8') { throw } }
if ($global:XkDispatchCount -ne 1) { throw 'A failed build was retried unexpectedly' }
Write-Output 'UPDATE_ORCHESTRATION_OK'
