[CmdletBinding()]
param(
    [switch] $AllowUserPathMutation
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw 'The installer PATH regression only supports Windows.'
}
if (-not $AllowUserPathMutation) {
    throw 'Pass -AllowUserPathMutation to confirm this test may temporarily replace the current-user PATH.'
}

function Get-RawUserPath {
    param(
        [Parameter(Mandatory = $true)]
        [Microsoft.Win32.RegistryKey] $EnvironmentKey
    )

    if (-not (@($EnvironmentKey.GetValueNames()) -contains 'Path')) {
        return $null
    }

    return [string] $EnvironmentKey.GetValue(
        'Path',
        $null,
        [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    )
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$testRoot = Join-Path `
    ([System.IO.Path]::GetTempPath()) `
    "codewhale-installer-path-$PID-$([guid]::NewGuid().ToString('N'))"
$stageRoot = Join-Path $testRoot 'source'
$stageInstallerDir = Join-Path $stageRoot 'scripts\installer'
$installDir = Join-Path $testRoot 'installed'
$environmentKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
if ($null -eq $environmentKey) {
    throw 'Could not open or create the current-user Environment registry key.'
}

$originalPathExists = @($environmentKey.GetValueNames()) -contains 'Path'
if ($originalPathExists) {
    $originalPath = Get-RawUserPath -EnvironmentKey $environmentKey
    $originalPathKind = $environmentKey.GetValueKind('Path')
}
else {
    $originalPath = $null
    $originalPathKind = [Microsoft.Win32.RegistryValueKind]::ExpandString
}

try {
    [void] (New-Item -ItemType Directory -Path $stageInstallerDir -Force)
    Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $stageRoot 'LICENSE')
    Copy-Item `
        -LiteralPath (Join-Path $PSScriptRoot 'codewhale.nsi') `
        -Destination (Join-Path $stageInstallerDir 'codewhale.nsi')
    Copy-Item `
        -LiteralPath (Join-Path $PSScriptRoot 'update-user-path.ps1') `
        -Destination (Join-Path $stageInstallerDir 'update-user-path.ps1')

    foreach ($binary in @('codewhale.exe', 'codew.exe', 'codewhale-tui.exe')) {
        [System.IO.File]::WriteAllBytes(
            (Join-Path $stageInstallerDir $binary),
            [byte[]] @(0x4d, 0x5a)
        )
    }

    $makensisCandidates = @(
        "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
        "$env:ProgramFiles\NSIS\makensis.exe"
    )
    $makensis = $makensisCandidates |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($makensis)) {
        throw 'makensis.exe was not found after installing NSIS.'
    }

    Push-Location $stageInstallerDir
    try {
        & $makensis '/DVERSION=0.0.0-test' 'codewhale.nsi'
        if ($LASTEXITCODE -ne 0) {
            throw "makensis.exe exited with code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    $installer = Join-Path $stageInstallerDir 'CodeWhaleSetup.exe'
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw 'CodeWhaleSetup.exe was not produced.'
    }

    $canaries = [System.Collections.Generic.List[string]]::new()
    $index = 1
    while ((($canaries -join ';').Length) -lt 1800) {
        $canaries.Add(('C:\CWCanary\DevelopmentTool{0:D4}\bin' -f $index))
        $index++
    }
    $seedPath = $canaries -join ';'
    $environmentKey.SetValue(
        'Path',
        $seedPath,
        [Microsoft.Win32.RegistryValueKind]::ExpandString
    )

    $installArguments = [System.Collections.Generic.List[string]]::new()
    $installArguments.Add('/S')
    $installArguments.Add("/D=$installDir")
    $installProcess = Start-Process `
        -FilePath $installer `
        -ArgumentList @($installArguments) `
        -Wait `
        -PassThru
    if ($installProcess.ExitCode -ne 0) {
        throw "CodeWhaleSetup.exe exited with code $($installProcess.ExitCode)."
    }

    $expectedBin = Join-Path $installDir 'bin'
    $afterInstall = Get-RawUserPath -EnvironmentKey $environmentKey
    if (([string] $afterInstall) -cne "$seedPath;$expectedBin") {
        throw "The installer did not preserve the seeded long PATH exactly. Before=$($seedPath.Length), after=$(([string] $afterInstall).Length)."
    }
    if ($environmentKey.GetValueKind('Path') -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
        throw 'The installer changed the user PATH registry value kind.'
    }

    $uninstaller = Join-Path $installDir 'Uninstall.exe'
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        throw 'Uninstall.exe was not produced.'
    }
    $uninstallProcess = Start-Process `
        -FilePath $uninstaller `
        -ArgumentList '/S' `
        -Wait `
        -PassThru
    if ($uninstallProcess.ExitCode -ne 0) {
        throw "Uninstall.exe exited with code $($uninstallProcess.ExitCode)."
    }

    $afterUninstall = Get-RawUserPath -EnvironmentKey $environmentKey
    if (([string] $afterUninstall) -cne $seedPath) {
        throw 'Install followed by uninstall did not restore the seeded long PATH exactly.'
    }

    Write-Host "Full NSIS installer PATH regression passed with $($seedPath.Length) characters."
}
finally {
    $uninstaller = Join-Path $installDir 'Uninstall.exe'
    if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
        try {
            [void] (Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -PassThru)
        }
        catch {
            Write-Warning "Best-effort test uninstallation failed: $_"
        }
    }

    if ($originalPathExists) {
        $environmentKey.SetValue('Path', $originalPath, $originalPathKind)
    }
    else {
        $environmentKey.DeleteValue('Path', $false)
    }
    $environmentKey.Close()

    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
