$ErrorActionPreference = 'Stop'

$Repo = 'muninn-hq/aether-sdk'
$BinName = 'aether-mcp.exe'
$Target = 'x86_64-pc-windows-msvc'
$InstallDir = if ($env:AETHER_INSTALL_DIR) {
  $env:AETHER_INSTALL_DIR
} else {
  Join-Path $env:LOCALAPPDATA 'aether-mcp\bin'
}

$Tag = if ($env:AETHER_VERSION) {
  $env:AETHER_VERSION
} else {
  $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
  $Release.tag_name
}

if (-not $Tag) {
  throw 'Could not resolve latest release tag from GitHub.'
}

$Archive = "aether-mcp-$Tag-$Target.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Tag"
$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("aether-mcp-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null

try {
  $ArchivePath = Join-Path $TmpDir $Archive
  $ChecksumPath = Join-Path $TmpDir "$Archive.sha256"

  Write-Host "Downloading aether-mcp $Tag ($Target)..."
  Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile $ArchivePath
  Invoke-WebRequest -Uri "$BaseUrl/$Archive.sha256" -OutFile $ChecksumPath

  $ExpectedHash = ((Get-Content $ChecksumPath -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
  $ActualHash = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($ActualHash -ne $ExpectedHash) {
    throw "Checksum mismatch: expected $ExpectedHash, got $ActualHash"
  }

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Expand-Archive -Path $ArchivePath -DestinationPath $TmpDir -Force
  Copy-Item (Join-Path $TmpDir $BinName) (Join-Path $InstallDir $BinName) -Force

  $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $Entries = if ($UserPath) { $UserPath -split ';' } else { @() }
  if ($Entries -notcontains $InstallDir) {
    $NewPath = (($Entries | Where-Object { $_ -ne '' }) + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
    $env:Path = "$InstallDir;$env:Path"
    Write-Host "Added $InstallDir to your User PATH."
  }

  $InstalledExe = Join-Path $InstallDir $BinName
  Write-Host "Installed aether-mcp $Tag to $InstalledExe"
  # Releases from before `setup` existed do not list it in --help. An empty stdin stops an old
  # build that ignores --help from waiting for input, and a local scope keeps stderr text from
  # becoming a terminating error.
  $HelpText = & {
    $ErrorActionPreference = 'Continue'
    '' | & $InstalledExe --help 2>&1 | Out-String
  }
  $HasSetup = $HelpText -match '(?m)^\s+setup\b'

  if ($HasSetup -and -not $env:AETHER_NO_SETUP -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected) {
    $Answer = Read-Host 'Run the guided setup now? [Y/n]'
    if ($Answer -notmatch '^(n|no)$') {
      & $InstalledExe setup
      return
    }
  }

  Write-Host ''
  Write-Host 'Next steps:'
  if ($HasSetup) {
    Write-Host "  $InstalledExe setup"
    Write-Host '  # or install the service yourself:'
  }
  Write-Host "  $InstalledExe service install --vault C:\path\to\your\vault"
  Write-Host '  # or run in the foreground:'
  Write-Host "  $InstalledExe --vault C:\path\to\your\vault --http-addr 127.0.0.1:14771 --http-only"
} finally {
  Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
}
