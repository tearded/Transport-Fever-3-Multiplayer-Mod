# Screenshots, clicks and keys for one Transport Fever 3 window, and nothing
# else on the desktop (docs/GAME_TESTING.md). Captures copy the game
# window's own contents (PrintWindow), never the screen. Coordinates are
# those of a capture: half the window's size, frame and title bar included.
#
#   gamewin.ps1 shot <name>              capture to <work>\<name>.png (half size)
#   gamewin.ps1 click <x> <y>            left click
#   gamewin.ps1 dblclick <x> <y>         double click (for example, add a vehicle)
#   gamewin.ps1 rclick <x> <y>           right click
#   gamewin.ps1 move <x> <y>             move the cursor (a tooltip, a tool's preview)
#   gamewin.ps1 drag <x1,y1> <x2,y2>     press, move in steps, release (a road)
#   gamewin.ps1 scroll <x> <y> -Clicks n mouse wheel; n > 0 zooms in
#   gamewin.ps1 key <keys>               SendKeys syntax ({ESC}); not for text fields
#   gamewin.ps1 vk <vk> <scan>           one key as a keyboard sends it (hex; vk 0D 1C is Enter)
#   gamewin.ps1 hold <vk> <scan> -Clicks <ms>  a key held down (hold 44 20 -Clicks 600 pans right)
#   gamewin.ps1 text <string>            types into the focused text field
#   gamewin.ps1 console <lua> [-Open]    types one line into the developer console and runs it
#
# -GamePid <pid> picks the game when several run (the rig prints each one's).
# Every command but shot first brings the game to the front, and refuses
# (throws, sends nothing) when it cannot.
param([string]$cmd, [string]$a, [string]$b, [int]$GamePid = 0, [int]$Clicks = 0, [switch]$Open)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\env.ps1"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class TpfWin {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT {
    public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  // INPUT's union starts after the 4-byte type, padded to 8 on 64-bit; its
  // largest member (MOUSEINPUT) makes it 40 bytes.
  [StructLayout(LayoutKind.Explicit, Size = 40)] public struct INPUT {
    [FieldOffset(0)] public uint type; [FieldOffset(8)] public KEYBDINPUT ki; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern short VkKeyScan(char ch);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern void SwitchToThisWindow(IntPtr h, bool altTab);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
  [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);

  // A key by its scan code, which no keyboard layout remaps.
  public static void Scan(ushort code, bool up) {
    INPUT i = new INPUT(); i.type = 1; i.ki.wScan = code; i.ki.dwFlags = 8u | (up ? 2u : 0u);
    SendInput(1, new INPUT[] { i }, Marshal.SizeOf(typeof(INPUT)));
  }
  public static void Press(ushort code) { Scan(code, false); System.Threading.Thread.Sleep(40); Scan(code, true); System.Threading.Thread.Sleep(60); }
  // Text as characters, not keys: quotes and brackets arrive as written
  // whatever the layout.
  public static void Unicode(string text, IntPtr window) {
    foreach (char c in text) {
      if (GetForegroundWindow() != window) throw new InvalidOperationException("Game lost focus while typing; Enter was not sent. Clear the partial console input before retrying.");
      INPUT down = new INPUT(); down.type = 1; down.ki.wScan = c; down.ki.dwFlags = 4;
      INPUT up = down; up.ki.dwFlags = 4 | 2;
      if (SendInput(2, new INPUT[] { down, up }, Marshal.SizeOf(typeof(INPUT))) != 2)
        throw new InvalidOperationException("Windows refused console input; Enter was not sent.");
      System.Threading.Thread.Sleep(4);
    }
  }
}
"@
# Physical pixels for the window's rectangle, its capture and the cursor
# alike (per-monitor aware v2), so a click lands where the capture shows.
[void][TpfWin]::SetThreadDpiAwarenessContext([IntPtr](-4))
$p = Get-Process TransportFever3 -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 -and ($GamePid -eq 0 -or $_.Id -eq $GamePid) } | Select-Object -First 1
if (-not $p) { throw "no game window" }
$h = $p.MainWindowHandle
$r = New-Object TpfWin+RECT
[void][TpfWin]::GetWindowRect($h, [ref]$r)
$w = $r.R - $r.L; $hgt = $r.B - $r.T

function Front {
  # Windows lets a process take the foreground after a key event: tap Alt.
  [TpfWin]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
  [TpfWin]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
  [void][TpfWin]::ShowWindow($h, 9)
  [void][TpfWin]::SetForegroundWindow($h)
  Start-Sleep -Milliseconds 500
  if ([TpfWin]::GetForegroundWindow() -ne $h) {
    # Windows keeps the foreground from a process without the last input:
    # share the foreground window's input state while asking.
    $fg = [TpfWin]::GetForegroundWindow()
    $theirs = [TpfWin]::GetWindowThreadProcessId($fg, [IntPtr]::Zero)
    $mine = [TpfWin]::GetCurrentThreadId()
    [void][TpfWin]::AttachThreadInput($mine, $theirs, $true)
    [void][TpfWin]::BringWindowToTop($h)
    [void][TpfWin]::SetForegroundWindow($h)
    [void][TpfWin]::AttachThreadInput($mine, $theirs, $false)
    Start-Sleep -Milliseconds 500
  }
  if ([TpfWin]::GetForegroundWindow() -ne $h) {
    # A remote desktop's host window keeps the foreground even so; switch
    # to the game as Alt+Tab does.
    [TpfWin]::SwitchToThisWindow($h, $true)
    Start-Sleep -Milliseconds 700
  }
  if ([TpfWin]::GetForegroundWindow() -ne $h) {
    # Last: click the game's own title bar, as a player would, but only
    # where the screen shows this game's window there.
    $pt = New-Object TpfWin+POINT
    $pt.X = [int](($r.L + $r.R) / 2); $pt.Y = $r.T + 12
    if ([TpfWin]::GetAncestor([TpfWin]::WindowFromPoint($pt), 2) -eq $h) {
      [void][TpfWin]::SetCursorPos($pt.X, $pt.Y)
      Start-Sleep -Milliseconds 150
      [TpfWin]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 60
      [TpfWin]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 600
    }
  }
  if ([TpfWin]::GetForegroundWindow() -ne $h) { throw "the game did not come to the front; nothing sent" }
}

# A capture's coordinates on the screen.
function ScreenX([string]$x) { $r.L + [int]$x * 2 }
function ScreenY([string]$y) { $r.T + [int]$y * 2 }

switch ($cmd) {
  "shot" {
    $bmp = New-Object System.Drawing.Bitmap $w, $hgt
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    $ok = [TpfWin]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc)
    $small = New-Object System.Drawing.Bitmap $bmp, ([int]($w / 2)), ([int]($hgt / 2))
    $file = "$Work\$a.png"
    $small.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
    "PrintWindow=$ok; window ${w}x${hgt}; saved $file at half size"
  }
  "click" {
    Front
    [void][TpfWin]::SetCursorPos((ScreenX $a), (ScreenY $b))
    Start-Sleep -Milliseconds 200
    [TpfWin]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 80
    [TpfWin]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
    "clicked $a,$b"
  }
  "dblclick" {
    Front
    [void][TpfWin]::SetCursorPos((ScreenX $a), (ScreenY $b))
    Start-Sleep -Milliseconds 200
    for ($i = 0; $i -lt 2; $i++) {
      [TpfWin]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 60
      [TpfWin]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 60
    }
    "double-clicked $a,$b"
  }
  "rclick" {
    Front
    [void][TpfWin]::SetCursorPos((ScreenX $a), (ScreenY $b))
    Start-Sleep -Milliseconds 200
    [TpfWin]::mouse_event(0x8, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 80
    [TpfWin]::mouse_event(0x10, 0, 0, 0, [UIntPtr]::Zero)
    "right-clicked $a,$b"
  }
  "move" {
    Front
    $x = ScreenX $a; $y = ScreenY $b
    [void][TpfWin]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 150
    # A second, one-pixel move: the game reacts to motion, not position.
    [void][TpfWin]::SetCursorPos($x + 1, $y)
    "moved to $a,$b"
  }
  "drag" {
    Front
    $p1 = $a -split ","; $p2 = $b -split ","
    $x1 = ScreenX $p1[0]; $y1 = ScreenY $p1[1]; $x2 = ScreenX $p2[0]; $y2 = ScreenY $p2[1]
    [void][TpfWin]::SetCursorPos($x1, $y1)
    Start-Sleep -Milliseconds 300
    [TpfWin]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero)
    for ($i = 1; $i -le 20; $i++) {
      Start-Sleep -Milliseconds 60
      [void][TpfWin]::SetCursorPos([int]($x1 + ($x2 - $x1) * $i / 20), [int]($y1 + ($y2 - $y1) * $i / 20))
    }
    Start-Sleep -Milliseconds 300
    [TpfWin]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero)
    "dragged $a to $b"
  }
  "scroll" {
    Front
    [void][TpfWin]::SetCursorPos((ScreenX $a), (ScreenY $b))
    Start-Sleep -Milliseconds 200
    $delta = if ($Clicks -ge 0) { 120 } else { [uint32]4294967176 }
    for ($i = 0; $i -lt [Math]::Abs($Clicks); $i++) {
      [TpfWin]::mouse_event(0x800, 0, 0, $delta, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 120
    }
    "scrolled $Clicks at $a,$b"
  }
  "key" {
    Front
    [System.Windows.Forms.SendKeys]::SendWait($a)
    "sent $a"
  }
  "vk" {
    Front
    $vk = [Convert]::ToByte($a, 16); $sc = [Convert]::ToByte($b, 16)
    [TpfWin]::keybd_event($vk, $sc, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
    [TpfWin]::keybd_event($vk, $sc, 2, [UIntPtr]::Zero)
    "pressed vk $a"
  }
  "hold" {
    Front
    $vk = [Convert]::ToByte($a, 16); $sc = [Convert]::ToByte($b, 16)
    [TpfWin]::keybd_event($vk, $sc, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds ([Math]::Max(50, $Clicks))
    [TpfWin]::keybd_event($vk, $sc, 2, [UIntPtr]::Zero)
    "held vk $a for $Clicks ms"
  }
  "text" {
    # Each character pressed as a keyboard does (VkKeyScan, with Shift
    # where the character needs it): what the game's text fields take.
    Front
    foreach ($ch in $a.ToCharArray()) {
      $scan = [TpfWin]::VkKeyScan($ch)
      $vk = [byte]($scan -band 0xff); $shift = (($scan -shr 8) -band 1) -eq 1
      if ($shift) { [TpfWin]::keybd_event(0x10, 0x2A, 0, [UIntPtr]::Zero) }
      [TpfWin]::keybd_event($vk, 0, 0, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 40
      [TpfWin]::keybd_event($vk, 0, 2, [UIntPtr]::Zero)
      if ($shift) { [TpfWin]::keybd_event(0x10, 0x2A, 2, [UIntPtr]::Zero) }
      Start-Sleep -Milliseconds 40
    }
    "typed $($a.Length) characters"
  }
  "console" {
    # The developer console, as tf2mod's tools/send_game_console.ps1 drives
    # TPF2's (whose method this follows): the key below Escape (scan 0x29)
    # opens it; the key can leave its own character in the input, so a few
    # Backspaces clear it; the line goes in as characters; Enter is held for
    # longer than one frame, or the console does not see it.
    Front
    if ($Open) {
      [TpfWin]::Press(0x29)
      Start-Sleep -Milliseconds 600
    }
    for ($i = 0; $i -lt 4; $i++) { [TpfWin]::Press(0x0E) }
    [TpfWin]::Unicode($a, $h)
    if ([TpfWin]::GetForegroundWindow() -ne $h) { throw 'Game lost focus; Enter was not sent' }
    [TpfWin]::Scan(0x1C, $false)
    Start-Sleep -Milliseconds 650
    [TpfWin]::Scan(0x1C, $true)
    Start-Sleep -Milliseconds 300
    "ran $($a.Length) characters in the console"
  }
  default { throw "unknown command '$cmd'; see the top of this file" }
}
