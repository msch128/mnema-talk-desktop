# Reads the WebView2 probe record written by the desktop host.
# Prints OK when every required line is present and every reported origin is
# the mapped custom-protocol origin. Prints WAIT while lines are still
# missing. Throws when a line contradicts the mapping.
param(
  [Parameter(Mandatory)][string]$LogPath
)
$ErrorActionPreference = 'Stop'
if (!(Test-Path -LiteralPath $LogPath)) { Write-Output 'WAIT no record yet'; return }
$text = Get-Content -LiteralPath $LogPath -Raw
if ([string]::IsNullOrEmpty($text)) { Write-Output 'WAIT empty record'; return }

$badOrigin = [regex]::Matches($text, '(?m)^\[probe\] \S+ origin .+$') | Where-Object {
  $_.Value -notmatch '^\[probe\] \S+ origin https://mnema\.localhost/(?:[A-Za-z0-9._~!$&''()*+,;=:@%/-]*)?$'
}
if ($badOrigin) { throw "WebView2 reported an unexpected origin: $($badOrigin[0].Value)" }

$required = @(
  '(?m)^\[probe\] main origin https://mnema\.localhost/',
  '(?m)^\[perm\] main Microphone Deny$',
  '(?m)^\[probe\] main media NotAllowedError$',
  '(?m)^\[probe\] instance origin https://mnema\.localhost/',
  '(?m)^\[perm\] instance Microphone Default$',
  '(?m)^\[perm\] instance Camera Default$',
  '(?m)^\[probe\] instance media granted$',
  '(?m)^\[probe\] sidepeek origin https://mnema\.localhost/',
  '(?m)^\[perm\] sidepeek Microphone Deny$',
  '(?m)^\[probe\] sidepeek media NotAllowedError$',
  '(?m)^\[probe\] gaming-overlay origin https://mnema\.localhost/',
  '(?m)^\[perm\] gaming-overlay Microphone Deny$',
  '(?m)^\[probe\] gaming-overlay media NotAllowedError$',
  '(?m)^\[probe\] gaming-settings origin https://mnema\.localhost/',
  '(?m)^\[perm\] gaming-settings Microphone Deny$',
  '(?m)^\[probe\] gaming-settings media NotAllowedError$'
)
$missing = @($required | Where-Object { $text -notmatch $_ })
if ($missing.Count -gt 0) {
  Write-Output ("WAIT " + ($missing.Count) + " lines")
  return
}
Write-Output 'OK'
