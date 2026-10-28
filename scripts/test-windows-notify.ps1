<#
Installs the NSIS installer silently, checks the Start-menu shortcut carries the
AppUserModelID that the toasts use, runs the installed app in the notification
self-test (a build with the `notify-selftest` feature, started with
MNEMA_NOTIFY_SELFTEST naming a result file) and checks that Windows really
holds the toast: ToastNotificationManager.History.GetHistory(<aumid>).

Needs Windows PowerShell 5.1 (the WinRT projection of Windows.UI.Notifications).
A click on the banner is tried through UI Automation; it is reported, and only
required with -RequireClick, because a runner's desktop may not show banners.
#>
param(
  [Parameter(Mandatory = $true)][string]$Installer,
  [string]$AppId = 'com.mnema.desktop.shareddev',
  [string]$Product = 'Mnema Desktop DEV',
  [switch]$RequireClick
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ShortcutAumid {
  [StructLayout(LayoutKind.Sequential, Pack = 4)]
  struct PROPERTYKEY { public Guid fmtid; public uint pid; }
  [StructLayout(LayoutKind.Explicit, Size = 24)]
  struct PROPVARIANT { [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr p; }
  [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IPropertyStore {
    [PreserveSig] int GetCount(out uint count);
    [PreserveSig] int GetAt(uint index, out PROPERTYKEY key);
    [PreserveSig] int GetValue(ref PROPERTYKEY key, out PROPVARIANT value);
  }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
  static extern void SHGetPropertyStoreFromParsingName(string path, IntPtr bc, int flags, ref Guid riid, out IPropertyStore store);
  [DllImport("ole32.dll")] static extern int PropVariantClear(ref PROPVARIANT value);
  public static string Read(string lnk) {
    Guid iid = new Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99");
    IPropertyStore store;
    SHGetPropertyStoreFromParsingName(lnk, IntPtr.Zero, 0, ref iid, out store);
    PROPERTYKEY key = new PROPERTYKEY();
    key.fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"); // System.AppUserModel.ID
    key.pid = 5;
    PROPVARIANT value;
    int hr = store.GetValue(ref key, out value);
    if (hr != 0) throw new COMException("GetValue failed", hr);
    string text = (value.vt == 31 || value.vt == 8) ? Marshal.PtrToStringUni(value.p) : null;
    PropVariantClear(ref value);
    return text;
  }
}
'@

[void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]

function Fail($message) { throw $message }

# 1. Install per user, silently.
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -ne 0) { Fail "Installer exit code $($p.ExitCode)" }

$installDir = $null
$completed = $false
try {
  # 2. The Start-menu shortcut names the app by the ID the toasts use.
  $menu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
  $lnk = Get-ChildItem $menu -Filter "$Product*.lnk" -Recurse | Select-Object -First 1
  if (!$lnk) { Fail "No Start-menu shortcut for $Product under $menu" }
  $aumid = [ShortcutAumid]::Read($lnk.FullName)
  Write-Host "Shortcut $($lnk.FullName) has AppUserModelID '$aumid'"
  if ($aumid -ne $AppId) { Fail "The shortcut carries '$aumid', the toasts use '$AppId'" }

  $exe = Get-ChildItem $env:LOCALAPPDATA -Include 'mnema-native-client-preview.exe', "$Product.exe" -Recurse -Depth 3 -ErrorAction SilentlyContinue | Select-Object -First 1
  if (!$exe) { Fail 'Installed program not found in the user profile' }
  $installDir = $exe.DirectoryName

  # 3. The installed app posts one toast on its own.
  $result = Join-Path $env:RUNNER_TEMP 'notify-selftest.txt'
  Remove-Item $result -ErrorAction SilentlyContinue
  $env:MNEMA_NOTIFY_SELFTEST = $result
  $env:MNEMA_NOTIFY_SELFTEST_WAIT = '40'
  $env:MNEMA_DESKTOP_NO_UPDATE_CHECK = '1'
  $process = Start-Process -FilePath $exe.FullName -PassThru
  function Lines { if (Test-Path $result) { @(Get-Content $result) } else { @() } }
  try {
    $shown = $false
    for ($i = 0; $i -lt 120 -and !$shown; $i++) {
      Start-Sleep -Milliseconds 500
      $process.Refresh()
      if ($process.HasExited) { Fail "The installed app exited early: $($process.ExitCode); result: $((Lines) -join ' | ')" }
      $shown = (Lines) -contains 'shown=ok'
    }
    if (!$shown) { Fail "The app did not report a shown toast: $((Lines) -join ' | ')" }

    # 4. Windows holds the toast for this app ID.
    $found = $null
    for ($i = 0; $i -lt 20 -and !$found; $i++) {
      $history = [Windows.UI.Notifications.ToastNotificationManager]::History.GetHistory($aumid)
      foreach ($toast in $history) {
        $xml = $toast.Content.GetXml()
        if ($xml -like '*Mnema selftest*') { $found = $xml; break }
      }
      if (!$found) { Start-Sleep -Milliseconds 500 }
    }
    if (!$found) { Fail "ToastNotificationManager.History.GetHistory('$aumid') does not hold the toast" }
    Write-Host "History holds the toast: $found"

    # 5. Using the toast. Best effort: find its banner and press it.
    Add-Type -AssemblyName UIAutomationClient
    $clicked = $false
    for ($i = 0; $i -lt 20 -and !$clicked; $i++) {
      $name = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, 'Mnema selftest')
      $banner = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $name)
      if ($banner) {
        try {
          $pattern = $banner.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
          $pattern.Invoke()
        } catch {
          $box = $banner.Current.BoundingRectangle
          Add-Type -AssemblyName System.Windows.Forms
          [System.Windows.Forms.Cursor]::Position = [System.Drawing.Point]::new([int]($box.X + $box.Width / 2), [int]($box.Y + $box.Height / 2))
          Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public static class Mouse { [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, System.UIntPtr e); }'
          [Mouse]::mouse_event(0x2, 0, 0, 0, [System.UIntPtr]::Zero); [Mouse]::mouse_event(0x4, 0, 0, 0, [System.UIntPtr]::Zero)
        }
        Start-Sleep -Seconds 2
        $clicked = ((Lines) | Where-Object { $_ -like 'clicked *' }).Count -gt 0
      } else { Start-Sleep -Milliseconds 500 }
    }
    # The activation handler, called the way the WinRT event calls it (no banner can be pressed here).
    $synthetic = $false
    for ($i = 0; $i -lt 40 -and !$synthetic; $i++) {
      $synthetic = (Lines) -contains 'synthetic-activation reported=true stale-dropped=true'
      if (!$synthetic) { Start-Sleep -Milliseconds 500 }
    }
    if (!$synthetic) { Fail "The activation handler did not report the current click and drop the stale one: $((Lines) -join ' | ')" }
    $lines = Lines
    Write-Host "Self-test record: $($lines -join ' | ')"
    if ($clicked) { Write-Host 'The toast click reached the app (activation callback).' }
    elseif ($RequireClick) { Fail 'No click reached the app' }
    else { Write-Host 'NOTE: the banner could not be pressed on this runner; the click is not verified here.' }
  } finally {
    if (!$process.HasExited) { Stop-Process -Id $process.Id -Force }
    Remove-Item Env:MNEMA_NOTIFY_SELFTEST, Env:MNEMA_NOTIFY_SELFTEST_WAIT, Env:MNEMA_DESKTOP_NO_UPDATE_CHECK -ErrorAction SilentlyContinue
  }
  $completed = $true
} finally {
  # 6. Uninstall removes the shortcut, also when an earlier check failed.
  if (!$installDir) {
    $found = Get-ChildItem $env:LOCALAPPDATA -Include 'uninstall.exe' -Recurse -Depth 3 -ErrorAction SilentlyContinue | Where-Object { $_.DirectoryName -like "*$Product*" } | Select-Object -First 1
    if ($found) { $installDir = $found.DirectoryName }
  }
  $uninstall = if ($installDir) { Join-Path $installDir 'uninstall.exe' } else { $null }
  if ($uninstall -and (Test-Path $uninstall)) {
    Start-Process -FilePath $uninstall -ArgumentList '/S' -Wait
    $leftover = @(Get-ChildItem $menu -Filter "$Product*.lnk" -Recurse -ErrorAction SilentlyContinue)
    if ($leftover.Count -gt 0) {
      if ($completed) { Fail "Uninstall left the Start-menu shortcut: $($leftover[0].FullName)" }
      Write-Warning "Uninstall left the Start-menu shortcut: $($leftover[0].FullName)"
    }
  } elseif ($completed) { Fail 'Uninstaller missing' }
}
Write-Host 'OK'
