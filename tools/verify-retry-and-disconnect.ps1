# 协议级实机验收：设置项「请求重试次数」→ 适配器重试 → 重试耗尽**断开对话**
#
# 不点界面（本模型看不了截图），而是直接驱动**真产品二进制** `ada-coding.exe`：
#   1. 把激活供应商指向必然连不上的端点（http://127.0.0.1:9/v1），retryCount=N
#   2. 起 ada-coding（打印 A_DA_HOST_READY 拿端口/token），用 WebSocket 走真 JSON-RPC：
#        session.initialize → thread.create → thread.send("hello") → thread.send("second")
#   3. 断言：
#        · agent 日志出现「第 N/N 次重试」且**正好 N 行**（= 设置里的 N 真的生效）
#        · 会话里出现「已重试 N 次仍失败」（错误真的带上了次数）
#        · 第二条消息**仍在队列里**（重试耗尽即断开本轮对话，不再拿注定失败的请求去撞队列）
#        · 会话不在运行中（runningThreadIds 不含它）
#   4. 再用真协议调 `config.setRetryCount` → 断言 config.json 里落了盘
param(
  [string]$Exe = 'E:\codes\rust_projects\a_da\dist\ada-coding.exe',
  [int]$Retries = 2
)
$ErrorActionPreference = 'Stop'
$RepoRoot = 'E:\codes\rust_projects\a_da'
$appHome = 'C:\Users\User\.a-da'
$cfg = Join-Path $appHome 'config.json'
$backup = Join-Path $RepoRoot 'tmp\config.before-retry-e2e.json'
$hostLog = Join-Path $appHome 'host.log'
$failures = New-Object 'System.Collections.Generic.List[System.String]'
function Check([bool]$ok, [string]$what) {
  if ($ok) { Write-Host "  [PASS] $what" } else { Write-Host "  [FAIL] $what"; $failures.Add($what) }
}

Get-Process -Name ada-coding, ada-gateway -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

# ── 1. 备份配置，写成"死端点 + 指定重试次数" ───────────────────
Copy-Item $cfg $backup -Force
$dead = [ordered]@{
  activeProviderId = 'e2e-dead-endpoint'
  apiKey = 'sk-e2e-dead'
  appearance = 'light'
  baseUrl = 'http://127.0.0.1:9/v1'
  contextWindow = 128000
  maxOutputTokens = 4096
  model = 'e2e-dead-model'
  protocol = 'openai_chat'
  providers = @(
    [ordered]@{
      apiKey = 'sk-e2e-dead'
      baseUrl = 'http://127.0.0.1:9/v1'
      id = 'e2e-dead-endpoint'
      models = @([ordered]@{ contextWindow = 128000; id = 'e2e-dead-model'; maxOutputTokens = 4096; name = 'e2e-dead-model'; supportsImages = $false })
      name = 'e2e-dead'
      protocol = 'openai_chat'
    }
  )
  retryCount = $Retries
  supportsImages = $false
}
$dead | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $cfg -Encoding utf8
Write-Host "验收配置：baseUrl=http://127.0.0.1:9/v1  retryCount=$Retries"

$hostBefore = if (Test-Path $hostLog) { (Get-Item $hostLog).Length } else { 0 }

# ── 2. 起产品进程，读就绪行 ─────────────────────────────────────
$outFile = Join-Path $RepoRoot 'tmp\ada-coding-e2e.out'
$errFile = Join-Path $RepoRoot 'tmp\ada-coding-e2e.err'
$proc = Start-Process -FilePath $Exe -ArgumentList @('--port', '0', '--workspace', $RepoRoot) `
  -WorkingDirectory $RepoRoot -RedirectStandardOutput $outFile -RedirectStandardError $errFile -PassThru -WindowStyle Hidden
Write-Host "已启动 ada-coding (pid=$($proc.Id))"

$ready = $null
for ($i = 0; $i -lt 40; $i++) {
  Start-Sleep -Milliseconds 500
  $line = Select-String -Path $outFile -Pattern 'A_DA_HOST_READY' -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($line) {
    $json = $line.Line.Substring($line.Line.IndexOf('A_DA_HOST_READY') + 'A_DA_HOST_READY'.Length).Trim()
    $ready = $json | ConvertFrom-Json
    break
  }
}
if (-not $ready) {
  Write-Host 'FAIL: 没拿到 A_DA_HOST_READY'; Get-Content $outFile -Tail 5; Get-Content $errFile -Tail 5
  Copy-Item $backup $cfg -Force; exit 1
}
$port = $ready.port; $token = $ready.token
Write-Host "就绪：port=$port token=$($token.Substring(0,[Math]::Min(8,$token.Length)))..."

# ── 3. WebSocket JSON-RPC 客户端 ───────────────────────────────
$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ct = New-Object System.Threading.CancellationTokenSource
$uri = [Uri]"ws://127.0.0.1:$port/rpc?token=$token"
$ws.ConnectAsync($uri, $ct.Token).Wait()
Write-Host "已连上 $($uri.ToString().Replace($token,'***'))"

$script:rpcId = 0
function Rpc([string]$method, [hashtable]$params) {
  $script:rpcId++
  $id = $script:rpcId
  $payload = @{ jsonrpc = '2.0'; id = $id; method = $method; params = $params } | ConvertTo-Json -Depth 10 -Compress
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
  $seg = New-Object 'System.ArraySegment[byte]' -ArgumentList @(,$bytes)
  $ws.SendAsync($seg, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct.Token).Wait()
  # 收帧直到拿到该 id 的响应（事件帧先来也照收）
  $deadline = (Get-Date).AddSeconds(30)
  while ((Get-Date) -lt $deadline) {
    $sb = New-Object System.Text.StringBuilder
    do {
      $buf = New-Object byte[] 131072
      $segR = New-Object 'System.ArraySegment[byte]' -ArgumentList @(,$buf)
      $res = $ws.ReceiveAsync($segR, $ct.Token).Result
      [void]$sb.Append([System.Text.Encoding]::UTF8.GetString($buf, 0, $res.Count))
    } while (-not $res.EndOfMessage)
    $msg = $sb.ToString() | ConvertFrom-Json
    if ($msg.id -eq $id) {
      if ($msg.error) { return @{ error = $msg.error } }
      return @{ result = $msg.result }
    }
  }
  return @{ error = @{ message = 'timeout' } }
}

$init = Rpc 'session.initialize' @{ protocolVersion = '1.0'; client = @{ name = 'e2e'; version = '0.0.1'; platform = 'e2e' }; token = $token }
Check ([bool]$init.result.product) 'session.initialize 成功'

$created = Rpc 'thread.create' @{ workspace = $RepoRoot; mode = 'code' }
$tid = $created.result.threadId
Check ([bool]$tid) "thread.create 成功（$tid）"

# 第一条：触发模型请求（必然失败）；第二条：进队列（用来验证"断开对话"不吃队列）
[void](Rpc 'thread.send' @{ threadId = $tid; text = 'hello'; images = @() })
Start-Sleep -Milliseconds 400
[void](Rpc 'thread.send' @{ threadId = $tid; text = 'second-queued-message'; images = @() })
Write-Host '已发送两条消息（第 2 条应进队列），等待重试耗尽...'
# 轮询快照：一旦出现「请求异常」立即取证（不必干等固定时长）
for ($i = 0; $i -lt 30; $i++) {
  Start-Sleep -Seconds 1
  $probe = (Rpc 'session.snapshot' @{}).result
  $pt = $probe.threads | Where-Object { $_.id -eq $tid } | Select-Object -First 1
  $ptext = ($pt.items | ForEach-Object { if ($_.text) { $_.text } }) -join "`n"
  if ($ptext -match '请求异常' -or $ptext -match '已重试') { Write-Host "  第 $i 秒已看到错误文本"; break }
}

# ── 4. 断言 ─────────────────────────────────────────────────────
Write-Host '== 断言 =='
$snap = (Rpc 'session.snapshot' @{}).result
$thread = $snap.threads | Where-Object { $_.id -eq $tid } | Select-Object -First 1
$allText = ($thread.items | ForEach-Object { if ($_.text) { $_.text } }) -join "`n"
$queueText = ($snap.queue | ForEach-Object { $_.text }) -join ','
$running = $snap.runningThreadIds -contains $tid
Write-Host '--- 调试：会话 items（原始 JSON，截断 700） ---'
$itemsJson = ($thread.items | ConvertTo-Json -Depth 6 -Compress)
Write-Host ('   ' + $itemsJson.Substring(0, [Math]::Min(700, $itemsJson.Length)))
Write-Host '--- 调试：快照 log 最近 8 条 ---'
@($snap.log) | Select-Object -Last 8 | ForEach-Object { '   [' + $_.level + '] ' + ('' + $_.message).Substring(0, [Math]::Min(180, ('' + $_.message).Length)) }
Write-Host "--- 调试：snapshot.config.retryCount=$($snap.config.retryCount) running=$running queue=$($snap.queue.Count) ---"

$hostDelta = ''
if (Test-Path $hostLog) {
  $fs = [System.IO.File]::Open($hostLog, 'Open', 'Read', 'ReadWrite')
  try {
    $len = $fs.Length
    if ($len -gt $hostBefore) {
      $fs.Seek($hostBefore, 'Begin') | Out-Null
      $take = [int]($len - $hostBefore)
      $buf = New-Object byte[] $take; $fs.Read($buf, 0, $take) | Out-Null
      $hostDelta = [System.Text.Encoding]::UTF8.GetString($buf)
    }
  } finally { $fs.Dispose() }
}
$retryLines = @(($hostDelta -split "`r?`n") | Where-Object { $_ -match '次重试' })
Write-Host '--- agent 日志中的重试行 ---'
$retryLines | ForEach-Object { '   ' + $_.Substring(0, [Math]::Min(190, $_.Length)) }

$expectedRetry = "第 $Retries/$Retries 次重试"
Check (($retryLines -join "`n") -match [regex]::Escape($expectedRetry)) "设置里的 $Retries 次真的生效（日志出现「$expectedRetry」）"
Check ($retryLines.Count -eq $Retries) "重试次数正好 $Retries 次（实际 $($retryLines.Count)）"
Check ($allText -match "已重试 $Retries 次仍失败") '错误信息带上了「已重试 N 次仍失败」'
Check ($allText -match '断开') '会话里明确写了「已断开」'
# 报错必须落在 **Notice 卡片**上，而不是助手正文里——落进正文会劈开一段连贯回答，
# 还会被当成"模型说过的话"回传给模型（详见 AGENTS.md 那条）。
$noticeItems = @($thread.items | Where-Object { $_.kind -eq 'notice' })
Check ($noticeItems.Count -ge 1) "报错落在 notice 卡片上（找到 $($noticeItems.Count) 张）"
$noticeText = ($noticeItems | ForEach-Object { $_.text }) -join "`n"
Check ($noticeText -match "已重试 $Retries 次仍失败") 'notice 卡片里带着「已重试 N 次仍失败」'
$asstWithError = @($thread.items | Where-Object { $_.kind -eq 'assistant' -and $_.text -match '请求异常' })
Check ($asstWithError.Count -eq 0) "报错**没有**混进助手正文（助手卡片命中 $($asstWithError.Count) 张）"
$queueText = ($snap.queue | ForEach-Object { $_.text }) -join ','
$running = $snap.runningThreadIds -contains $tid
Check ($queueText -match 'second-queued-message') '重试耗尽后不再消费队列（第 2 条仍在队列里）'
Check (-not $running) '会话已不在运行中（对话确实断开了）'
Write-Host "--- 会话文本（截断） ---"
Write-Host ('   ' + ($allText -replace "`r?`n", ' ').Substring(0, [Math]::Min(300, $allText.Length)))

# ── 5. 再用真协议改设置，验证落盘 ───────────────────────────────
$set = Rpc 'config.setRetryCount' @{ retries = 3 }
Check ($set.result.retryCount -eq 3) 'config.setRetryCount 返回新值 3'
Start-Sleep -Milliseconds 800
$onDisk = (Get-Content $cfg -Raw | ConvertFrom-Json).retryCount
Check ($onDisk -eq 3) "设置已落盘（config.json retryCount=$onDisk）"

# ── 6. 收尾 ─────────────────────────────────────────────────────
try { $ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'done', $ct.Token).Wait(3000) } catch { }
Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Get-Process -Name ada-coding, ada-gateway -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Copy-Item $backup $cfg -Force
$restored = Get-Content $cfg -Raw | ConvertFrom-Json
Write-Host ("已还原用户配置：activeProviderId={0} retryCount={1}" -f $restored.activeProviderId, $restored.retryCount)

Write-Host ''
if ($failures.Count -eq 0) { Write-Host 'RESULT: PASS — 请求重试次数设置 + 重试耗尽断开对话 全部通过'; exit 0 }
Write-Host "RESULT: FAIL — $($failures.Count) 项未通过:"; $failures | ForEach-Object { Write-Host "  - $_" }
exit 1
