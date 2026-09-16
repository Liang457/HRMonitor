# scripts/deploy-afterburner-plugin.ps1
#
# 把 HeartRate.dll 装进 MSI Afterburner，并在 Afterburner 里启用这个插件。
#
# 背景（本机 Afterburner 4.6.6 实测）：
#   * 插件目录是 <Afterburner>\Plugins\Monitoring\，Afterburner 启动时扫描里面的
#     *.dll；总开关是根配置 <Afterburner>\MSIAfterburner.cfg 的
#     [Monitoring] EnablePlugins=1。
#   * 某个插件有没有启用，记在 <Afterburner>\Profiles\MSIAfterburner.cfg 的
#     [Monitoring] 段里，形如 "HwInfo.dll=1"。键名就是 DLL 的文件名。
#   * 插件说明是 <Afterburner>\Help\Plugins\Monitoring\<DLL 主名>（没有扩展名），
#     纯文本，显示在"选择插件"窗口的描述栏里。
#   * Afterburner 退出时会把内存里的配置整个写回 profile，所以必须按
#     "先关程序 -> 改文件 -> 再启动" 的顺序来；而且它会把插件 DLL 锁住，
#     不关掉就换不了文件。
#
# 脚本**不**动 [Settings] Sources= 和 [Source Heart rate] 这些段 —— 它们是
# Afterburner 自己的数据结构，硬改容易被它覆盖。启用插件之后在界面里点两下即可：
#     设置 -> 监控 -> 硬件监控图表列表里勾上 "Heart rate"
#     再点该项右边的 "设置" 按钮，把 Show in On-Screen Display 勾上
# 想改 OSD 上的显示文字/颜色/量程，也在那个属性窗口里改（Override graph name
# 改成 HR、Graph color、Min/Max limit 等）。
#
#     powershell -ExecutionPolicy Bypass -File scripts\deploy-afterburner-plugin.ps1
#
# 参数：
#   -AbDir <路径>    Afterburner 安装目录（默认 D:\Program Files\MSI Afterburner）
#   -DllPath <路径>  要安装的 DLL（默认 <仓库>\build\HeartRate.dll）
#   -Uninstall       卸载：删 DLL、删说明、删启用项
#   -KeepRunning     改完不重启 Afterburner（下次启动才生效）

[CmdletBinding()]
param(
    [string]$AbDir = 'D:\Program Files\MSI Afterburner',
    [string]$DllPath,
    [switch]$Uninstall,
    [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'

$PluginName = 'HeartRate'
$PluginFile = "$PluginName.dll"

# --- 0. 前置检查 ------------------------------------------------------------

if (-not (Test-Path -LiteralPath $AbDir)) {
    throw "找不到 Afterburner 目录：$AbDir（用 -AbDir 指定安装目录）"
}

$monDir  = Join-Path $AbDir 'Plugins\Monitoring'
$helpDir = Join-Path $AbDir 'Help\Plugins\Monitoring'
$rootCfg = Join-Path $AbDir 'MSIAfterburner.cfg'
$profCfg = Join-Path $AbDir 'Profiles\MSIAfterburner.cfg'
$exe     = Join-Path $AbDir 'MSIAfterburner.exe'

# 权限用"实际能不能写"来判断，而不是看进程在不在管理员组 ——
# 有些机器的 Afterburner 目录本来就允许写入（比如是解包出来的而不是装出来的），
# 那种情况下不需要管理员，硬卡管理员反而会误报。
function Test-Writable([string]$dir) {
    if (-not (Test-Path -LiteralPath $dir)) { return $false }
    $probe = Join-Path $dir ('.writetest-' + [guid]::NewGuid().ToString('N') + '.tmp')
    try {
        [System.IO.File]::WriteAllBytes($probe, [byte[]](0))
        Remove-Item -LiteralPath $probe -Force
        return $true
    } catch {
        return $false
    }
}

function Test-RunningElevated {
    ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

$probeDir = if (Test-Path -LiteralPath $monDir) { $monDir } else { $AbDir }
if (-not (Test-Writable $probeDir)) {
    throw "没有写入权限：$probeDir。请用【以管理员身份运行】的 PowerShell 再跑一次。"
}
if (-not (Test-RunningElevated)) {
    # Afterburner 通常以管理员身份运行，而它启动时会加载这个目录下所有 DLL。
    # 目录对普通用户可写 + 宿主是提权进程 = 谁都能把代码塞进那个提权进程里。
    # 这是本脚本有意的便利（有些机器的 Afterburner 目录本来就允许写入），
    # 但要让人知道代价。
    Write-Warning "Afterburner 目录当前用户就能写，而 MSIAfterburner.exe 一般以管理员运行："
    Write-Warning "  往 $probeDir 里放 DLL 等于往一个提权进程里注入代码，别把该目录的写权限开放给不受信任的账户。"
}

if (-not $DllPath) {
    $DllPath = Join-Path (Split-Path -Parent $PSScriptRoot) "build\$PluginFile"
}

if (-not (Test-Path -LiteralPath $profCfg)) {
    throw "找不到 $profCfg —— 先正常启动一次 Afterburner 让它生成配置。"
}

# --- 1. profile 的读写 ------------------------------------------------------
#
# 一律用 Latin-1（ISO-8859-1）读写：字节 0x00-0xFF 和字符一一对应，字符串进、
# 字节出能还原出完全相同的字节。这样除了我们插入/删除的那一行，原文件一个字节
# 都不会变 —— 不用担心 profile 的编码、BOM 或换行风格被改坏。
function Get-IniText([string]$path) {
    $bytes = [System.IO.File]::ReadAllBytes($path)
    return [System.Text.Encoding]::GetEncoding(28591).GetString($bytes)
}

# 写回时同样走"临时文件 + 原子替换"，并把旧文件留成 .bak：profile 里装着用户
# 全部的监控设置，直接截断重写遇到崩溃/断电就是一份残缺的配置。
function Set-IniText([string]$path, [string]$text) {
    $dir  = Split-Path -Parent $path
    $name = [System.IO.Path]::GetFileName($path)
    $tmp  = Join-Path $dir ($name + '.tmp')
    $bak  = Join-Path $dir ($name + '.bak')
    [System.IO.File]::WriteAllBytes($tmp, [System.Text.Encoding]::GetEncoding(28591).GetBytes($text))
    [System.IO.File]::Replace($tmp, $path, $bak)
}

# 注意：一律用 [ \t]* 而不是 \s* —— .NET 正则里 \s 会匹配换行，
# 会把下一行的内容一起吞掉，这点和 configure-trafficmonitor.ps1 一样。
$Eol = '[ \t]*=[ \t]*([^\r\n]*)'
$KeyRe = [regex]::Escape($PluginFile)      # HeartRate.dll -> 转义掉那个点

# 按路径认进程：只凭映像名匹配会碰到同名的别的进程。路径读不到（Afterburner
# 通常提权跑着，普通权限拿不到 Path）就按名字认，保持"关得掉它"的能力。
function Get-AbProcess {
    @(Get-Process -Name 'MSIAfterburner' -ErrorAction SilentlyContinue | Where-Object {
        $p = $null
        try { $p = $_.Path } catch { }
        (-not $p) -or ($p -ieq $exe)
    })
}

# Afterburner 通常以管理员身份运行（实测普通权限连 taskkill 都关不掉它），
# 所以这里要把"关不掉"变成一句人话，而不是让后面的 Copy-Item 抛个看不懂的 IO 错。
function Stop-Afterburner {
    $p = Get-AbProcess
    if ($p.Count -eq 0) { return $false }
    Write-Host "[ab] 关闭 Afterburner (PID $($p.Id -join ', ')) ..."

    try { $p | ForEach-Object { $_.CloseMainWindow() | Out-Null } } catch { }
    try { $p | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue } catch { }

    if ((Get-AbProcess).Count -gt 0) {
        # 主窗口的 × 一般只是收进托盘，进程还在，只能强杀。
        try {
            Get-AbProcess | Stop-Process -Force -ErrorAction Stop
            Start-Sleep -Milliseconds 500
        } catch {
            throw ("关不掉 Afterburner —— 它多半是以管理员身份运行的。" +
                   "请先在托盘图标上右键退出它，或者用【以管理员身份运行】的 PowerShell 再跑本脚本。")
        }
    }

    if ((Get-AbProcess).Count -gt 0) {
        throw 'Afterburner 还在运行，它锁着插件 DLL，没法替换。请手动退出后再跑。'
    }
    return $true
}

$wasRunning = Stop-Afterburner

if ($Uninstall) {
    # --- 2a. 卸载 -----------------------------------------------------------
    $installed = Join-Path $monDir $PluginFile
    if (Test-Path -LiteralPath $installed) {
        Remove-Item -LiteralPath $installed -Force
        Write-Host "[ab] 已删除 $installed"
    }
    $helpFile = Join-Path $helpDir $PluginName
    if (Test-Path -LiteralPath $helpFile) {
        Remove-Item -LiteralPath $helpFile -Force
        Write-Host "[ab] 已删除 $helpFile"
    }
    $text = Get-IniText $profCfg
    $new = [regex]::Replace($text, "(?m)^$KeyRe$Eol\r?\n", '')
    if ($new -ne $text) {
        Set-IniText $profCfg $new
        Write-Host "[ab] 已从 profile 的 [Monitoring] 移除 $PluginFile"
    } else {
        Write-Host "[ab] profile 里本来就没有 $PluginFile"
    }
    Write-Host ''
    Write-Host '卸载完成。Afterburner 里那条 Heart rate 曲线需要手动在监控列表里取消勾选。'
} else {
    # --- 2b. 安装 -----------------------------------------------------------
    if (-not (Test-Path -LiteralPath $DllPath)) {
        throw "找不到 $DllPath —— 先构建：cmd /c ab-plugin\build.cmd"
    }

    if (-not (Test-Path -LiteralPath $monDir))  { New-Item -ItemType Directory -Force -Path $monDir  | Out-Null }
    if (-not (Test-Path -LiteralPath $helpDir)) { New-Item -ItemType Directory -Force -Path $helpDir | Out-Null }

    $target = Join-Path $monDir $PluginFile
    Copy-Item -LiteralPath $DllPath -Destination $target -Force
    Write-Host "[ab] 已安装 $target"

    # 说明文件是 ANSI 纯文本（官方那 7 个说明文件都是），所以内容用 ASCII 英文，
    # 免得换一台机器、换一个代码页就变成乱码。
    $helpText = @"
This plugin provides [b]Heart rate[/b] hardware monitoring data source, read from
a BLE heart rate monitor by the hr-daemon helper process.

Hints:
- Start hr-daemon.exe first, otherwise the data source stays unavailable.
- It appears in the hardware monitoring graphs list as [b]Heart rate[/b], units BPM.
- Default graph range is 40-180 BPM; change Min/Max limit in the source properties.
- To put it on the On-Screen Display, open the source properties and enable
  [b]Show in On-Screen Display[/b].
"@
    $helpFile = Join-Path $helpDir $PluginName
    [System.IO.File]::WriteAllText($helpFile, $helpText, [System.Text.Encoding]::ASCII)
    Write-Host "[ab] 已写插件说明 $helpFile"

    # --- 3. 启用插件（写 profile 的 [Monitoring] 段）
    # 替换串一律用 MatchEvaluator 委托返回字面量：.NET 的替换模式里 $ 是元字符
    # （$1 / $& / $`），现在值恰好是常量，但照着抄到别处就会踩坑。
    $text = Get-IniText $profCfg
    if ($text -match "(?m)^$KeyRe$Eol") {
        $new = [regex]::Replace($text, "(?m)^$KeyRe$Eol", { param($m) "$PluginFile=1" })
        Set-IniText $profCfg $new
        Write-Host "[ab] profile 里已有该项，已置为 1"
    } elseif ($text -match '(?m)^\[Monitoring\][ \t]*\r?$') {
        # 插在 [Monitoring] 表头后面，保持同一个段的写法
        $new = [regex]::Replace($text, '(?m)^(\[Monitoring\][ \t]*\r?\n)',
                                { param($m) $m.Groups[1].Value + $PluginFile + "=1`r`n" })
        Set-IniText $profCfg $new
        Write-Host "[ab] 已写入 [Monitoring] $PluginFile=1"
    } else {
        # 没有 [Monitoring] 段就补一个在文件末尾
        $sep = if ($text.EndsWith("`n")) { '' } else { "`r`n" }
        Set-IniText $profCfg ($text + $sep + "[Monitoring]`r`n$PluginFile=1`r`n")
        Write-Host "[ab] 已追加 [Monitoring] 段并启用插件"
    }

    # --- 4. 总开关 EnablePlugins（根配置）
    if (Test-Path -LiteralPath $rootCfg) {
        $t = Get-IniText $rootCfg
        if ($t -match '(?m)^EnablePlugins[ \t]*=[ \t]*([^\r\n]*)') {
            if ($Matches[1].Trim() -ne '1') {
                $t2 = [regex]::Replace($t, '(?m)^EnablePlugins[ \t]*=[ \t]*([^\r\n]*)',
                                       "EnablePlugins`t`t`t= 1")
                Set-IniText $rootCfg $t2
                Write-Host '[ab] 根配置 EnablePlugins 已置为 1'
            } else {
                Write-Host '[ab] 根配置 EnablePlugins 已经是 1'
            }
        } else {
            Write-Warning '根配置里没找到 EnablePlugins —— 请在 设置 -> 常规 里确认插件功能已开'
        }
    }
}

# --- 5. 重新启动 ------------------------------------------------------------

if ($wasRunning -and -not $KeepRunning) {
    Write-Host '[ab] 重新启动 Afterburner ...'
    Start-Process -FilePath $exe -WorkingDirectory $AbDir
    Start-Sleep -Seconds 3
}

if (-not $Uninstall) {
    Write-Host ''
    Write-Host '完成。接下来在 Afterburner 界面里点两下：'
    Write-Host '  1) 设置 -> 监控 -> 在硬件监控图表列表里勾上 Heart rate'
    Write-Host '  2) 选中它，点右边的设置按钮，把 Show in On-Screen Display 勾上'
    Write-Host ''
    Write-Host '想改 OSD 上的样子，也在这个属性窗口里：'
    Write-Host '  Override graph name = HR        把显示名缩短'
    Write-Host '  Graph color / Min limit / Max limit'
    Write-Host ''
    Write-Host '确认数值在动：先跑 build\hr-daemon.exe --demo，然后看 Afterburner 的监控曲线。'
    Write-Host '这一步不需要 RTSS 在跑，也不需要有 3D 程序。'
}
