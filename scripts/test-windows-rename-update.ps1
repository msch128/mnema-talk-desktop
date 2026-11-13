<#
Up to 1.3.5 the app was called "Mnema Desktop DEV", and the installer names its
folder, its entry under Installed apps and its shortcuts after the product.
This installs such a release, updates it the way the app's updater does, and
checks that the new version took its place: one folder, one entry, the
shortcuts under the new name and pointing at the new program, the data kept,
the new version started. Then it uninstalls and checks that nothing is left.

The update: the feed (latest.json, its signature already checked by
scripts/verify-updater-signatures.mjs) names the installer, and the installer
runs with the arguments the updater gives it in passive mode
(tauri-plugin-updater: /P /UPDATE /R /ARGS). The old release cannot fetch this
feed itself: it trusts only the release key, and this build is signed with a
throwaway one.

It installs and uninstalls programs of the current user: a disposable runner only.

  ./scripts/test-windows-rename-update.ps1 -OldInstaller <1.3.5 setup> -Feed <latest.json>
#>
param(
  [Parameter(Mandatory)][string]$OldInstaller,
  [Parameter(Mandatory)][string]$Feed
)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This installs and uninstalls programs: run it on a disposable CI runner.' }

$oldName = 'Mnema Desktop DEV'
$newName = 'Mnema Desktop'
$uninstallRoot = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$menu = [Environment]::GetFolderPath('Programs')
$desktop = [Environment]::GetFolderPath('Desktop')
# The windows are titled "<name> - Instance" with an em dash.
$dash = [char]0x2014

function Invoke-Limited($file, $arguments, $seconds) {
  $child = Start-Process -FilePath $file -ArgumentList $arguments -PassThru
  if (!$child.WaitForExit($seconds * 1000)) {
    & taskkill.exe /T /F /PID $child.Id | Out-Null
    throw "$file did not finish within ${seconds}s"
  }
  return $child.ExitCode
}
function Entry($name) { Get-ItemProperty -Path (Join-Path $uninstallRoot $name) -ErrorAction SilentlyContinue }
function Entries { @(Get-ChildItem $uninstallRoot -ErrorAction SilentlyContinue | Get-ItemProperty | Where-Object { $_.DisplayName -like 'Mnema Desktop*' }) }
function Folder($entry) { $entry.InstallLocation.Trim('"') }
function Target($lnk) { (New-Object -ComObject WScript.Shell).CreateShortcut($lnk).TargetPath }
function Wait-Window($title) {
  for ($i = 0; $i -lt 90; $i++) {
    $found = Get-Process | Where-Object { $_.MainWindowTitle -eq $title } | Select-Object -First 1
    if ($found) { return $found }
    Start-Sleep -Milliseconds 500
  }
  throw "No window titled '$title' appeared"
}
function Stop-Tree($process) { & taskkill.exe /T /F /PID $process.Id | Out-Null; $process.WaitForExit(10000) | Out-Null }
function Wait-Until($what, [scriptblock]$done) {
  for ($i = 0; $i -lt 120; $i++) { if (& $done) { return }; Start-Sleep -Milliseconds 500 }
  throw $what
}
function Shortcuts { @(Get-ChildItem $menu, $desktop -Filter 'Mnema Desktop*.lnk' -Recurse -ErrorAction SilentlyContinue) }

if ((Entries).Count -ne 0) { throw 'A copy of Mnema Desktop is installed before the check' }

# 1. The last release named DEV, installed the way people did: silently it also makes the desktop shortcut.
$code = Invoke-Limited $OldInstaller '/S' 120
if ($code -ne 0) { throw "The old installer exited with $code" }
$old = Entry $oldName
if (!$old) { throw "The old release made no entry named $oldName" }
$oldDir = Folder $old
$program = if ($old.MainBinaryName) { $old.MainBinaryName } else { 'mnema-native-client-preview.exe' }
if (!(Test-Path (Join-Path $oldDir $program))) { throw "The old program is not in $oldDir" }
foreach ($lnk in "$menu\$oldName.lnk", "$desktop\$oldName.lnk") { if (!(Test-Path $lnk)) { throw "The old release made no shortcut $lnk" } }
Write-Host "Installed $($old.DisplayVersion) in $oldDir"

# 2. It runs once, so that it has data, and leaves a mark in it.
$running = Start-Process -FilePath (Join-Path $oldDir $program) -PassThru
$null = Wait-Window "$oldName $dash Instance"
$data = @("$env:LOCALAPPDATA\com.mnema.desktop.shareddev", "$env:APPDATA\com.mnema.desktop.shareddev") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (!$data) { throw 'The old release wrote no data under its identifier' }
$mark = Join-Path $data 'rename-check.mark'
Set-Content -Path $mark -Value 'kept'
# The updater starts the installer and ends the app at once.
Stop-Tree $running

# 3. The update, as the updater runs it.
$manifest = Get-Content $Feed -Raw | ConvertFrom-Json
$entry = $manifest.platforms.'windows-x86_64'
if (!$entry) { throw 'The feed has no Windows entry' }
$installer = Join-Path (Split-Path $Feed) ([Uri]::UnescapeDataString(([Uri]$entry.url).Segments[-1]))
if (!(Test-Path $installer)) { throw "The feed names $installer, which is not there" }
$code = Invoke-Limited $installer '/P /UPDATE /R /ARGS' 180
if ($code -ne 0) { throw "The update exited with $code" }

# 4. In place: one entry, the new folder, the old one gone, the shortcuts renamed, the data kept.
$new = Entry $newName
if (!$new) { throw "The update made no entry named $newName" }
if ($new.DisplayVersion -ne $manifest.version) { throw "The entry says $($new.DisplayVersion), the feed $($manifest.version)" }
if (Entry $oldName) { throw "The entry $oldName is left" }
if ((Entries).Count -ne 1) { throw "$((Entries).Count) entries for Mnema Desktop under Installed apps" }
$newDir = Folder $new
if ($newDir -ne "$env:LOCALAPPDATA\$newName") { throw "The update went into $newDir" }
if (Test-Path $oldDir) { throw "The old folder $oldDir is left: $(Get-ChildItem $oldDir -Recurse -Name)" }
$newProgram = Join-Path $newDir $new.MainBinaryName
if (!(Test-Path $newProgram)) { throw "No program in $newDir" }
foreach ($lnk in "$menu\$oldName.lnk", "$desktop\$oldName.lnk") { if (Test-Path $lnk) { throw "The old shortcut $lnk is left" } }
foreach ($lnk in "$menu\$newName.lnk", "$desktop\$newName.lnk") {
  if (!(Test-Path $lnk)) { throw "No shortcut $lnk" }
  if ((Target $lnk) -ne $newProgram) { throw "$lnk starts $(Target $lnk)" }
}
if ((Shortcuts).Count -ne 2) { throw "Shortcuts: $((Shortcuts).FullName -join ', ')" }
$leftover = @(Get-ChildItem HKCU:\Software -ErrorAction SilentlyContinue | Where-Object { Test-Path (Join-Path $_.PSPath $oldName) })
if ($leftover) { throw "Registry keys of the old name are left: $($leftover.Name -join ', ')" }
if ((Get-Content $mark -ErrorAction SilentlyContinue) -ne 'kept') { throw 'The data of the old release is gone' }
# /R starts the new version.
$started = Wait-Window "$newName $dash Instance"
if ($started.Path -ne $newProgram) { throw "The started program is $($started.Path)" }
Stop-Tree $started
Write-Host "Updated in place to $($new.DisplayVersion) in $newDir"

# 5. The uninstaller leaves nothing but the data (which it deletes only when asked in its dialog).
$code = Invoke-Limited (Join-Path $newDir 'uninstall.exe') '/S' 120
if ($code -ne 0) { throw "The uninstaller exited with $code" }
# Without `_?=` the uninstaller runs from a copy and its first process returns at once.
Wait-Until 'The uninstaller left the entry' { (Entries).Count -eq 0 }
Wait-Until "The uninstaller left $newDir" { !(Test-Path (Join-Path $newDir $new.MainBinaryName)) -and !(Test-Path (Join-Path $newDir 'uninstall.exe')) }
Wait-Until "Shortcuts are left: $((Shortcuts).FullName -join ', ')" { (Shortcuts).Count -eq 0 }
Write-Host 'Uninstalled: no entry, no program, no shortcut left'
@"
### Update from $($old.DisplayVersion) ("$oldName") to $($new.DisplayVersion) ("$newName")
- one entry under Installed apps, in ``$newDir``; the old folder, entry and shortcuts are gone
- Start menu and desktop shortcuts renamed and pointing at the new program; the data kept
- the new version started after the update; the uninstaller left no entry, program or shortcut
"@ | Out-File -Append -Encoding utf8 $env:GITHUB_STEP_SUMMARY
