# 实机验收①：**同一工作区只有一个 ada-coding 进程** + 空闲回收
#
# 断言：
#   1. 三种"同一目录的不同写法"（原样 / 尾巴斜杠 / 全大写）连上来 → 只拉起**一个**实例
#   2. 同一实例里开多个会话（thread.create × 3）→ 进程数仍然只有 1
#   3. 有客户端连着时**不回收**（哪怕超过空闲阈值）
#   4. 所有客户端断开后 → 实例被回收（进程消失，注册表清空）
param(
  [string]$Gateway = 'E:\codes\rust_projects\a_da\dist\ada-gateway.exe',
  [string]$Workspace = 'E:\codes\rust_projects\a_da',
  [int]$IdleSecs = 8,
  [int]$IntervalSecs = 3
)
$ErrorActionPreference = 'Stop'
$RepoRoot = 'E:\codes\rust_projects\a_da'
$failures = New-Object 'System.Collections.Generic.List[System.String]'
function Check([bool]$ok, [string]$what) {
  if ($ok) { Write-Host "  [PASS] $what" } else { Write-Host "  [FAIL] $what"; $failures.Add($what) }
}
function AgentProcCount {
  @(Get-Process -Name 'ada-coding' -ErrorAction SilentlyContinue).Count
}

# 前置清理：只清我们自己的那套
Get-Process -Name ada-gateway, ada-coding -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

$outFile = Join-Path $RepoRoot 'tmp\gw-one-proc.out'
$errFile = Join-Path $RepoRoot 'tmp\gw-one-proc.err'
Remove-Item $outFile, $errFile -ErrorAction SilentlyContinue

$env:A_DA_GATEWAY_IDLE_REAP_SECS = "$IdleSecs"
$env:A_DA_GATEWAY_REAP_INTERVAL_SECS = "$IntervalSecs"
$gw = Start-Process -FilePath $Gateway -ArgumentList @('--port', '0', '--workspace', $Workspace) `
  -WorkingDirectory $RepoRoot -RedirectStandardOutput $outFile -RedirectStandardError $errFile -PassThru -WindowStyle Hidden
Write-Host "网关已启动 (pid=$($gw.Id))，空闲回收阈值 ${IdleSecs}s / 扫描间隔 ${IntervalSecs}s"

$ready = $null
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 400
  $line = Select-String -Path $outFile -Pattern 'A_DA_GATEWAY_READY' -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($line) {
    $prefix = 'A_DA_GATEWAY_READY'
    $json = $line.Line.Substring($line.Line.IndexOf($prefix) + $prefix.Length).Trim()
    $ready = $json | ConvertFrom-Json
    break
  }
}
if (-not $ready) { Write-Host 'FAIL: 网关没起来'; Get-Content $errFile -Tail 10; exit 1 }
$port = $ready.port
Write-Host "网关就绪：port=$port"

function NewClient([string]$wsQuery) {
  $ws = New-Object System.Net.WebSockets.ClientWebSocket
  $ct = New-Object System.Threading.CancellationTokenSource
  $ws.ConnectAsync([Uri]"ws://127.0.0.1:$port/rpc?workspace=$wsQuery", $ct.Token).Wait()
  return @{ ws = $ws; ct = $ct }
}
function Rpc($client, [int]$id, [string]$method, [hashtable]$params) {
  $payload = @{ jsonrpc = '2.0'; id = $id; method = $method; params = $params } | ConvertTo-Json -Depth 8 -Compress
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
  $seg = New-Object 'System.ArraySegment[byte]' -ArgumentList @(,$bytes)
  $client.ws.SendAsync($seg, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $client.ct.Token).Wait()
  # 🔴 每次接收都要有**自己的取消时限**：`ReceiveAsync(...).Result` 在服务端没发帧时
  # 会无限阻塞（第一次测试脚本就卡在这儿），deadline 放在循环外根本轮不到。
  $deadline = (Get-Date).AddSeconds(15)
  while ((Get-Date) -lt $deadline) {
    $sb = New-Object System.Text.StringBuilder
    $gotFrame = $false
    do {
      $buf = New-Object byte[] 131072
      $segR = New-Object 'System.ArraySegment[byte]' -ArgumentList @(,$buf)
      $recvCts = New-Object System.Threading.CancellationTokenSource
      $recvCts.CancelAfter(5000)
      try {
        $res = $client.ws.ReceiveAsync($segR, $recvCts.Token).Result
      } catch {
        return $null   # 超时/断开：不装死，如实返回没拿到响应
      } finally { $recvCts.Dispose() }
      [void]$sb.Append([System.Text.Encoding]::UTF8.GetString($buf, 0, $res.Count))
      $gotFrame = $true
    } while (-not $res.EndOfMessage)
    if (-not $gotFrame) { return $null }
    $msg = $sb.ToString() | ConvertFrom-Json
    if ($msg.id -eq $id) { return $msg }
  }
  return $null
}
function CloseClient($client) {
  try { $client.ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'bye', $client.ct.Token).Wait(2000) } catch { }
}

$wsPlain = $Workspace.Replace('\', '/')
$wsSlash = "$wsPlain/"
$wsUpper = $wsPlain.ToUpper()
Write-Host "三种写法：`n  A=$wsPlain`n  B=$wsSlash`n  C=$wsUpper"

# ── 1. 三个客户端（同一目录的三种写法） ────────────────────────
$a = NewClient $wsPlain
Start-Sleep -Seconds 3
$b = NewClient ([uri]::EscapeDataString($wsSlash))
Start-Sleep -Seconds 3
$c = NewClient ([uri]::EscapeDataString($wsUpper))
Start-Sleep -Seconds 5

Write-Host '== 断言 1：同一工作区只拉起一个实例 =='
$log = Get-Content $outFile -Raw
$spawnLines = @([regex]::Matches($log, '网关拉起 agent 实例'))
$bindLines = @([regex]::Matches($log, '客户端已接入网关，绑定实例 ([^\r\n]+)'))
$boundIds = $bindLines | ForEach-Object { $_.Groups[1].Value.Trim() } | Sort-Object -Unique
Write-Host "  拉起次数=$($spawnLines.Count) 绑定次数=$($bindLines.Count) 不同实例 id=$($boundIds -join ', ')"
Check ($spawnLines.Count -eq 1) "只拉起了 1 个 agent 进程（实际 $($spawnLines.Count)）"
Check ($boundIds.Count -eq 1) "三种路径写法都绑到同一个实例（实际 $($boundIds -join ', ')）"
Check ((AgentProcCount) -eq 1) "ada-coding 进程数 = 1（实际 $(AgentProcCount)）"

# ── 2. 同一实例内开多个会话 ────────────────────────────────────
Write-Host '== 断言 2：同一工作区的多个会话共用同一进程 =='
[void](Rpc $c 1 'session.initialize' @{ protocolVersion = '1.0'; client = @{ name = 'e2e'; version = '1'; platform = 'e2e' } })

# 会话落盘目录：<home>/sessions/<workspace-hash>/thread_*.jsonl —— 用它数"新开了几个会话"
$sessionsRoot = Join-Path $env:USERPROFILE '.a-da\sessions'
$beforeFiles = @(Get-ChildItem $sessionsRoot -Recurse -File -Filter 'thread_*.jsonl' -ErrorAction SilentlyContinue).Count
$tids = @()
foreach ($i in 2..4) {
  $r = Rpc $c $i 'thread.create' @{ workspace = $wsPlain; mode = 'code' }
  if ($r -and $r.result.threadId) { $tids += $r.result.threadId }
}
Start-Sleep -Seconds 3
$afterFiles = @(Get-ChildItem $sessionsRoot -Recurse -File -Filter 'thread_*.jsonl' -ErrorAction SilentlyContinue).Count
Write-Host "  会话文件：$beforeFiles → $afterFiles；返回的 threadId：$($tids -join ', ')"
Check (($afterFiles - $beforeFiles) -ge 3 -or $tids.Count -eq 3) "同一进程内建出了 3 个会话（文件 +$($afterFiles - $beforeFiles) / 回执 $($tids.Count)）"
Check ((AgentProcCount) -eq 1) "多会话没有多起进程（ada-coding 进程数 = $(AgentProcCount)）"

# ── 3. 有连接时不许回收 ────────────────────────────────────────
Write-Host "== 断言 3：有客户端连着时不回收（等 $($IdleSecs + $IntervalSecs + 6) 秒） =="
Start-Sleep -Seconds ($IdleSecs + $IntervalSecs + 6)
$log = Get-Content $outFile -Raw
Check ((AgentProcCount) -eq 1) "挂着的客户端没被回收（进程数 = $(AgentProcCount)）"
Check (-not ($log -match '回收空闲 agent 实例')) '日志里没有"回收空闲实例"（连接活着就不该回收）'

# ── 4. 全部断开 → 空闲即回收 ───────────────────────────────────
Write-Host '== 断言 4：全部断开后按空闲回收 =='
CloseClient $a; CloseClient $b; CloseClient $c
$reaped = $false
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 500
  if ((AgentProcCount) -eq 0) { $reaped = $true; break }
}
$log = Get-Content $outFile -Raw
Check $reaped "断开后实例被回收（进程数 = $(AgentProcCount)）"
Check ($log -match '回收空闲 agent 实例') '日志如实记录了回收动作'

# ── 收尾 ───────────────────────────────────────────────────────
if (-not $gw.HasExited) { Stop-Process -Id $gw.Id -Force -ErrorAction SilentlyContinue }
Get-Process -Name ada-gateway, ada-coding -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item Env:A_DA_GATEWAY_IDLE_REAP_SECS, Env:A_DA_GATEWAY_REAP_INTERVAL_SECS -ErrorAction SilentlyContinue

Write-Host ''
if ($failures.Count -eq 0) { Write-Host 'RESULT: PASS — 同工作区单进程 + 空闲回收 全部通过'; exit 0 }
Write-Host "RESULT: FAIL — $($failures.Count) 项未通过:"; $failures | ForEach-Object { Write-Host "  - $_" }
exit 1
