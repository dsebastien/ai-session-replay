$ErrorActionPreference = "Stop"

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$bundleDirectory = Join-Path $repositoryRoot "src-tauri\target\release\bundle\nsis"
$installers = @(
  Get-ChildItem -LiteralPath $bundleDirectory -File -Filter "*_x64-setup.exe" -ErrorAction Stop
)
if ($installers.Count -ne 1) {
  throw "Expected exactly one x64 NSIS installer. Build the release bundle first."
}

$runtimeRoot = (Resolve-Path (Join-Path $repositoryRoot ".runtime")).Path
$smokeRoot = Join-Path $runtimeRoot ("installed-smoke-" + [Guid]::NewGuid().ToString("N"))
$installRoot = Join-Path $smokeRoot "application"
$smokeProfile = Join-Path $smokeRoot "profile"
$roaming = Join-Path $smokeProfile "AppData\Roaming"
$local = Join-Path $smokeProfile "AppData\Local"
$claude = Join-Path $smokeRoot "sources\claude"
$codex = Join-Path $smokeRoot "sources\codex"
$copilot = Join-Path $smokeRoot "sources\copilot"
$temporary = Join-Path $smokeRoot "temp"
New-Item -ItemType Directory -Path $installRoot, $roaming, $local, $claude, $codex, $copilot, $temporary | Out-Null

$isolatedEnvironment = @{
  USERPROFILE = $smokeProfile
  APPDATA = $roaming
  LOCALAPPDATA = $local
  TEMP = $temporary
  TMP = $temporary
  CLAUDE_CONFIG_DIR = $claude
  CODEX_HOME = $codex
  COPILOT_HOME = $copilot
}

function Wait-CheckedProcess {
  param(
    [Parameter(Mandatory)] [System.Diagnostics.Process] $Process,
    [Parameter(Mandatory)] [string] $Label,
    [int] $TimeoutMs = 180000
  )
  if (-not $Process.WaitForExit($TimeoutMs)) {
    $Process.Kill($true)
    throw "$Label timed out."
  }
  if ($Process.ExitCode -ne 0) {
    throw "$Label failed with exit code $($Process.ExitCode)."
  }
}

function Start-IsolatedProcess {
  param(
    [Parameter(Mandatory)] [string] $FilePath,
    [string[]] $Arguments = @(),
    [Parameter(Mandatory)] [hashtable] $Environment
  )
  $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
  $startInfo.FileName = $FilePath
  $startInfo.UseShellExecute = $false
  $startInfo.CreateNoWindow = $true
  $startInfo.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
  foreach ($argument in $Arguments) {
    [void] $startInfo.ArgumentList.Add($argument)
  }
  $startInfo.Environment.Clear()
  foreach ($name in @(
    "SystemRoot", "WINDIR", "SystemDrive", "ProgramData", "ProgramFiles",
    "ProgramFiles(x86)", "CommonProgramFiles", "CommonProgramFiles(x86)",
    "PROCESSOR_ARCHITECTURE", "NUMBER_OF_PROCESSORS"
  )) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if (-not [string]::IsNullOrEmpty($value)) {
      $startInfo.Environment[$name] = $value
    }
  }
  foreach ($entry in $Environment.GetEnumerator()) {
    $startInfo.Environment[$entry.Key] = [string] $entry.Value
  }
  return [System.Diagnostics.Process]::Start($startInfo)
}

function Install-IsolatedPackage {
  $installer = Start-IsolatedProcess `
    -FilePath $installers[0].FullName `
    -Arguments @("/S", "/D=$installRoot") `
    -Environment $isolatedEnvironment
  Wait-CheckedProcess -Process $installer -Label "NSIS install"
}

function Invoke-InstalledSmoke {
  param(
    [Parameter(Mandatory)] [string] $Executable,
    [Parameter(Mandatory)] [string] $JobId
  )
  $appEnvironment = $isolatedEnvironment.Clone()
  $appEnvironment.PATH = ""
  $appEnvironment.AI_SESSION_REPLAY_RUNTIME_SMOKE_JOB = $JobId
  $application = Start-IsolatedProcess `
    -FilePath $Executable `
    -Environment $appEnvironment
  Wait-CheckedProcess -Process $application -Label "Installed-runtime smoke" -TimeoutMs 60000
}

try {
  Install-IsolatedPackage

  $installedExecutables = @(
    Get-ChildItem -LiteralPath $installRoot -Recurse -File -Filter "ai-session-replay.exe"
  )
  if ($installedExecutables.Count -ne 1) {
    throw "The installed application executable is missing or ambiguous."
  }
  $resolvedInstallRoot = (Resolve-Path -LiteralPath $installRoot).Path
  $executable = (Resolve-Path -LiteralPath $installedExecutables[0].FullName).Path
  if (-not $executable.StartsWith($resolvedInstallRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "The installed executable resolved outside the isolated install directory."
  }

  Invoke-InstalledSmoke -Executable $executable -JobId "runtime_smoke_initial_$PID"

  $databases = @(
    Get-ChildItem -LiteralPath $roaming -Recurse -File -Filter "session-library-v3.sqlite3"
  )
  if ($databases.Count -ne 1) {
    throw "The installed application did not create exactly one isolated database."
  }
  $databasePath = $databases[0].FullName
  $databaseHash = (Get-FileHash -LiteralPath $databasePath -Algorithm SHA256).Hash

  Install-IsolatedPackage
  if (-not (Test-Path -LiteralPath $databasePath -PathType Leaf)) {
    throw "NSIS reinstall removed the app-owned database."
  }
  if ((Get-FileHash -LiteralPath $databasePath -Algorithm SHA256).Hash -ne $databaseHash) {
    throw "NSIS reinstall modified the app-owned database before startup."
  }
  Invoke-InstalledSmoke -Executable $executable -JobId "runtime_smoke_upgrade_$PID"

  Write-Output "Installed-runtime smoke and reinstall-retention check passed with isolated data roots and global PATH disabled."
}
finally {
  $uninstaller = Join-Path $installRoot "uninstall.exe"
  if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
    $uninstall = Start-IsolatedProcess `
      -FilePath $uninstaller `
      -Arguments @("/S") `
      -Environment $isolatedEnvironment
    Wait-CheckedProcess -Process $uninstall -Label "NSIS uninstall"
  }
  if (Test-Path -LiteralPath $smokeRoot) {
    for ($attempt = 0; $attempt -lt 20 -and (Test-Path -LiteralPath $smokeRoot); $attempt++) {
      $resolvedSmoke = Resolve-Path -LiteralPath $smokeRoot
      $attributes = (Get-Item -LiteralPath $resolvedSmoke.Path -Force).Attributes
      if (
        -not $resolvedSmoke.Path.StartsWith($runtimeRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        ($attributes -band [IO.FileAttributes]::ReparsePoint)
      ) {
        throw "Refusing to remove an unsafe runtime-smoke directory."
      }
      try {
        Remove-Item -LiteralPath $resolvedSmoke.Path -Recurse -Force -ErrorAction Stop
      }
      catch {
        if ($attempt -eq 19) { throw }
        Start-Sleep -Milliseconds 250
      }
    }
  }
}
