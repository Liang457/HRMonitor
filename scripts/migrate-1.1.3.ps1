# scripts/migrate-1.1.3.ps1
# 从 1.1.2 及更早版本迁移到 1.1.3。
# 1.1.3 把全部进程间标识符从 HuaWeiHR 前缀改为 BleHR 前缀：
#   共享内存  Local\HuaWeiHR_SM         → Local\BleHR_SM
#   互斥体    Local\HuaWeiHR_daemon     → Local\BleHR_daemon
#             Local\HuaWeiHR_manager    → Local\BleHR_manager
#             Local\HuaWeiHR_manager_open → Local\BleHR_manager_open
#   窗口类    HuaWeiHRDaemonWnd         → BleHRDaemonWnd
#   注册表 Run 值名 HuaWeiHRManager     → BleHRManager
#   日志目录  %LOCALAPPDATA%\HuaWeiHR   → %LOCALAPPDATA%\BleHR
# 新旧版本的组件混跑互认不了，升级必须整体更换全部文件后再跑本脚本。
# 本脚本只随 1.1.3 发行版提供，之后的版本不再附带。
# 幂等：重复执行不报错，已迁移的项目会跳过。
#
#     powershell -ExecutionPolicy Bypass -File scripts\migrate-1.1.3.ps1

$ErrorActionPreference = 'Stop'

$RunKey   = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$OldValue = 'HuaWeiHRManager'
$NewValue = 'BleHRManager'
$TaskName = 'HuaweiHRDaemon'
$OldLog   = Join-Path $env:LOCALAPPDATA 'HuaWeiHR'
$NewLog   = Join-Path $env:LOCALAPPDATA 'BleHR'
$Root     = Split-Path -Parent $PSScriptRoot

$did = $false

# --- 停进程。共享内存、互斥体是进程生命期对象，重启后自然用新名字。
foreach ($proc in 'hr-daemon', 'hr-manager') {
    if (Get-Process -Name $proc -ErrorAction SilentlyContinue) {
        Stop-Process -Name $proc -Force
        Write-Host "[process] 已结束 $proc.exe"
        $did = $true
        Start-Sleep -Milliseconds 500
    } else {
        Write-Host "[process] $proc.exe 未在运行。"
    }
}

# --- 注册表 Run 值改名。
# 只改指向本目录 hr-manager.exe 的值，避免碰同名但指向别处的项。
$old = Get-ItemProperty -Path $RunKey -Name $OldValue -ErrorAction SilentlyContinue
if ($old) {
    $target = $old.$OldValue
    $expected = Join-Path $Root 'hr-manager.exe'
    $pointsHere = $target -and ($target -ieq $expected -or $target -like "$expected*")
    if ($pointsHere) {
        Set-ItemProperty -Path $RunKey -Name $NewValue -Value $target
        Remove-ItemProperty -Path $RunKey -Name $OldValue
        Write-Host "[run] 自启值 $OldValue 已改名为 $NewValue"
        $did = $true
    } else {
        Write-Warning "注册表值 $OldValue 指向 $target，不是本目录的 $expected，没有动它。"
    }
} else {
    if (Get-ItemProperty -Path $RunKey -Name $NewValue -ErrorAction SilentlyContinue) {
        Write-Host "[run] 自启值已是 $NewValue，跳过。"
    } else {
        Write-Host '[run] 注册表里没有旧自启值，跳过。'
    }
}

# --- 旧版计划任务残留（更早版本的自启方式）一并清掉。
if (Get-Command Unregister-ScheduledTask -ErrorAction SilentlyContinue) {
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($task) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Host "[task] 已删除旧版计划任务 '$TaskName'"
        $did = $true
    } else {
        Write-Host "[task] 没有旧版计划任务 '$TaskName'。"
    }
}

# --- 日志目录改名。目标已存在时跳过，避免覆盖。
if (Test-Path -LiteralPath $OldLog) {
    if (Test-Path -LiteralPath $NewLog) {
        Write-Warning "$NewLog 已存在，旧日志目录 $OldLog 保留未动，可自行合并或删除。"
    } else {
        Move-Item -LiteralPath $OldLog -Destination $NewLog
        Write-Host "[log] 日志目录已改名为 $NewLog"
        $did = $true
    }
} else {
    Write-Host '[log] 没有旧日志目录，跳过。'
}

if (-not $did) { Write-Host '没有需要迁移的内容。' }
Write-Host ''
Write-Host '迁移完成。请确认全部组件（hr-daemon.exe、hr-manager.exe、两个插件 DLL）'
Write-Host '都已换成 1.1.3 的文件，然后启动 hr-manager.exe 即可。'
