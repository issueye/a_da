# 桌面壳验收脚本（真实鼠标点击，无人工介入）
#
# 为什么需要它：壳子的职责属于"点一下、连一次才知道有没有接上线"的那类，单测覆盖不到——
# 窗口按钮走的是 webview → 宿主 IPC/命令 → 原生窗口这条链路，网关唤醒走的是
# "进程探测 → 拉起 → 读就绪行 → 界面接入"这条链路，而 preload 漏打包这种事故
# 只有"界面到底连上没有"能看见。
#
# 覆盖：
#   1. 壳子唤醒网关：拉起 ada-gateway → 读到 A_DA_GATEWAY_READY → **界面已接入网关**
#   2. 宿主 IPC 桥就位（Electron：preload 路径 + 渲染进程日志能进 log；这是"功能全哑"的根因）
#   3. 窗口控制：最小化 / 最大化 / 还原 / 关闭（点按钮）+ 标题栏双击最大化
#   4. 状态同步：最大化状态变化后按钮图标跟着切换
#   5. 退出清理：关窗后网关子进程被清掉（不留孤儿）
#
# 用法：
#   # Electron 交付形态（默认；先 `bun run electron:pack`）
#   pwsh -NoProfile -File tools/verify-desktop-shell.ps1
#   # Electron 壳的 dev 形态（源码直跑；会自己拉起 Vite 以走热重载分支）
#   pwsh -NoProfile -File tools/verify-desktop-shell.ps1 -Dev
#
# 注意：会真的动鼠标（点窗口按钮），跑的时候别抢鼠标。
param(
  [string]$Exe = '',
  [string]$WorkDir = '',
  [switch]$Dev
)
$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not $WorkDir) { $WorkDir = $RepoRoot }

# 默认目标与进程名（Electron 交付形态）
if (-not $Exe) {
  $Exe = if ($Dev) { '' } else { Join-Path $RepoRoot 'dist\electron\win-unpacked\ada.exe' }
}
$appProcessNames = @('ada', 'a_da', 'electron')
# 🔴 Electron 会认 ELECTRON_RUN_AS_NODE：一旦继承到（例如从 Electron 系终端里启动），
# 它就以"纯 Node"跑 main.cjs——require('electron') 变成路径字符串，进程当场崩掉。
if ($env:ELECTRON_RUN_AS_NODE) {
  Write-Host "注：检测到 ELECTRON_RUN_AS_NODE=$($env:ELECTRON_RUN_AS_NODE)，已为本次验收清除"
  Remove-Item Env:ELECTRON_RUN_AS_NODE -ErrorAction SilentlyContinue
}
$useVite = [bool]$Dev
if ($Exe) { $Exe = (Resolve-Path -LiteralPath $Exe).Path }
$artDir = Join-Path $RepoRoot 'tmp'
New-Item -ItemType Directory -Force -Path $artDir | Out-Null
$appHome = if ($env:A_DA_HOME) { $env:A_DA_HOME } else { Join-Path $env:USERPROFILE '.a-da' }
$logPath = Join-Path $appHome 'launcher.log'

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class ShellWin {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr i);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

  /// 按 PID + 标题找主窗口（MainWindowHandle 会挑到 16x16 的隐藏窗口，不可靠；
  /// Electron 主进程还有好几个辅助进程，更必须按标题认）。标题按前缀匹配：
  /// Electron 的窗口标题跟随 document.title，多一个后缀（如 "a_da Agent"）也要认出来。
  public static IntPtr FindMain(uint pid, string title) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p != pid || !IsWindowVisible(h)) return true;
      var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
      if (sb.ToString().StartsWith(title)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@

$StripW = 260     # 从右边缘往里截多宽来定位三个窗口按钮
$ButtonY = 19     # 标题栏高 40px，取中线附近
$failures = New-Object 'System.Collections.Generic.List[System.String]'
function Check([bool]$ok, [string]$what) {
  if ($ok) { Write-Host "  [PASS] $what" } else { Write-Host "  [FAIL] $what"; $failures.Add($what) }
}
function Rect($h) { $r = New-Object ShellWin+RECT; [void][ShellWin]::GetWindowRect($h, [ref]$r); $r }

# 右上条带里的图标簇中心。
# 🔴 判据必须与主题无关：先取条带里出现次数最多的颜色当背景，再按「与背景的差异」找图标。
# 早先用「暗像素计数」在亮色主题下能用，一切到暗色主题整条标题栏都算"暗"，定位直接失效。
function Buttons($h) {
  $r = Rect $h
  $w = $StripW; $ht = 70
  $bmp = New-Object System.Drawing.Bitmap $w, $ht
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen([int]$r.Right - $w, [int]$r.Top, 0, 0, (New-Object System.Drawing.Size $w, $ht))
  $g.Dispose()

  $hist = @{}
  for ($x = 0; $x -lt $w; $x += 2) {
    for ($y = 8; $y -lt 32; $y += 2) {
      $key = $bmp.GetPixel($x, $y).ToArgb()
      $hist[$key] = [int]$hist[$key] + 1
    }
  }
  $bgKey = ($hist.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 1).Key
  $bg = [System.Drawing.Color]::FromArgb($bgKey)

  $runs = New-Object 'System.Collections.Generic.List[System.Int32]'
  $inRun = $false; $runStart = 0
  for ($x = 0; $x -lt ($w - 12); $x++) {
    $hit = 0
    for ($y = 6; $y -lt 34; $y++) {
      $px = $bmp.GetPixel($x, $y)
      $delta = [Math]::Max([Math]::Abs($px.R - $bg.R), [Math]::Max([Math]::Abs($px.G - $bg.G), [Math]::Abs($px.B - $bg.B)))
      if ($delta -gt 45) { $hit = $hit + 1 }
    }
    if ($hit -gt 0) { if (-not $inRun) { $runStart = $x; $inRun = $true } }
    elseif ($inRun) { $runs.Add([int](($runStart + $x - 1) / 2)); $inRun = $false }
  }
  if ($inRun) { $runs.Add([int](($runStart + $w - 13) / 2)) }
  $bmp.Dispose()
  return [pscustomobject]@{ Rect = $r; Xs = $runs; Bg = $bg }
}

# 每次点击前都重新量窗口矩形：最大化/还原会改变窗口位置与尺寸，写死坐标必然点空
function ClickRel($h, [int]$relX) {
  $r = Rect $h
  [void][ShellWin]::SetCursorPos(([int]$r.Right - $StripW + $relX), ([int]$r.Top + $ButtonY))
  Start-Sleep -Milliseconds 320
  [ShellWin]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 70
  [ShellWin]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 700
}

# 双击标题栏空白（标题栏是拖拽区，双击由宿主原生切换最大化）
function DoubleClickAt([int]$x, [int]$y) {
  [void][ShellWin]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 350
  for ($i = 0; $i -lt 2; $i++) {
    [ShellWin]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
    [ShellWin]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
  }
  Start-Sleep -Milliseconds 900
}

function ClickAt([int]$x, [int]$y) {
  [void][ShellWin]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 280
  [ShellWin]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 70
  [ShellWin]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 600
}

function PressEnter {
  [ShellWin]::keybd_event(0x0D, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [ShellWin]::keybd_event(0x0D, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 600
}

function PressEscape {
  [ShellWin]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [ShellWin]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 600
}

# 在窗口里找 emerald-600（#059669）按钮的中心：进程管理里的「重新唤醒网关」。
# 🔴 不能取"所有匹配像素的外接框中心"：界面上还有 emerald 的小点（核心链路在线、运行中徽章），
# 外接框会被它们拉大，算出来的中心落在弹窗中间、点了个空。这里改成取**最密的那一格**。
function FindEmeraldCenter($h) {
  $r = Rect $h
  $w = [int]($r.Right - $r.Left); $ht = [int]($r.Bottom - $r.Top)
  $bmp = New-Object System.Drawing.Bitmap $w, $ht
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen([int]$r.Left, [int]$r.Top, 0, 0, (New-Object System.Drawing.Size $w, $ht))
  $g.Dispose()

  $cell = 10
  $cols = [int][Math]::Ceiling($w / $cell)
  $rows = [int][Math]::Ceiling($ht / $cell)
  $grid = New-Object 'int[,]' $cols, $rows
  for ($x = 0; $x -lt $w; $x += 2) {
    for ($y = 0; $y -lt $ht; $y += 2) {
      $px = $bmp.GetPixel($x, $y)
      # emerald-600 及其 hover/active 变体
      if ($px.G -ge 110 -and $px.G -le 185 -and $px.R -le 70 -and $px.B -ge 70 -and $px.B -le 145) {
        $grid[[int]($x / $cell), [int]($y / $cell)]++
      }
    }
  }
  $bmp.Dispose()

  # 3x3 邻域求和，取最密处（实心按钮内部格子会显著高于小徽章/圆点）
  $best = 0; $bestCx = -1; $bestCy = -1
  for ($cx = 1; $cx -lt ($cols - 1); $cx++) {
    for ($cy = 1; $cy -lt ($rows - 1); $cy++) {
      $sum = 0
      for ($dx = -1; $dx -le 1; $dx++) {
        for ($dy = -1; $dy -le 1; $dy++) {
          # 注意括号：PowerShell 里 `$grid[$a + $dx, $b + $dy]` 会把索引解析成数组
          $sum += $grid[($cx + $dx), ($cy + $dy)]
        }
      }
      if ($sum -gt $best) { $best = $sum; $bestCx = $cx; $bestCy = $cy }
    }
  }
  if ($best -lt 30) { return $null }
  return [pscustomobject]@{
    X = [int]$r.Left + $bestCx * $cell + [int]($cell / 2)
    Y = [int]$r.Top + $bestCy * $cell + [int]($cell / 2)
    Pixels = $best
  }
}

# 存一张窗口截图（失败取证用）
function SaveWindowShot($h, [string]$name) {
  $r = Rect $h
  $w = [int]($r.Right - $r.Left); $ht = [int]($r.Bottom - $r.Top)
  if ($w -le 0 -or $ht -le 0) { return }
  $bmp = New-Object System.Drawing.Bitmap $w, $ht
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen([int]$r.Left, [int]$r.Top, 0, 0, (New-Object System.Drawing.Size $w, $ht))
  $g.Dispose()
  $bmp.Save((Join-Path $artDir $name), [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
}

# 采样「最大化」按钮的像素指纹：图标随状态切换 → 指纹必须不同（用来验证状态同步）
function ButtonPatch($h, [int]$relX, [string]$name) {
  $r = Rect $h
  $size = 40
  $x0 = [int]$r.Right - $StripW + $relX - [int]($size / 2)
  $y0 = [int]$r.Top + $ButtonY - [int]($size / 2)
  $bmp = New-Object System.Drawing.Bitmap $size, $size
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($x0, $y0, 0, 0, (New-Object System.Drawing.Size $size, $size))
  $g.Dispose()
  if ($name) { $bmp.Save((Join-Path $artDir $name), [System.Drawing.Imaging.ImageFormat]::Png) }
  $sig = New-Object System.Text.StringBuilder
  for ($x = 0; $x -lt $size; $x += 2) {
    for ($y = 0; $y -lt $size; $y += 2) {
      $px = $bmp.GetPixel($x, $y)
      [void]$sig.Append($px.R.ToString('x2') + $px.G.ToString('x2') + $px.B.ToString('x2') + ';')
    }
  }
  $bmp.Dispose()
  return $sig.ToString()
}

# 🔴 把窗口真的顶到最前。
# 截屏（CopyFromScreen）与合成鼠标事件都作用在**屏幕坐标**上：只要别的窗口盖住了它，
# 截到的是别人的界面、点下去也点在被盖住的位置——整套断言会变成一堆莫名其妙的 FAIL。
# 已经在最前就直接返回：用 ALT 抢前台后，系统会把紧接着的第一次点击当成"激活窗口"吃掉。
function Force-Foreground($h) {
  if ([ShellWin]::GetForegroundWindow() -eq $h) { return $true }
  [void][ShellWin]::ShowWindow($h, 9)   # SW_RESTORE
  [ShellWin]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)   # ALT down
  [ShellWin]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)   # ALT up
  [void][ShellWin]::BringWindowToTop($h)
  [void][ShellWin]::SetForegroundWindow($h)
  Start-Sleep -Milliseconds 500
  $ok = ([ShellWin]::GetForegroundWindow() -eq $h)
  if ($ok) {
    $r = Rect $h
    [void][ShellWin]::SetCursorPos([int]$r.Left + 30, [int]$r.Top + $ButtonY)
    Start-Sleep -Milliseconds 250
    [ShellWin]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 50
    [ShellWin]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 400
  }
  return $ok
}

# 🔴 只清我们自己那套进程：`electron` 这个名字在用户机器上可能是别的应用（VS Code、
# 别的 Electron 桌面端），按名字无差别 Stop-Process 会误杀别人的程序。
# 判据是**可执行文件路径**：本次要跑的那个 exe（含它的 renderer/GPU 辅助进程，
# 它们共用同一个 exe）、以及 node_modules/electron 与 dist/electron 两处。
$allowedExePaths = @(
  if ($Exe) { $Exe },
  (Join-Path $RepoRoot 'node_modules\electron'),
  (Join-Path $RepoRoot 'dist\electron')
)
function Get-OurAppProcesses {
  $result = @()
  foreach ($p in (Get-Process -Name $appProcessNames -ErrorAction SilentlyContinue)) {
    $exePath = ''
    try { $exePath = $p.Path } catch { }
    if (-not $exePath) { continue }
    foreach ($allowed in $allowedExePaths) {
      if (-not $allowed) { continue }
      if ($exePath.Equals($allowed, [System.StringComparison]::OrdinalIgnoreCase) -or
          $exePath.StartsWith($allowed + [IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        $result += $p
        break
      }
    }
  }
  return $result
}
function Stop-OurAppProcesses {
  Get-OurAppProcesses | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
  Get-Process -Name 'ada-gateway', 'ada-coding' -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue
}

Stop-OurAppProcesses
Start-Sleep -Seconds 2

# Vite 开发服务器：Electron -Dev 时若它没起来就会退回静态产物
$vite = $null
if ($useVite) {
  $vite = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', 'npm --prefix desktop-ui run dev' -WorkingDirectory $RepoRoot -PassThru -WindowStyle Hidden
  $up = $false
  for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 500
    try {
      $client = New-Object System.Net.Sockets.TcpClient
      # Vite 默认只监听 localhost（本机解析优先 ::1），探 127.0.0.1 会误判为没起来
      $client.Connect('localhost', 1420); $client.Close(); $up = $true; break
    } catch { }
  }
  if (-not $up) { Write-Host 'FAIL: Vite 开发服务器（1420）没起来'; if ($vite) { taskkill /pid $vite.Id /T /F 2>&1 | Out-Null }; exit 1 }
  Write-Host 'Vite 开发服务器已就绪 (http://localhost:1420)'
}

$logBefore = (Get-Item $logPath).Length
# 记录启动前就已存在的同名进程：关窗后只要求"我们起的那些"消失，
# 别把用户环境里别的 Electron 应用算进来
$preexistingHostPids = @(Get-Process -Name $appProcessNames -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)

$env:A_DA_SILENT_FATAL = '1'   # 宿主的原生错误框静音，避免挡住无人值守流程
$proc = if ($Exe) {
  Start-Process -FilePath $Exe -WorkingDirectory $WorkDir -PassThru
} else {
  # Electron 壳的 dev 形态：npx electron .（源码 + 热重载）
  Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', 'npx electron .' -WorkingDirectory $WorkDir -PassThru -WindowStyle Hidden
}
Write-Host "已启动 $(if ($Exe) { $Exe } else { 'npx electron . (dev)' }) (宿主 pid=$($proc.Id))"

function NewLogChunk {
  $fs = [System.IO.File]::Open($logPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
  try {
    $len = $fs.Length
    if ($len -le $logBefore) { return '' }
    $fs.Seek($logBefore, [System.IO.SeekOrigin]::Begin) | Out-Null
    $take = [int]($len - $logBefore)
    $buf = New-Object byte[] $take; $fs.Read($buf, 0, $take) | Out-Null
    return [System.Text.Encoding]::UTF8.GetString($buf)
  } finally { $fs.Dispose() }
}

# 等界面与网关握手完成
$connected = $false
for ($i = 0; $i -lt 60; $i++) {   # 冷启动要现拉起 agent 实例，给足 30s
  if ((NewLogChunk) -match '客户端已接入网关') { $connected = $true; break }
  Start-Sleep -Milliseconds 500
}
$newLog = NewLogChunk

# 找主窗口（Electron：主进程 + 若干辅助进程，按标题认）
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 500
  $proc.Refresh()
  if ($proc.HasExited) { Write-Host "FAIL: 启动进程提前退出 code=$($proc.ExitCode)"; exit 1 }
  $candidates = Get-OurAppProcesses
  foreach ($c in $candidates) {
    $h = [ShellWin]::FindMain([uint32]$c.Id, 'a_da')
    if ($h -ne [IntPtr]::Zero) { break }
  }
  if ($h -ne [IntPtr]::Zero) { break }
}

# ---------------- 1. 壳子唤醒网关 ----------------
Write-Host '== 1. 网关唤醒与 IPC 桥 =='
Check ($h -ne [IntPtr]::Zero) '找到主窗口'
# Electron 宿主拉起网关时写「网关进程」
Check ($newLog -match '正在拉起网关(进程|程序)') '宿主拉起了网关进程'
Check ($newLog -match 'A_DA_GATEWAY_READY') '读到网关就绪行（A_DA_GATEWAY_READY）'
Check $connected '界面已接入网关（UI 与网关握手成功）'
Check ($null -ne (Get-Process -Name 'ada-gateway' -ErrorAction SilentlyContinue)) 'ada-gateway 进程在线'
# 这两个是"Electron 版功能全哑"的根因守卫：preload 一漏打包，界面就再也连不上网关
Check ($newLog -match 'preload 路径:') '宿主加载到了 preload（window.electronAPI 可用）'
Check ($newLog -match '\[Renderer') '渲染进程日志已转发进 launcher.log（可诊断）'
if ($h -eq [IntPtr]::Zero) { exit 1 }

$fgOk = $false
for ($i = 0; $i -lt 20; $i++) {
  if (Force-Foreground $h) { $fgOk = $true; break }
  Start-Sleep -Milliseconds 400
}
if (-not $fgOk) {
  Write-Host "FAIL: 桌面端窗口顶不到前台（当前前台 hwnd=$([ShellWin]::GetForegroundWindow())），截屏/点击都会落到别的窗口上"
  exit 1
}
Start-Sleep -Milliseconds 800

# ---------------- 2. 窗口控制 ----------------
Write-Host '== 2. 窗口控制 =='
[void](Force-Foreground $h)
# 等界面真正画完再定位：关闭按钮永远贴着右边缘（条带内约 226/260），
# 且三个按钮等距相邻——用这两条把"别的窗口恰好有 3 个图标"挡掉。
$info = $null
for ($i = 0; $i -lt 60; $i++) {
  Start-Sleep -Milliseconds 500
  $info = Buttons $h
  $xsTry = @($info.Xs)
  if ($xsTry.Count -ge 3) {
    $lastThree = $xsTry[($xsTry.Count - 3)..($xsTry.Count - 1)]
    $gap1 = $lastThree[1] - $lastThree[0]; $gap2 = $lastThree[2] - $lastThree[1]
    $distToRight = $StripW - $lastThree[2]
    if ($distToRight -ge 15 -and $distToRight -le 60 -and
        $gap1 -ge 15 -and $gap1 -le 70 -and $gap2 -ge 15 -and $gap2 -le 70) { break }
  }
}
$xs = @($info.Xs)
if ($xs.Count -lt 3) { Write-Host "FAIL: 定位不到窗口按钮（簇=$($xs -join ',')）"; exit 1 }
$minX = $xs[$xs.Count - 3]; $maxX = $xs[$xs.Count - 2]; $closeX = $xs[$xs.Count - 1]
Write-Host "  按钮位置: 最小化=$minX 最大化=$maxX 关闭=$closeX (背景 $($info.Bg.Name))"

# 2.1 最小化 / 还原
ClickRel $h $minX
$iconic = $false
for ($i = 0; $i -lt 10; $i++) {
  if ([ShellWin]::IsIconic($h)) { $iconic = $true; break }
  Start-Sleep -Milliseconds 200
}
Check $iconic '点击「最小化」→ 窗口已最小化'
[void][ShellWin]::ShowWindow($h, 9)   # SW_RESTORE
Start-Sleep -Milliseconds 1200
Check (-not [ShellWin]::IsIconic($h)) '还原回可见状态'
[void](Force-Foreground $h)

# 2.2 最大化 / 还原（按钮）+ 按钮图标随状态同步
ClickRel $h $maxX
Check ([ShellWin]::IsZoomed($h)) '点击「最大化」→ 窗口已最大化'
$sigMaxed = ButtonPatch $h $maxX 'btn_maximized.png'
ClickRel $h $maxX
Check (-not [ShellWin]::IsZoomed($h)) '再次点击 → 窗口已还原'
$sigRestored = ButtonPatch $h $maxX 'btn_restored.png'
Check ($sigMaxed -ne $sigRestored) '最大化/还原后按钮图标已切换（is_maximized 状态同步）'

# 2.3 标题栏双击最大化（前端**没有** onDoubleClick 兜底，图标只能靠宿主广播的状态事件更新）
$r = Rect $h
DoubleClickAt ([int]$r.Left + 30) ([int]$r.Top + $ButtonY)
Check ([ShellWin]::IsZoomed($h)) '双击标题栏空白 → 窗口已最大化'
$sigDblMax = ButtonPatch $h $maxX 'btn_dblclick_max.png'
Check ($sigDblMax -ne $sigRestored) '双击最大化后按钮图标切到「向下还原」（事件链路通）'
$r = Rect $h
DoubleClickAt ([int]$r.Left + 30) ([int]$r.Top + $ButtonY)
Check (-not [ShellWin]::IsZoomed($h)) '再次双击 → 窗口已还原'
$sigDblBack = ButtonPatch $h $maxX 'btn_dblclick_back.png'
Check ($sigDblBack -ne $sigDblMax) '双击还原后按钮图标切回「最大化」'

# 2.4 只重启网关（新能力：不必重启整个桌面端）
#     点标题栏中间的连接状态胶囊 → 打开进程管理 → 找 emerald 按钮 → Enter 确认
Write-Host '== 2.4 重新唤醒网关 =='
$restartReqMarker = '收到「重新唤醒网关」请求'
$restartDoneMarker = '重新唤醒网关结束'
$logBeforeRestart = NewLogChunk
$r = Rect $h
ClickAt ([int](($r.Left + $r.Right) / 2)) ([int]$r.Top + $ButtonY)
Start-Sleep -Milliseconds 1500
$btn = FindEmeraldCenter $h
Check ($null -ne $btn) '进程管理里找到「重新唤醒网关」按钮'
if ($btn) {
  ClickAt $btn.X $btn.Y
  Start-Sleep -Milliseconds 800
  PressEnter   # 确认弹窗：Enter 即确认
  $restarted = $false
  for ($i = 0; $i -lt 60; $i++) {
    if ((NewLogChunk) -match $restartReqMarker) { $restarted = $true; break }
    Start-Sleep -Milliseconds 500
  }
  Check $restarted '宿主收到了「重新唤醒网关」请求'
  $reconnected = $false
  for ($i = 0; $i -lt 60; $i++) {
    $chunk = NewLogChunk
    $fresh = $chunk.Substring([Math]::Min($logBeforeRestart.Length, $chunk.Length))
    if ($fresh -match $restartDoneMarker -and $fresh -match 'A_DA_GATEWAY_READY') { $reconnected = $true; break }
    Start-Sleep -Milliseconds 500
  }
  Check $reconnected '重启后网关重新就绪（拿到新的就绪行）'
  $gw2 = Get-Process -Name 'ada-gateway' -ErrorAction SilentlyContinue
  Check ($null -ne $gw2) '重启后的 ada-gateway 进程在线'

  # 弹窗还开着：这时窗口按钮**必须**照常可用（标题栏是窗口 chrome，不能被模态层盖住）
  ClickRel $h $maxX
  Check ([ShellWin]::IsZoomed($h)) '模态层打开时点「最大化」仍生效（标题栏未被盖住）'
  ClickRel $h $maxX
  Check (-not [ShellWin]::IsZoomed($h)) '模态层打开时点「还原」仍生效'
  PressEscape   # Esc 关模态层
  Start-Sleep -Milliseconds 500
}

# ---------------- 3. 关闭 + 退出清理 ----------------
Write-Host '== 3. 关闭与清理 =='
SaveWindowShot $h 'before_close.png'
ClickRel $h $closeX
$hostGone = $false
for ($i = 0; $i -lt 30; $i++) {
  Start-Sleep -Milliseconds 500
  $left = @(Get-OurAppProcesses | Where-Object { $preexistingHostPids -notcontains $_.Id })
  if ($left.Count -eq 0) { $hostGone = $true; break }
}
Check $hostGone '点击「关闭」→ 桌面端进程已退出'
Start-Sleep -Seconds 2
Check ($null -eq (Get-Process -Name 'ada-gateway' -ErrorAction SilentlyContinue)) '关窗后没有遗留 ada-gateway 孤儿进程'

Stop-OurAppProcesses
if ($vite) { taskkill /pid $vite.Id /T /F 2>&1 | Out-Null }

Write-Host ''
if ($failures.Count -eq 0) {
  Write-Host "RESULT: PASS — Electron 壳子（网关唤醒 + IPC 桥 + 窗口控制 + 状态同步 + 退出清理）全部通过"
  exit 0
} else {
  Write-Host "RESULT: FAIL — $($failures.Count) 项未通过:"
  $failures | ForEach-Object { Write-Host "  - $_" }
  exit 1
}
