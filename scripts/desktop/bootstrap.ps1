param(
    [string]$RepositoryDirectory = 'E:\Codex\codex-async-windows',
    [string]$StageRoot = 'E:\Codex\codex-async-staged'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $RepositoryDirectory).Path
$url = (git -C $root remote get-url origin).Trim()
if ($LASTEXITCODE -ne 0 -or $url -ne 'https://github.com/thisisme112/codex-async.git') { throw 'Unexpected repository origin' }
$headLine = git -C $root ls-remote --heads origin refs/heads/async-main
if ($LASTEXITCODE -ne 0 -or -not $headLine) { throw 'Unable to read async-main' }
$sha = ($headLine -split '\s+')[0].ToLowerInvariant()
if ($sha -notmatch '^[0-9a-f]{40}$') { throw 'Invalid remote commit ID' }
git -C $root fetch --no-tags origin $sha
if ($LASTEXITCODE -ne 0) { throw 'Unable to fetch the exact async-main commit' }
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
$tracked = @(git -C $root ls-tree -r --name-only $sha -- scripts/desktop)
if ($LASTEXITCODE -ne 0 -or $tracked.Count -eq 0) { throw 'Unable to enumerate the pinned updater files' }
foreach ($relative in $tracked) {
    $file = Join-Path $scriptRoot $relative
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing pinned updater file: $relative" }
    if ((Get-Item -LiteralPath $file -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Updater file is a reparse point: $relative" }
    $expectedBlob = (git -C $root rev-parse "$sha`:$relative").Trim()
    if ($LASTEXITCODE -ne 0) { throw "Unable to resolve updater file: $relative" }
    $actualBlob = (git -C $root hash-object -- $file).Trim()
    if ($LASTEXITCODE -ne 0 -or $actualBlob -cne $expectedBlob) { throw "Pinned updater file changed: $relative" }
}
& $script -StageRoot $StageRoot -ScriptCommit $sha
