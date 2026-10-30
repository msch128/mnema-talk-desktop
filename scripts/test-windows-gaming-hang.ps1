<#
Runs the host built with the test-only `gaming-selftest` feature (a Talk with gaming on
is simulated, "mnema-fakegame.exe" is a known game) next to a stand-in game: a
borderless, topmost, full-screen window that repaints every frame. While the focus
moves between the stand-in game and an ordinary window, and a flood of key events plus
the overlay shortcut (Alt+M) is sent, the host's windows must keep answering: neither
IsHungAppWindow nor a SendMessageTimeout(WM_NULL, SMTO_ABORTIFHUNG) to any of them may
fail. The host also writes logs/desktop.log (started line; stall lines if its watchdog
saw the UI thread blocked); it is printed.

Needs Windows PowerShell 5.1 (Add-Type compiles the stand-in game).
#>
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [int]$Seconds = 25,
  [string]$AppId = 'com.mnema.desktop.shareddev'
)
$ErrorActionPreference = 'Stop'

$work = Join-Path $env:RUNNER_TEMP 'gaming-hang'
if (-not $env:RUNNER_TEMP) { $work = Join-Path $env:TEMP 'gaming-hang' }
New-Item -ItemType Directory -Force $work | Out-Null
$game = Join-Path $work 'mnema-fakegame.exe'
$other = Join-Path $work 'other-window.exe'

Add-Type -ReferencedAssemblies 'System.Windows.Forms', 'System.Drawing' -OutputType WindowsApplication -OutputAssembly $game -TypeDefinition @'
using System;
using System.Drawing;
using System.Windows.Forms;
public static class FakeGame {
  [STAThread]
  public static void Main(string[] args) {
    Application.EnableVisualStyles();
    Form f = new Form();
    bool game = args.Length > 0 && args[0] == "game";
    f.Text = game ? "Fake Game" : "Other Window";
    f.StartPosition = FormStartPosition.Manual;
    if (game) {
      f.FormBorderStyle = FormBorderStyle.None;
      f.Bounds = Screen.PrimaryScreen.Bounds;
      f.TopMost = true;
      f.BackColor = Color.Black;
    } else {
      f.Size = new Size(400, 300);
      f.Location = new Point(40, 40);
    }
    Timer t = new Timer();
    t.Interval = 16;
    t.Tick += delegate { f.Invalidate(); };
    t.Start();
    Application.Run(f);
  }
}
'@
Copy-Item $game $other -Force

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Win {
  delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr p);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsHungAppWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll", SetLastError = true)]
  static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, UIntPtr w, IntPtr l, uint flags, uint timeout, out UIntPtr result);

  public static List<IntPtr> Windows(uint pid, bool onlyVisible) {
    List<IntPtr> found = new List<IntPtr>();
    EnumWindows(delegate (IntPtr h, IntPtr p) {
      uint owner; GetWindowThreadProcessId(h, out owner);
      if (owner == pid && (!onlyVisible || IsWindowVisible(h))) found.Add(h);
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static string Title(IntPtr h) {
    StringBuilder s = new StringBuilder(256);
    GetWindowText(h, s, 256);
    return s.ToString();
  }
  // True when the window's thread answers within the timeout.
  public static bool Answers(IntPtr h, uint ms) {
    UIntPtr r;
    return SendMessageTimeout(h, 0, UIntPtr.Zero, IntPtr.Zero, 0x2 /* SMTO_ABORTIFHUNG */, ms, out r) != IntPtr.Zero;
  }
  public static bool Focus(IntPtr h) {
    keybd_event(0x12, 0, 0, UIntPtr.Zero);      // an Alt tap lets this process take the foreground
    keybd_event(0x12, 0, 2, UIntPtr.Zero);
    SetForegroundWindow(h);
    return GetForegroundWindow() == h;
  }
  public static void Key(byte vk, bool down) { keybd_event(vk, 0, down ? 0u : 2u, UIntPtr.Zero); }
}
'@

$log = Join-Path $env:LOCALAPPDATA "$AppId\logs\desktop.log"
Remove-Item $log -ErrorAction SilentlyContinue

$app = Start-Process -FilePath $Exe -PassThru
$procs = @($app)
try {
  # The host is up when it shows a window.
  $deadline = (Get-Date).AddSeconds(90)
  while ((Get-Date) -lt $deadline -and -not [Win]::Windows($app.Id, $true).Count) {
    if ($app.HasExited) { throw "The host exited with code $($app.ExitCode)" }
    Start-Sleep -Milliseconds 250
  }
  if (-not [Win]::Windows($app.Id, $true).Count) { throw 'The host never showed a window' }
  Start-Sleep -Seconds 3

  $procs += Start-Process -FilePath $other -ArgumentList 'other' -PassThru
  $gameProc = Start-Process -FilePath $game -ArgumentList 'game' -PassThru
  $procs += $gameProc
  Start-Sleep -Seconds 2
  $gameWindow = $null; $otherWindow = $null
  $deadline = (Get-Date).AddSeconds(20)
  while ((Get-Date) -lt $deadline -and (-not $gameWindow -or -not $otherWindow)) {
    $gameWindow = ([Win]::Windows($gameProc.Id, $true) | Select-Object -First 1)
    $otherWindow = ([Win]::Windows($procs[1].Id, $true) | Select-Object -First 1)
    Start-Sleep -Milliseconds 250
  }
  if (-not $gameWindow -or -not $otherWindow) { throw 'The stand-in windows did not appear' }

  $round = 0; $probes = 0; $failures = @(); $focused = 0; $sidepeekSeen = $false; $overlaySeen = $false
  $clock = [Diagnostics.Stopwatch]::StartNew()
  $nextFocus = 0
  while ($clock.Elapsed.TotalSeconds -lt $Seconds) {
    if ($clock.ElapsedMilliseconds -ge $nextFocus) {
      $round++
      $target = if ($round % 2 -eq 1) { $gameWindow } else { $otherWindow }
      if ([Win]::Focus($target) -and $target -eq $gameWindow) { $focused++ }
      # A flood of key events, then the overlay shortcut (Alt+M) twice.
      for ($i = 0; $i -lt 100; $i++) { [Win]::Key(0x41, $true); [Win]::Key(0x41, $false) }
      [Win]::Key(0x12, $true); [Win]::Key(0x4D, $true); [Win]::Key(0x4D, $false); [Win]::Key(0x12, $false)
      $nextFocus = $clock.ElapsedMilliseconds + 700
    }
    foreach ($h in [Win]::Windows($app.Id, $false)) {
      $probes++
      $title = [Win]::Title($h)
      if ($title -like '*Sidepeek*') { $sidepeekSeen = $true }
      if ($title -like '*Gaming DEV*') { $overlaySeen = $true }
      if ([Win]::IsHungAppWindow($h)) { $failures += "hung window '$title' at $($clock.ElapsedMilliseconds) ms" }
      elseif (-not [Win]::Answers($h, 1000)) { $failures += "no answer from '$title' within 1 s at $($clock.ElapsedMilliseconds) ms" }
    }
    Start-Sleep -Milliseconds 50
  }

  Write-Host "probes=$probes focusedGame=$focused sidepeek=$sidepeekSeen overlay=$overlaySeen failures=$($failures.Count)"
  if (Test-Path $log) { Write-Host '--- desktop.log ---'; Get-Content $log | Write-Host } else { Write-Host 'no desktop.log' }
  if ($failures.Count) { $failures | Select-Object -First 20 | ForEach-Object { Write-Host $_ }; throw 'The host stopped answering while a full-screen game took the focus' }
  if ($focused -lt 1) { throw "The stand-in game took the focus only $focused times; the check proved nothing" }
  if (-not $sidepeekSeen) { throw 'The host never detected the stand-in game (no sidepeek window); the check proved nothing' }
  if (-not (Test-Path $log) -or -not (Select-String -Path $log -Pattern ' started ' -Quiet)) { throw 'The host wrote no desktop.log' }
  if ($app.HasExited) { throw 'The host exited during the check' }
}
finally {
  foreach ($p in $procs) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } }
}
