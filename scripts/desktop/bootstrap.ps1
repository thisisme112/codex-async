param(
    [string]$RepositoryDirectory = 'E:\Codex\codex-async-windows',
    [string]$StageRoot = 'E:\Codex\codex-async-staged'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $RepositoryDirectory).Path
$url = (git -C $root remote get-url origin).Trim()
if ($LASTEXITCODE -ne 0 -or $url -ne 'https://github.com/thisisme112/codex-async.git') { throw 'Unexpected repository origin' }
git -C $root fetch --no-tags origin refs/heads/async-main
if ($LASTEXITCODE -ne 0) { throw 'Unable to fetch async-main' }
$sha = (git -C $root rev-parse FETCH_HEAD).Trim().ToLowerInvariant()
if ($LASTEXITCODE -ne 0 -or $sha -notmatch '^[0-9a-f]{40}$') { throw 'Invalid fetched commit' }
New-Item -ItemType Directory -Path $StageRoot -Force | Out-Null
$scriptRoot = Join-Path $StageRoot "scripts-$sha"
if (-not (Test-Path -LiteralPath $scriptRoot)) {
    New-Item -ItemType Directory -Path $scriptRoot | Out-Null
    $archive = Join-Path $scriptRoot 'scripts.tar'
    git -C $root archive --format=tar --output=$archive $sha scripts/desktop
    if ($LASTEXITCODE -ne 0) { throw 'Unable to extract updater scripts from the validated branch' }
    tar -xf $archive -C $scriptRoot
    if ($LASTEXITCODE -ne 0) { throw 'Unable to unpack updater scripts' }
}
$script = Join-Path $scriptRoot 'scripts\desktop\check-and-stage.ps1'
if (-not (Test-Path -LiteralPath $script -PathType Leaf)) { throw 'The fetched commit has no desktop updater' }
& $script -StageRoot $StageRoot -ScriptCommit $sha
