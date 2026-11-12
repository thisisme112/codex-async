param(
    [Parameter(Mandatory=$true)][string]$SourceApp,
    [Parameter(Mandatory=$true)][string]$BackendDirectory,
    [Parameter(Mandatory=$true)][string]$ExpectedCommit,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$source = (Resolve-Path -LiteralPath $SourceApp).Path
$backend = (Resolve-Path -LiteralPath $BackendDirectory).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw "Candidate path already exists: $output" }
if ($output.StartsWith($source + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Candidate cannot be inside the source app' }
if ($ExpectedCommit -notmatch '^[0-9a-f]{40}$') { throw 'ExpectedCommit must be a full lowercase commit ID' }
$manifestPath = Join-Path $backend 'WINDOWS_BUILD_INFO.json'
$build = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($build.source_commit -cne $ExpectedCommit -or $build.target -cne 'x86_64-pc-windows-msvc') { throw 'Backend identity does not match the approved commit and target' }
$names = @('codex.exe','codex-code-mode-host.exe','codex-command-runner.exe','codex-windows-sandbox-setup.exe','codex-windows-sandbox-service.exe')
foreach ($name in $names) {
    $file = Join-Path $backend $name
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing backend file: $name" }
    $expected = $build.files.$name
    if (-not $expected -or (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant() -cne $expected) { throw "Backend checksum mismatch: $name" }
}
python -m unittest discover -s $PSScriptRoot -p 'test_*.py' -v
if ($LASTEXITCODE -ne 0) { throw 'Desktop patch tests failed' }
node (Join-Path $PSScriptRoot 'test-background-panel.cjs')
if ($LASTEXITCODE -ne 0) { throw 'Background panel behavior test failed' }
New-Item -ItemType Directory -Path $output | Out-Null
$candidateApp = Join-Path $output 'app'
robocopy $source $candidateApp /E /COPY:DAT /DCOPY:DAT /R:1 /W:1 /XJ /NFL /NDL /NP /NJH /NJS | Out-Null
if ($LASTEXITCODE -gt 7) { throw "Desktop copy failed: robocopy exit $LASTEXITCODE" }
python (Join-Path $PSScriptRoot 'patch-asar.py') --app $candidateApp
if ($LASTEXITCODE -ne 0) { throw 'Desktop patch is incompatible; keep the current version' }
$resources = Join-Path $candidateApp 'resources'
foreach ($name in $names) { Copy-Item -LiteralPath (Join-Path $backend $name) -Destination (Join-Path $resources $name) -Force }
& (Join-Path $resources 'codex.exe') --version
if ($LASTEXITCODE -ne 0) { throw 'Candidate backend did not start' }
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $output 'WINDOWS_BUILD_INFO.json')
@'
$ErrorActionPreference = 'Stop'
$candidate = Split-Path -Parent $MyInvocation.MyCommand.Path
$env:CODEX_CLI_PATH = Join-Path $candidate 'app\resources\codex.exe'
$env:CODEX_ELECTRON_USER_DATA_PATH = 'E:\Codex\codex-async-ui-copy\profile'
Start-Process -FilePath (Join-Path $candidate 'app\ChatGPT.exe') -ArgumentList '--user-data-dir=E:\Codex\codex-async-ui-copy\web-enhanced'
'@ | Set-Content -LiteralPath (Join-Path $output 'start-candidate.ps1') -Encoding utf8
Write-Output "Prepared candidate: $output"
