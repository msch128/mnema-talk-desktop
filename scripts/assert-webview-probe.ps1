# Reads the WebView2 probe record written by the desktop host.
# Prints OK when every required line is present and every reported origin is
# the mapped custom-protocol origin. Prints WAIT while lines are still
# missing. Throws when a line contradicts the mapping, when a microphone is
# granted without a human prompt, or when a window that must refuse is not
# refused.
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

foreach ($perm in [regex]::Matches($text, '(?m)^\[perm\] (\S+) (Microphone|Camera) (\S+)$')) {
  $label = $perm.Groups[1].Value
  $kind = $perm.Groups[2].Value
  $decision = $perm.Groups[3].Value
  $expected = if ($label -eq 'instance') { 'Default' } else { 'Deny' }
  if ($decision -ne $expected) {
    throw "WebView2 permission decision was $label $kind $decision"
  }
}
foreach ($media in [regex]::Matches($text, '(?m)^\[probe\] (\S+) media (\S+)$')) {
  $label = $media.Groups[1].Value
  $value = $media.Groups[2].Value
  if ($value -eq 'granted') {
    throw "WebView2 granted a microphone without a human prompt: $label"
  }
  if ($value -eq 'NotFoundError') { throw "WebView2 saw no capture device: $label" }
  if ($label -ne 'instance' -and $value -ne 'NotAllowedError') {
    throw "WebView2 did not refuse ${label}: $value"
  }
}

$required = @(
  '(?m)^\[probe\] browser-args synthetic$',
  '(?m)^\[probe\] profile main$',
  '(?m)^\[probe\] profile instance$',
  '(?m)^\[probe\] profile sidepeek$',
  '(?m)^\[probe\] profile gaming-overlay$',
  '(?m)^\[probe\] profile gaming-settings$',
  '(?m)^\[probe\] main origin https://mnema\.localhost/',
  '(?m)^\[perm\] main Microphone Deny$',
  '(?m)^\[probe\] main media NotAllowedError$',
  '(?m)^\[probe\] instance origin https://mnema\.localhost/',
  '(?m)^\[perm\] instance Microphone Default$',
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
