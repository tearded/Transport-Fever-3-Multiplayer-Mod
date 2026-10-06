# Runs one line of Lua in a game's developer console and prints what the
# game wrote for it (docs/GAME_TESTING.md, "The console").
#
#   console.ps1 -GamePid <pid> -Lua 'print("@@n", #api.engine.system.lineSystem.getLines())' [-Open]
#   console.ps1 -GamePid <pid> -File query.lua [-Open] [-Wait 4]
#
# -Open first opens the console: once, when it is closed. The line runs in
# the GUI's Lua state. Every game on the PC writes to one stdout.txt, so
# print a marker ("@@" by default, -Tag to change it) and only lines with
# it, or with a Lua error, are shown. -File keeps quotes intact, which a
# -Lua argument passed through `powershell -File` does not.
param([int]$GamePid, [string]$Lua, [string]$File, [switch]$Open, [double]$Wait = 2, [string]$Tag = "@@")
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\env.ps1"
if ($File) { $Lua = (Get-Content $File -Raw) }
# One line: the console runs what is typed when Enter is pressed.
$Lua = (($Lua -split "`r?`n") | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith("--") }) -join " "
if (-not $Lua) { throw "no Lua to run" }
if (-not (Test-Path $GameStdout)) { throw "no game log at $GameStdout" }
$before = (Get-Item $GameStdout).Length
if ($Open) {
  & "$PSScriptRoot\gamewin.ps1" console $Lua -GamePid $GamePid -Open | Out-Null
} else {
  & "$PSScriptRoot\gamewin.ps1" console $Lua -GamePid $GamePid | Out-Null
}
Start-Sleep -Milliseconds ([int]($Wait * 1000))
# The game keeps the file open: read it shared, from where it was.
$fs = [System.IO.File]::Open($GameStdout, 'Open', 'Read', 'ReadWrite')
try {
  [void]$fs.Seek($before, 'Begin')
  $text = (New-Object System.IO.StreamReader($fs)).ReadToEnd()
} finally { $fs.Close() }
$lines = $text -split "`n" | Where-Object { $_.Trim() }
if ($Tag) { $lines = $lines | Where-Object { $_.Contains($Tag) -or $_ -match "rror|attempt to" } }
$lines | Select-Object -Last 60
