# Pack Claude Code for deployment
$ErrorActionPreference = "Stop"
$Desktop = [Environment]::GetFolderPath("Desktop")
$OutZip = Join-Path $Desktop "claudecode.zip"
$TempDir = Join-Path $env:TEMP "claudecode_pack"

Write-Host "=== Packing Claude Code ==="
Write-Host "Output: $OutZip"

# Clean temp
if (Test-Path $TempDir) { Remove-Item $TempDir -Recurse -Force }
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

# --- 1. Claude Code npm package ---
$NpmPkg = "$env:APPDATA\npm\node_modules\@anthropic-ai\claude-code"
if (Test-Path $NpmPkg) {
    $size = (Get-ChildItem $NpmPkg -Recurse -File | Measure-Object -Property Length -Sum).Sum
    Write-Host "[1/5] Claude Code package: $([math]::Round($size/1MB,2)) MB"
    Copy-Item $NpmPkg -Destination "$TempDir\claude-code-pkg" -Recurse
} else {
    Write-Host "[1/5] WARNING: Claude Code npm package not found"
}

# --- 2. .claude.json ---
$ClaudeJson = "$env:USERPROFILE\.claude.json"
if (Test-Path $ClaudeJson) {
    Write-Host "[2/5] .claude.json config"
    Copy-Item $ClaudeJson -Destination $TempDir
} else {
    Write-Host "[2/5] WARNING: .claude.json not found"
}

# --- 3. .claude/skills ---
$ClaudeSkills = "$env:USERPROFILE\.claude\skills"
if (Test-Path $ClaudeSkills) {
    $size = (Get-ChildItem $ClaudeSkills -Recurse -File | Measure-Object -Property Length -Sum).Sum
    Write-Host "[3/5] .claude/skills: $([math]::Round($size/1MB,2)) MB"
    Copy-Item $ClaudeSkills -Destination "$TempDir\.claude\skills" -Recurse
} else {
    Write-Host "[3/5] WARNING: .claude/skills not found"
}

# --- 4. .agents/skills ---
$AgentSkills = "$env:USERPROFILE\.agents\skills"
if (Test-Path $AgentSkills) {
    $size = (Get-ChildItem $AgentSkills -Recurse -File | Measure-Object -Property Length -Sum).Sum
    Write-Host "[4/5] .agents/skills: $([math]::Round($size/1MB,2)) MB"
    Copy-Item $AgentSkills -Destination "$TempDir\.agents\skills" -Recurse
} else {
    Write-Host "[4/5] WARNING: .agents/skills not found"
}

# --- 5. .agents/.skill-lock.json ---
$SkillLock = "$env:USERPROFILE\.agents\.skill-lock.json"
if (Test-Path $SkillLock) {
    Write-Host "[5/5] .skill-lock.json"
    Copy-Item $SkillLock -Destination "$TempDir\.agents\" -Force
} else {
    Write-Host "[5/5] .skill-lock.json not found, skipping"
}

# --- Create README ---
$Readme = @"
Claude Code Deployment Package
===============================
Exported: $(Get-Date -Format 'yyyy-MM-dd HH:mm')
Source machine: $env:COMPUTERNAME

Contents:
- claude-code-pkg/   : npm global package @anthropic-ai/claude-code
- .claude.json       : Claude Code user config & project trust
- .claude/skills/    : Custom Claude skills
- .agents/skills/    : Agent skills (OpenClaw etc.)
- .agents/.skill-lock.json : Skill lock file

NOT included (machine-specific, regenerate on new machine):
- Sessions, backups, plans, projects, shell-snapshots
- API keys (stored in OS credential manager)

Deployment steps: see README in the assistant instructions.
"@
$Readme | Out-File -FilePath "$TempDir\README.txt" -Encoding UTF8

# --- Zip ---
if (Test-Path $OutZip) { Remove-Item $OutZip -Force }
Compress-Archive -Path "$TempDir\*" -DestinationPath $OutZip -CompressionLevel Optimal

$zipSize = (Get-Item $OutZip).Length
Write-Host ""
Write-Host "=== Done ==="
Write-Host "File: $OutZip"
Write-Host "Size: $([math]::Round($zipSize/1MB,2)) MB"

# Cleanup
Remove-Item $TempDir -Recurse -Force