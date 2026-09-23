# scripts/build.ps1 — 防白构建（PLAN §10 #3，AGENTS.md 坑 1 的脚本化）
#
# 为什么存在：应用开着时 Windows 锁住 trae-signin-gui.exe，`npm run tauri build`
# 只刷新 deps/*.rlib，exe 内容不变——修复静默不生效（2026-09-15 事故：一次签到判据
# 修复因此失效一整天）。本脚本固定顺序：停进程 → 构建 → 核对 exe mtime 前移。
#
# 用法：powershell -ExecutionPolicy Bypass -File scripts/build.ps1
# 不含「启动」环节：构建完是否立刻开由使用者决定。

# 本文件必须保存为 UTF-8 **with BOM**（PS 5.1 无 BOM 会按 GBK 误读中文导致解析失败）
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = "Stop"

# ── 定位 exe（cargo workspace 根的 target/release，与仓库根同级） ──
$repoRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repoRoot "target\release\trae-signin-gui.exe"

if (-not (Test-Path $exe)) {
    Write-Host "[build] 未找到 $exe，将执行首次构建" -ForegroundColor Yellow
    $beforeMtime = $null
} else {
    $beforeMtime = (Get-Item $exe).LastWriteTime
    Write-Host ("[build] 当前 exe mtime: {0}" -f $beforeMtime)
}

# ── 1) 停止运行中的应用（Windows 锁住运行中的 exe = 白构建的根源） ──
$proc = Get-Process -Name "trae-signin-gui" -ErrorAction SilentlyContinue
if ($proc) {
    Write-Host "[build] 停止运行中的 trae-signin-gui（PID $($proc.Id -join ', '))" -ForegroundColor Yellow
    Stop-Process -Name "trae-signin-gui" -Force
    Start-Sleep -Milliseconds 800   # 等文件锁释放
} else {
    Write-Host "[build] 应用未在运行" -ForegroundColor DarkGray
}

# ── 2) 构建 ──
Write-Host "[build] npm run tauri build ..." -ForegroundColor Cyan
Push-Location $repoRoot
try {
    npm run tauri build
    if ($LASTEXITCODE -ne 0) {
        Write-Error "[build] 构建失败（exit $LASTEXITCODE），中止"
        exit $LASTEXITCODE
    }
} finally {
    Pop-Location
}

# ── 3) 核对 exe mtime 前移（构建成功 ≠ exe 更新） ──
if (-not (Test-Path $exe)) {
    Write-Error "[build] 构建后仍未找到 $exe，中止"
    exit 1
}
$afterMtime = (Get-Item $exe).LastWriteTime
if ($null -ne $beforeMtime -and $afterMtime -le $beforeMtime) {
    Write-Error ("[build] exe mtime 未前移（before={0} after={1}）——极可能应用仍在运行锁住了文件，白构建！请确认进程已全部退出后重试" -f $beforeMtime, $afterMtime)
    exit 1
}
Write-Host ("[build] OK exe mtime 前移: {0} -> {1}" -f $beforeMtime, $afterMtime) -ForegroundColor Green
Write-Host ("[build] 产物: {0} ({1:N1} MB)" -f $exe, ((Get-Item $exe).Length / 1MB)) -ForegroundColor Green
