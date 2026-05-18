# DeepSeek-TUI Dev Container — rebuild image and launch
# Usage: .\dev-container.ps1 [-WorkspacePath <path>] [-EnvFile <path>] [-NoCache]
#   WorkspacePath defaults to the parent directory (..)
#   EnvFile      defaults to searching: $PSScriptRoot\.env, then parent dirs
#   NoCache      forces a full Docker rebuild (no layer caching)

param(
    [string]$WorkspacePath,
    [string]$EnvFile,
    [switch]$NoCache
)

$ErrorActionPreference = "Stop"

if (-not $WorkspacePath) {
    # Default: mount the project root's parent so the workspace is available
    $WorkspacePath = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
}

# ── Resolve .env file ────────────────────────────────────────────────
if (-not $EnvFile) {
    # Search priority:
    # 1. $PSScriptRoot\.env (next to this script)
    # 2. Walk up from $PSScriptRoot (up to 3 levels) to find .env
    $candidate = Join-Path $PSScriptRoot ".env"
    if (Test-Path $candidate) {
        $EnvFile = $candidate
    } else {
        $parent = $PSScriptRoot
        for ($i = 0; $i -lt 3 -and -not $EnvFile; $i++) {
            $parent = Split-Path $parent -Parent
            if ($parent) {
                $candidate = Join-Path $parent ".env"
                if (Test-Path $candidate) {
                    $EnvFile = $candidate
                }
            }
        }
    }
}

if ($EnvFile) {
    Write-Host "    Env file: $EnvFile" -ForegroundColor Gray
} else {
    Write-Host "    WARNING: No .env file found. Set -EnvFile or create .env next to this script." -ForegroundColor Yellow
    Write-Host "    The container will start without GH_TOKEN / DEEPSEEK_API_KEY." -ForegroundColor Yellow
}

# ── Generate gh hosts.yml from .env ──────────────────────────────────
# gh CLI reads ~/.config/gh/hosts.yml for file-based auth. This bypasses
# the need for GH_TOKEN in the environment (which the TUI's exec_shell
# strips via sanitized_child_env). We create the file on the host and
# bind-mount it into the container.
$ghHostsFile = $null
$ghMountArgs = @()
if ($EnvFile -and (Test-Path $EnvFile)) {
    $envContent = Get-Content $EnvFile -Raw
    if ($envContent -match 'GH_TOKEN[=](.+)') {
        $token = ($Matches[1] -split '\r?\n')[0].Trim()
        if ($token) {
            $ghHostsFile = Join-Path $env:TEMP "deepseek-gh-hosts-$pid.yml"
            @"
github.com:
    oauth_token: $token
    git_protocol: https
"@ | Out-File -FilePath $ghHostsFile -Encoding ascii -NoNewline
            $ghMountArgs = @("-v", "${ghHostsFile}:/home/deepseek/.config/gh/hosts.yml:ro")
            Write-Host "    gh hosts.yml generated for file-based auth" -ForegroundColor Gray
        }
    }
}

# ── Build ─────────────────────────────────────────────────────────────
$buildArgs = @("build", "-t", "deepseek-tui:dev", "-f", "$PSScriptRoot\Dockerfile.dev")
if ($NoCache) {
    $buildArgs += "--no-cache"
    Write-Host "==> Building image deepseek-tui:dev (--no-cache) ..." -ForegroundColor Cyan
} else {
    Write-Host "==> Building image deepseek-tui:dev ..." -ForegroundColor Cyan
    Write-Host "    (use -NoCache to force a full rebuild)" -ForegroundColor Gray
}
docker @buildArgs $PSScriptRoot

if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed." -ForegroundColor Red
    # Clean up temp file
    if ($ghHostsFile -and (Test-Path $ghHostsFile)) { Remove-Item $ghHostsFile }
    exit $LASTEXITCODE
}

# ── Run ───────────────────────────────────────────────────────────────
Write-Host "==> Running container ..." -ForegroundColor Cyan
Write-Host "    Workspace: $WorkspacePath" -ForegroundColor Gray
Write-Host "    Mounted at /workspace" -ForegroundColor Gray

$envFileArgs = @()
$envMountArgs = @()
if ($EnvFile -and (Test-Path $EnvFile)) {
    $envFileArgs = @("--env-file", $EnvFile)
    # Also mount the .env file so the entrypoint can source it as fallback
    $envMountArgs = @("-v", "${EnvFile}:/home/deepseek/.deepseek/dev.env:ro")
}

try {
    docker run --rm -it `
        -e DEEPSEEK_API_KEY `
        @envFileArgs `
        @envMountArgs `
        @ghMountArgs `
        -v deepseek-tui-dev-home:/home/deepseek/.deepseek `
        -v "${WorkspacePath}:/workspace" `
        -w /workspace `
        deepseek-tui:dev
} finally {
    # Clean up the temp hosts.yml file
    if ($ghHostsFile -and (Test-Path $ghHostsFile)) {
        Remove-Item $ghHostsFile -Force -ErrorAction SilentlyContinue
    }
}
