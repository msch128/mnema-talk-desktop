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

$tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$work = Join-Path $tempRoot 'gaming-hang'
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
  [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] static extern void SwitchToThisWindow(IntPtr h, bool alt);
  [DllImport("user32.dll")] static extern bool SystemParametersInfo(uint action, uint param, IntPtr value, uint flags);

  // "title (process.exe)" of a window, for messages that say who held the foreground.
  public static string Describe(IntPtr h) {
    if (h == IntPtr.Zero) return "no window";
    uint pid; GetWindowThreadProcessId(h, out pid);
    string name = "?";
    try { name = System.Diagnostics.Process.GetProcessById((int)pid).ProcessName + ".exe"; } catch { }
    return "'" + Title(h) + "' (" + name + ")";
  }
  // Windows lets SetForegroundWindow through only sometimes, so one call is not enough. Each
  // attempt joins the input queue of the current foreground thread, taps Alt, restores,
  // raises and switches to the window, then checks GetForegroundWindow. Returns true as soon
  // as the window is in front; `log` has one line per attempt either way.
  public static bool Acquire(IntPtr h, int attempts, int gapMs, out string log) {
    StringBuilder sb = new StringBuilder();
    SystemParametersInfo(0x2001 /* SPI_SETFOREGROUNDLOCKTIMEOUT */, 0, IntPtr.Zero, 0);
    bool ok = false;
    for (int n = 1; n <= attempts && !ok; n++) {
      IntPtr fg = GetForegroundWindow();
      if (fg != h) {
        uint pid; uint fgThread = fg == IntPtr.Zero ? 0 : GetWindowThreadProcessId(fg, out pid);
        uint me = GetCurrentThreadId();
        bool attached = fgThread != 0 && fgThread != me && AttachThreadInput(me, fgThread, true);
        keybd_event(0x12, 0, 0, UIntPtr.Zero);      // an Alt tap lets this process take the foreground
        keybd_event(0x12, 0, 2, UIntPtr.Zero);
        if (IsIconic(h)) ShowWindow(h, 9 /* SW_RESTORE */);
        BringWindowToTop(h);
        SetForegroundWindow(h);
        SwitchToThisWindow(h, true);
        if (attached) AttachThreadInput(me, fgThread, false);
        System.Threading.Thread.Sleep(gapMs);
      }
      ok = GetForegroundWindow() == h;
      sb.Append("attempt " + n + ": " + (ok ? "in front" : "held by " + Describe(GetForegroundWindow())) + "\n");
    }
    log = sb.ToString();
    return ok;
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
  # A cold runner can keep the UI thread busy for many seconds right after the first window
  # appears (WebView2 start-up, first-run scans). That is not what this check is about, so
  # measure only once the host has answered 20 probes in a row (about 2 s), within 60 s, and
  # print how long it took so a slow start is visible instead of silently failing the run.
  $shown = [Diagnostics.Stopwatch]::StartNew()
  $answeredInARow = 0
  while ($answeredInARow -lt 20 -and $shown.Elapsed.TotalSeconds -lt 60) {
    if ($app.HasExited) { throw "The host exited with code $($app.ExitCode)" }
    $ok = $true
    foreach ($h in [Win]::Windows($app.Id, $false)) { if ([Win]::IsHungAppWindow($h) -or -not [Win]::Answers($h, 1000)) { $ok = $false } }
    if ($ok) { $answeredInARow++ } else { $answeredInARow = 0 }
    Start-Sleep -Milliseconds 100
  }
  if ($answeredInARow -lt 20) { throw 'The host did not settle within 60 s of showing its window' }
  Write-Host "host settled $([int]$shown.Elapsed.TotalMilliseconds) ms after its first window"
  Start-Sleep -Seconds 1

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

  $round = 0; $probes = 0; $failures = @(); $focused = 0; $focusTries = 0; $lastFocusLog = ''; $sidepeekSeen = $false; $overlaySeen = $false
  $clock = [Diagnostics.Stopwatch]::StartNew()
  $nextFocus = 0
  while ($clock.Elapsed.TotalSeconds -lt $Seconds) {
    if ($clock.ElapsedMilliseconds -ge $nextFocus) {
      $round++
      $target = if ($round % 2 -eq 1) { $gameWindow } else { $otherWindow }
      $focusLog = ''
      $got = [Win]::Acquire($target, 8, 150, [ref]$focusLog)
      $name = if ($target -eq $gameWindow) { 'game' } else { 'other' }
      Write-Host "round $round focus $name -> $(if ($got) { 'ok' } else { 'FAILED' })`n$($focusLog.TrimEnd())"
      if ($target -eq $gameWindow) { $focusTries++; $lastFocusLog = $focusLog.TrimEnd(); if ($got) { $focused++ } }
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
      if ([Win]::IsHungAppWindow($h)) { $failures += "hung window '$title' at $($clock.ElapsedMilliseconds) ms (foreground: '$([Win]::Title([Win]::GetForegroundWindow()))', $($shown.Elapsed.TotalSeconds.ToString('0.0')) s since the host showed a window)" }
      elseif (-not [Win]::Answers($h, 1000)) { $failures += "no answer from '$title' within 1 s at $($clock.ElapsedMilliseconds) ms (foreground: '$([Win]::Title([Win]::GetForegroundWindow()))', $($shown.Elapsed.TotalSeconds.ToString('0.0')) s since the host showed a window)" }
    }
    Start-Sleep -Milliseconds 50
  }

  Write-Host "probes=$probes focusedGame=$focused sidepeek=$sidepeekSeen overlay=$overlaySeen failures=$($failures.Count)"
  if (Test-Path $log) { Write-Host '--- desktop.log ---'; Get-Content $log | Write-Host } else { Write-Host 'no desktop.log' }
  if ($failures.Count) { $failures | Select-Object -First 20 | ForEach-Object { Write-Host $_ }; throw 'The host stopped answering while a full-screen game took the focus' }
  if ($focused -lt 1) { throw "The stand-in game never took the focus in $focusTries rounds of up to 8 attempts each; the check proved nothing. Last round: $lastFocusLog. Foreground now: $([Win]::Describe([Win]::GetForegroundWindow()))" }
  if (-not $sidepeekSeen) { throw 'The host never detected the stand-in game (no sidepeek window); the check proved nothing' }
  if (-not (Test-Path $log) -or -not (Select-String -Path $log -Pattern ' started ' -Quiet)) { throw 'The host wrote no desktop.log' }
  if ($app.HasExited) { throw 'The host exited during the check' }
}
finally {
  foreach ($p in $procs) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } }
}
