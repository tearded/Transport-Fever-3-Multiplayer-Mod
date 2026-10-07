# Quits games as a player does: the window's close button opens the pause
# menu, then Quit, then Return to Desktop. A game still running a minute
# later is left running and reported as a failure. Only named games are
# touched; this helper never force-kills a game.
#   quit.ps1 -GamePids 1234,5678
# The two buttons are found where they are on the game's window as it is
# now: the pause menu scales with the window, so they sit at the same
# fraction of its capture whatever its size (a capture of the window before
# the clicks, <work>\quit-<pid>.png, says what was on it). -QuitAt /
# -DesktopAt give them in capture coordinates instead (docs/GAME_TESTING.md).
param([string[]]$GamePids, [string]$QuitAt = "", [string]$DesktopAt = "")
. "$PSScriptRoot\env.ps1"
# "1234,5678" from powershell -File, or an array from a script.
$GamePids = @($GamePids | ForEach-Object { $_ -split "," } | Where-Object { $_ } | ForEach-Object { [int]$_ })
$g = "$PSScriptRoot\gamewin.ps1"
# Where the buttons are, as fractions of the capture: measured on captures
# of 1291x748 and 1280x678 (the Quit button spans 0.10-0.34 across and
# 0.64-0.72 down, Return to Desktop 0.52-0.69 across).
$quitFraction = @(0.19, 0.68); $desktopFraction = @(0.605, 0.68)
foreach ($gp in $GamePids) {
  $p = Get-Process -Id $gp -ErrorAction SilentlyContinue
  if (-not $p -or $p.ProcessName -ne "TransportFever3") { continue }
  # A hung window takes no clicks, and capturing it waits for it.
  if (-not $p.Responding) { "game $gp is not responding: nothing sent to it"; continue }
  $q = $QuitAt -split ","; $d = $DesktopAt -split ","
  if (-not $QuitAt -or -not $DesktopAt) {
    # Measured as it will be clicked: restored and in front first (a click
    # restores a minimized or maximized window to another size).
    try { & $g front -GamePid $gp | Out-Null } catch {
      "game ${gp}: it did not come to the front ($_); nothing clicked"
      continue
    }
    Start-Sleep -Seconds 1
    $said = (& $g shot "quit-$gp" -GamePid $gp | Out-String)
    if ($said -match "window (\d+)x(\d+)") {
      $cw = [int]$Matches[1] / 2; $ch = [int]$Matches[2] / 2
      if (-not $QuitAt) { $q = @([int]($cw * $quitFraction[0]), [int]($ch * $quitFraction[1])) }
      if (-not $DesktopAt) { $d = @([int]($cw * $desktopFraction[0]), [int]($ch * $desktopFraction[1])) }
    } else {
      # The window's size did not read: no guessed clicks into a game whose
      # buttons may be elsewhere. -QuitAt and -DesktopAt name them instead.
      "game ${gp}: its window's size did not read; nothing clicked (give -QuitAt and -DesktopAt)"
      continue
    }
  }
  [void]$p.CloseMainWindow()
  Start-Sleep -Seconds 2
  try { & $g click $q[0] $q[1] -GamePid $gp | Out-Null } catch {}
  Start-Sleep -Seconds 2
  try { & $g click $d[0] $d[1] -GamePid $gp | Out-Null } catch {}
  Start-Sleep -Seconds 2
}
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline -and @($GamePids | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }).Count -gt 0) { Start-Sleep -Seconds 2 }
foreach ($gp in $GamePids) {
  $p = Get-Process -Id $gp -ErrorAction SilentlyContinue
  if ($p -and $p.ProcessName -eq "TransportFever3") {
    Write-Error "Game $gp did not quit; left running for diagnosis"
    $quitFailed = $true
  } else {
    "quit $gp"
  }
}

if ($quitFailed) { exit 1 }
