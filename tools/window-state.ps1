param(
  [Parameter(Mandatory = $true)][int]$ProcessId,
  [ValidateSet('state', 'restore', 'minimize', 'maximize', 'focus', 'click')][string]$Action = 'state',
  [string]$TitlePrefix = 'a_da',
  # 仅 click：相对窗口左上角的物理像素坐标（调用方按 devicePixelRatio 换算）。
  [int]$RelX = 0,
  [int]$RelY = 0
)

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class HeadlessWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr i);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

  public static IntPtr Find(uint pid, string prefix) {
    IntPtr found = IntPtr.Zero;
    IntPtr fallback = IntPtr.Zero;
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p != pid || !IsWindowVisible(h)) return true;
      var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
      var title = sb.ToString();
      if (title.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) { found = h; return false; }
      // SSR 页面标题是「新会话 · a_da」，不再以 a_da 开头：留一个同进程可见窗口兜底。
      if (fallback == IntPtr.Zero && title.Length > 0) fallback = h;
      return true;
    }, IntPtr.Zero);
    return found != IntPtr.Zero ? found : fallback;
  }

  public static string Title(IntPtr h) {
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    return sb.ToString();
  }
}
"@

$hwnd = [HeadlessWin]::Find([uint32]$ProcessId, $TitlePrefix)
if ($hwnd -eq [IntPtr]::Zero) {
  @{ found = $false } | ConvertTo-Json -Compress
  exit 2
}

switch ($Action) {
  'minimize' { [void][HeadlessWin]::ShowWindow($hwnd, 6); Start-Sleep -Milliseconds 500 }
  'maximize' { [void][HeadlessWin]::ShowWindow($hwnd, 3); Start-Sleep -Milliseconds 500 }
  'restore'  { [void][HeadlessWin]::ShowWindow($hwnd, 9); Start-Sleep -Milliseconds 600 }
  'focus' {
    [void][HeadlessWin]::ShowWindow($hwnd, 9)
    [HeadlessWin]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
    [HeadlessWin]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
    [void][HeadlessWin]::SetForegroundWindow($hwnd)
    Start-Sleep -Milliseconds 600
  }
  'click' {
    $rect = New-Object HeadlessWin+RECT
    [void][HeadlessWin]::GetWindowRect($hwnd, [ref]$rect)
    [void][HeadlessWin]::ShowWindow($hwnd, 9)
    # 与官方验收脚本同一套抢前台手法：ALT 轻敲后再 SetForegroundWindow。
    [HeadlessWin]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
    [HeadlessWin]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
    [void][HeadlessWin]::SetForegroundWindow($hwnd)
    Start-Sleep -Milliseconds 400
    [void][HeadlessWin]::SetCursorPos($rect.Left + $RelX, $rect.Top + $RelY)
    Start-Sleep -Milliseconds 150
    [HeadlessWin]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    [HeadlessWin]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 350
  }
}

@{
  found  = $true
  hwnd   = $hwnd.ToInt64()
  title  = [HeadlessWin]::Title($hwnd)
  iconic = [HeadlessWin]::IsIconic($hwnd)
  zoomed = [HeadlessWin]::IsZoomed($hwnd)
} | ConvertTo-Json -Compress
