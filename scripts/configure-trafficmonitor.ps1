# scripts/configure-trafficmonitor.ps1
#
# 把本插件的"心率"项目放进 TrafficMonitor 的显示列表。
#
# 背景（V1.86 实测）：
#   * 插件 DLL 要放在 <TrafficMonitor.exe 同级>\plugins\ 下，文件名任意，扩展名必须是
#     .dll —— V1.86 只会扫描 plugins\*.dll，"*.tmd" 在本版里根本不存在（源码里搜不到
#     这个字符串）。TrafficMonitor 启动时自动加载，不用点"启用"。
#   * 但"显示哪个项目"要单独勾选。plugin_display_item 是逗号分隔的项目 id 列表，
#     直接改 config.ini 是有效的。
#   * "在任务栏显示"这个开关（show_task_bar_wnd）实测**改 config.ini 不一定生效**，
#     可靠做法是右键通知区图标 -> "显示任务栏窗口"。脚本仍会把它写成 true，但如果
#     任务栏上没出现，请用菜单手动开一次（开一次之后就会被记住）。
#
# config.ini 装着用户的全部设置（窗口布局、项目顺序、逐项颜色），所以本脚本：
#   * 先按 BOM 判断编码，读进来什么编码就写回什么编码；
#   * 改之前先备份成 config.ini.bak，写临时文件再原子替换，中途出事不会留下半份配置；
#   * 改完重读一遍确认锚点键还在。
# TrafficMonitor 退出时会把内存里的配置整个写回去，所以必须"先关程序 -> 改文件 ->
# 再启动"，否则改动会被覆盖。
#
#     powershell -ExecutionPolicy Bypass -File scripts\configure-trafficmonitor.ps1
#
# 参数：
#   -TmDir <路径>       TrafficMonitor 安装目录
#                       （不传就读 hr-daemon.ini 的 integration.tm_dir，再猜几个常见位置）
#   -ItemId <id>        要显示的插件项目 id（默认 hr）
#   -HideMainWindow     顺便把悬浮主窗口隐藏（只要任务栏显示时用）
#   -KeepRunning        改完不重启 TrafficMonitor（下次启动才生效）

[CmdletBinding()]
param(
    [string]$TmDir = '',
    [string]$ItemId = 'hr',
    [switch]$HideMainWindow,
    [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'

# --- 解析 TrafficMonitor 目录 -------------------------------------------------
# 以前这里写死一个本机路径，等于仓库里带着别人的机器布局。改成：显式参数 ->
# hr-daemon.ini 的 integration.tm_dir -> 挨个试常见位置。

function Get-HrIniTmDir {
    $ini = Join-Path (Split-Path -Parent $PSScriptRoot) 'build\hr-daemon.ini'
    if (-not (Test-Path -LiteralPath $ini)) { return $null }
    $sec = ''
    foreach ($line in [System.IO.File]::ReadAllLines($ini, [System.Text.Encoding]::UTF8)) {
        $t = $line.Trim()
        if ($t -eq '' -or $t.StartsWith(';') -or $t.StartsWith('#')) { continue }
        if ($t.StartsWith('[') -and $t.EndsWith(']')) {
            $sec = $t.Substring(1, $t.Length - 2).Trim()
            continue
        }
        if ($sec -ne 'integration') { continue }
        $i = $t.IndexOf('=')
        if ($i -le 0) { continue }
        if ($t.Substring(0, $i).Trim() -ne 'tm_dir') { continue }
        $v = $t.Substring($i + 1).Trim()
        if ($v) { return $v }
    }
    return $null
}

function Resolve-TmDir([string]$explicit) {
    if ($explicit) { return $explicit }

    $fromIni = Get-HrIniTmDir
    if ($fromIni) {
        Write-Host "[tm] 目录取自 hr-daemon.ini 的 integration.tm_dir: $fromIni"
        return $fromIni
    }

    foreach ($p in @(
            (Join-Path $env:ProgramFiles 'TrafficMonitor'),
            (Join-Path ${env:ProgramFiles(x86)} 'TrafficMonitor'),
            'D:\Program Files\TrafficMonitor',
            'D:\TrafficMonitor'
        )) {
        if ($p -and (Test-Path -LiteralPath (Join-Path $p 'TrafficMonitor.exe'))) {
            Write-Host "[tm] 自动找到 TrafficMonitor: $p"
            return $p
        }
    }
    throw "找不到 TrafficMonitor。请用 -TmDir 指定安装目录，或先跑 hr-manager set tm_dir <路径>"
}

$TmDir = Resolve-TmDir $TmDir

$ini = Join-Path $TmDir 'config.ini'
$exe = Join-Path $TmDir 'TrafficMonitor.exe'

if (-not (Test-Path -LiteralPath $ini)) {
    throw "找不到 $ini —— 先启动一次 TrafficMonitor 让它生成配置文件。"
}

# --- 读写：按编码读、原编码写回、原子替换 ------------------------------------
#
# 编码不能只看 BOM 猜一次就算了：实测这份 config.ini 是 **UTF-8 带 BOM**
# （而更早的版本是 UTF-16LE），也就是说 TrafficMonitor 自己的编码是会变的。
# 所以按"哪个编码读出来能找到锚点键"来判定，认不出来就在动手写之前报错退出 ——
# 绝不把认错的文本写回去。

function Get-CandidateEncodings([byte[]]$bytes) {
    $utf8    = New-Object System.Text.UTF8Encoding($true)
    $utf8Bom = New-Object System.Text.UTF8Encoding($false)
    $unicode = [System.Text.Encoding]::Unicode            # UTF-16LE
    $beUni   = [System.Text.Encoding]::BigEndianUnicode
    $ansi    = [System.Text.Encoding]::Default

    # BOM 指到谁，谁先试；其余按常见程度排后面。
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        return @($utf8, $utf8Bom, $unicode, $beUni, $ansi)
    }
    if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) {
        return @($unicode, $beUni, $utf8Bom, $ansi)
    }
    if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFE -and $bytes[1] -eq 0xFF) {
        return @($beUni, $unicode, $utf8Bom, $ansi)
    }
    return @($unicode, $utf8Bom, $ansi, $beUni)
}

function Read-TextWithEncoding([string]$path, [string[]]$anchors) {
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $tried = @()
    foreach ($enc in (Get-CandidateEncodings $bytes)) {
        $text = $null
        try { $text = $enc.GetString($bytes) } catch { continue }
        $ok = $true
        foreach ($a in $anchors) {
            if ($text -notmatch ('(?m)^' + [regex]::Escape($a) + '[ \t]*=')) { $ok = $false; break }
        }
        if ($ok) {
            return [pscustomobject]@{ Text = $text; Encoding = $enc }
        }
        $tried += $enc.WebName
    }
    throw ("认不出 $path 的编码，或者它不是一个正常的 TrafficMonitor 配置" +
           "（试过：$($tried -join ', ')；找的是：$($anchors -join ', ')）。文件没有被改动。")
}

function Write-TextAtomic([string]$path, [string]$text, [System.Text.Encoding]$enc) {
    $name = [System.IO.Path]::GetFileName($path)
    $dir  = Split-Path -Parent $path
    $tmp  = Join-Path $dir ($name + '.tmp')
    $bak  = Join-Path $dir ($name + '.bak')
    [System.IO.File]::WriteAllBytes($tmp, $enc.GetBytes($text))
    # Replace 是原子的，顺手把旧文件留成 .bak
    [System.IO.File]::Replace($tmp, $path, $bak)
    Write-Host "[tm] 已写入 $path（旧文件备份为 $name.bak）"
}

# --- 0. 先确认插件 DLL 在位
$plugin = Join-Path $TmDir 'plugins\hr_plugin.dll'
if (Test-Path -LiteralPath $plugin) {
    Write-Host "[tm] 插件: $plugin"
} else {
    Write-Warning "没找到 $plugin —— 先构建并拷过去：cmd /c tm-plugin\build.cmd"
    Write-Warning "现在继续的话，显示列表里会留下一个没有对应插件的 'hr' 项。"
}

# --- 1. 关掉 TrafficMonitor，否则它退出时会覆盖我们的改动
# 按路径认进程：只凭映像名匹配会碰到同名的别的进程。路径读不到（比如它提权跑着）
# 就按名字认，保持"关得掉它"的能力。
$proc = @(Get-Process -Name 'TrafficMonitor' -ErrorAction SilentlyContinue | Where-Object {
    $p = $null
    try { $p = $_.Path } catch { }
    (-not $p) -or ($p -ieq $exe)
})
$wasRunning = $proc.Count -gt 0
if ($wasRunning) {
    Write-Host "[tm] 关闭 TrafficMonitor (PID $($proc.Id -join ', ')) ..."
    $proc | ForEach-Object { $_.CloseMainWindow() | Out-Null }
    $proc | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
    $proc = @(Get-Process -Name 'TrafficMonitor' -ErrorAction SilentlyContinue | Where-Object {
        $p = $null
        try { $p = $_.Path } catch { }
        (-not $p) -or ($p -ieq $exe)
    })
    if ($proc.Count -gt 0) {
        $proc | Stop-Process -Force
        Start-Sleep -Milliseconds 500
    }
}

# --- 2. 改配置
# 注意：一律用 [ \t]* 而不是 \s* —— .NET 正则里 \s 会匹配换行，
# 'plugin_display_item\s*=(.*)$' 会把下一行的内容一起吞掉。
$read = Read-TextWithEncoding $ini @('plugin_display_item')
$text = $read.Text
$Eol  = '[ \t]*=[ \t]*([^\r\n]*)'

# 注意替换串：.NET 里 $ 是替换模式里的元字符（$1 / $& / $`），而这里的值来自
# 文件内容和 -ItemId。用 MatchEvaluator 委托返回字面量，绕开整个转义问题。
$fixup = $false

# plugin_display_item = 逗号分隔的插件项目 id 集合（StringSet）
if ($text -match "(?m)^plugin_display_item$Eol") {
    $cur = $Matches[1].Trim()
    # 最外层的 @(...) 不能省：只有一个 id 时 -split 返回的是字符串而不是数组，
    # 那样 `$ids += $ItemId` 会变成字符串拼接（'hr' + 'hrx' = 'hrhrx'），
    # 逗号分隔的列表就被写坏了。
    $ids = @()
    if ($cur) { $ids = @($cur -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }
    if ($ids -notcontains $ItemId) { $ids += $ItemId }
    $new = ($ids -join ',')
    $text = [regex]::Replace($text, "(?m)^plugin_display_item$Eol", { param($m) "plugin_display_item = $new" })
    $fixup = $true
    Write-Host "[tm] plugin_display_item = $new"
} else {
    throw 'config.ini 里没有 plugin_display_item，请先正常启动一次 TrafficMonitor。'
}

if ($text -match "(?m)^show_task_bar_wnd$Eol") {
    $text = [regex]::Replace($text, "(?m)^show_task_bar_wnd$Eol", 'show_task_bar_wnd = true')
    Write-Host '[tm] show_task_bar_wnd = true   (若任务栏上没出现，请右键通知区图标 -> 显示任务栏窗口)'
}

if ($HideMainWindow -and ($text -match "(?m)^hide_main_window$Eol")) {
    $text = [regex]::Replace($text, "(?m)^hide_main_window$Eol", 'hide_main_window = 1')
    Write-Host '[tm] hide_main_window = 1   (隐藏悬浮主窗口)'
}

Write-TextAtomic $ini $text $read.Encoding

# --- 3. 重读校验：确认我们没把文件改坏（比如正则失手吞掉一整段）
$after = Read-TextWithEncoding $ini @('plugin_display_item')
if ($after.Text.Length -lt [int]($read.Text.Length * 0.9)) {
    throw "改写后的 config.ini 明显变短了（$($read.Text.Length) -> $($after.Text.Length)）—— 请用 $ini.bak 恢复"
}

# --- 4. 重新启动
if ($wasRunning -and -not $KeepRunning) {
    Write-Host '[tm] 重新启动 TrafficMonitor ...'
    Start-Process -FilePath $exe -WorkingDirectory $TmDir
    Start-Sleep -Seconds 3
}

Write-Host ''
Write-Host '完成。任务栏上应该能看到 "HR 130" 这样的项目（<15 秒没数据时显示 "HR --"）。'
Write-Host '调整位置/字体/顺序：右键任务栏上的该项 -> 显示设置；或右键通知区图标 -> 选项设置。'
