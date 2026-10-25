param(
  [Parameter(Mandatory = $true)][int]$ProcessId,
  [ValidateSet('state', 'restore', 'minimize', 'maximize')][string]$Action = 'state',
  [string]$TitlePrefix = 'a_da'
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
}

@{
  found  = $true
  hwnd   = $hwnd.ToInt64()
  title  = [HeadlessWin]::Title($hwnd)
  iconic = [HeadlessWin]::IsIconic($hwnd)
  zoomed = [HeadlessWin]::IsZoomed($hwnd)
} | ConvertTo-Json -Compress
