// daemon/main.cpp — hr-daemon：采集心率，写进中立共享内存
//
// 共享内存 Local\BleHR_SM 是唯一输出，两个显示端都读它：
//   * hr_plugin.dll        → TrafficMonitor 任务栏
//   * HeartRate.dll        → MSI Afterburner 监控数据源 / OSD
// daemon 自己不画任何东西，OSD 的呈现完全归 Afterburner 管。
//
// GUI 子系统静默运行；从 cmd 启动时会附加到父控制台，日志同时写文件；
// --quiet（hr-manager 拉起时用）则不附加任何控制台，日志只进文件。
#include <windows.h>
#include <shellapi.h>

#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include "../common/hr_config.h"
#include "../common/hr_names.h"
#include "../common/hr_shared.h"
#include "../common/version.h"
#include "hr_source.h"
#include "log.h"

namespace {

constexpr UINT_PTR kTimerId     = 1;
constexpr UINT     kTimerMs     = 100;    // 主循环节拍（分辨率；出数据按配置的 refresh_ms）

struct Options {
    bool               demo        = false;   // 命令行覆盖配置
    bool               haveAddress = false;
    bool               addressGiven = false;  // 命令行显式给过 --address（哪怕解析失败）
    unsigned long long address     = 0;
    bool               help        = false;
    bool               debug       = false;   // --debug：连每条心率都写进日志
    bool               scan        = false;   // --scan：只扫描设备，按行输出 JSON 后退出
    int                scanSec     = 0;       // 0 = 未在命令行给出，用配置里的 scan_timeout_ms
    bool               quiet       = false;   // --quiet：不附加控制台，日志只写文件（hr-manager 拉起时用）
    bool               version     = false;   // --version：打印版本号后退出
};

// ---------------------------------------------------------------- 全局状态

HWND                      g_hwnd = nullptr;
HrSharedWriter            g_writer;
HrSink                    g_sink;
std::unique_ptr<HrSource> g_source;
bool                      g_unconfigured = false;  // 没配手表地址：不连接（见 Run 里的未配置分支）
HrConfig                  g_cfg;          // 启动时从 hr-daemon.ini 读入

ULONGLONG   g_lastTick1s  = 0;
bool        g_firstTick   = true;
int         g_prevOut     = -2;             // -2 = 尚未写过
DWORD       g_prevStatus  = (DWORD)-1;
std::wstring g_prevDevice;

// ---------------------------------------------------------------- 工具

// 把对外状态打包成一条记录。手表名按 UTF-8 存进定长字段。
void BuildRecord(HrSharedData& d, int out, DWORD status, const std::wstring& dev) {
    memset(&d, 0, sizeof(d));
    d.magic   = HRSM_MAGIC;
    d.version = HRSM_VERSION;
    d.bpm     = (LONG)out;
    d.tick_ms = GetTickCount64();
    d.status  = status;
    if (dev.empty()) return;

    // 先整体转成 UTF-8，再按字段大小截断。直接让 WideCharToMultiByte 写进定长字段
    // 的话，名字一长它就整体失败并返回 0，于是名字静默变成空串（两个插件的
    // tooltip 上就看不到手表名了）。
    // 转换缓冲复用（仅主线程定时器调用）：写共享内存每秒最多 10 次，没必要每次分配。
    static std::string utf8;
    const int need = WideCharToMultiByte(CP_UTF8, 0, dev.c_str(), (int)dev.size(),
                                         nullptr, 0, nullptr, nullptr);
    if (need <= 0) return;
    utf8.assign((size_t)need, '\0');
    if (WideCharToMultiByte(CP_UTF8, 0, dev.c_str(), (int)dev.size(),
                            utf8.data(), need, nullptr, nullptr) <= 0) return;

    const size_t cap = sizeof(d.device_name) - 1;
    size_t n = ((size_t)need <= cap) ? (size_t)need : cap;
    // 截断可能落在多字节字符中间，回退到字符边界，免得插件那边解出乱码
    while (n > 0 && ((unsigned char)utf8[n] & 0xC0) == 0x80) --n;
    memcpy(d.device_name, utf8.data(), n);
    d.device_name[n] = '\0';
}

// 每个主循环节拍（100ms）：判断是否超时、按需写共享内存。
void Tick() {
    const ULONGLONG    now = GetTickCount64();
    HrSink::Snapshot   s   = g_sink.Get();

    const bool have  = (s.bpm >= 0);
    const bool fresh = have && s.lastMs != 0 &&
                       (now - s.lastMs) <= (ULONGLONG)g_cfg.timeout_ms;
    const int  out   = fresh ? s.bpm : -1;

    DWORD status;
    if (!have)      status = g_unconfigured ? HRS_NODATA : s.status;
    else if (fresh) status = HRS_OK;
    // 曾经拿到过数据之后 bpm 不会再被清成 -1，所以"正在重连"只能靠数据源自己
    // 上报的状态来分辨；不加这一句的话每次重连都会被写成"已超时"。
    else            status = (s.status == HRS_CONNECTING) ? HRS_CONNECTING : HRS_TIMEOUT;

    const bool oneSec  = (now - g_lastTick1s) >= (ULONGLONG)g_cfg.refresh_ms;
    const bool changed = g_firstTick
                      || out != g_prevOut
                      || status != g_prevStatus
                      || s.device != g_prevDevice;
    if (!oneSec && !changed) return;
    if (oneSec) g_lastTick1s = now;

    // 中立共享内存：两个显示端都读这里。超时/无效时写 bpm=-1 + HRS_TIMEOUT，
    // 读取端（HrEffectiveBpm）据此显示 "--"。
    if (!g_writer.IsOpen()) {
        // 打不开多半是名字被同会话别的进程占了/权限不对。每 100ms 一条 WARN
        // 会把日志均匀灌满，只在第一条和之后每 30 秒提醒一次。
        static int s_openFailTicks = 0;   // 仅主线程定时器调用，无并发
        if (g_writer.Open()) {
            LogInfo("共享内存: 已就绪 %s", ToUtf8(HRSM_NAME).c_str());
            s_openFailTicks = 0;
        } else if (s_openFailTicks == 0) {
            LogWarn("共享内存: 打开失败 err=%lu", GetLastError());
            s_openFailTicks = 1;
        } else if (++s_openFailTicks % 300 == 0) {   // 300 拍 × 100ms = 30 秒
            LogWarn("共享内存: 仍打不开 err=%lu（每 30 秒提示一次）", GetLastError());
        }
    }
    HrSharedData rec;
    BuildRecord(rec, out, status, s.device);
    g_writer.Write(rec);

    if (g_firstTick || out != g_prevOut) {
        // 心率值本身走 DEBUG：手表的心率几乎每秒都在变，写进 INFO 日志很快就把
        // 文件刷爆了。想看到每一条就打开 log.debug（或 --debug）。
        if (out >= 0) LogDebug("心率: %d bpm", out);
        else if (g_unconfigured)
            LogInfo("心率: -- （未配置手表地址，不连接；请在 hr-manager 面板里扫描选表）");
        else          LogInfo("心率: -- （%s）",
                              status == HRS_TIMEOUT     ? "数据超时" :
                              status == HRS_CONNECTING  ? "连接中"   : "无数据");
    }
    if (!s.device.empty() && s.device != g_prevDevice)
        LogInfo("设备名: %s", ToUtf8(s.device).c_str());

    g_prevOut    = out;
    g_prevStatus = status;
    g_prevDevice = s.device;
    g_firstTick  = false;
}

// ---------------------------------------------------------------- 窗口与退出路径

LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    switch (msg) {
    case WM_TIMER:
        if (wp == kTimerId) { Tick(); return 0; }
        break;
    case WM_CLOSE:
        DestroyWindow(hwnd);
        return 0;
    case WM_DESTROY:
        KillTimer(hwnd, kTimerId);
        PostQuitMessage(0);
        return 0;
    case WM_ENDSESSION:      // 注销/关机
        DestroyWindow(hwnd);
        return 0;
    default:
        break;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

// 从 cmd 里 Ctrl+C，或注销/关机时，走正常退出路径。
BOOL WINAPI ConsoleCtrlHandler(DWORD type) {
    switch (type) {
    case CTRL_C_EVENT:
    case CTRL_BREAK_EVENT:
    case CTRL_CLOSE_EVENT:
    case CTRL_LOGOFF_EVENT:
    case CTRL_SHUTDOWN_EVENT:
        if (g_hwnd) PostMessageW(g_hwnd, WM_CLOSE, 0, 0);
        return TRUE;
    default:
        return FALSE;
    }
}

// 用日志通道输出，这样文件和（附加的）控制台都能看到，中文也不会乱码。
void PrintUsage() {
    LogInfo("");
    LogInfo("hr-daemon —— BLE 心率广播 → 共享内存（TrafficMonitor 任务栏 / Afterburner OSD）");
    LogInfo("");
    LogInfo("用法:");
    LogInfo("  hr-daemon.exe              连接配置里的手表（正常使用；未配地址则不连接）");
    LogInfo("  hr-daemon.exe --demo       用模拟心率源联调，无需手表");
    LogInfo("  hr-daemon.exe --address AA:BB:CC:DD:EE:FF");
    LogInfo("                             直连指定手表");
    LogInfo("  hr-daemon.exe --debug      连每条心率都写进日志（平时不写）");
    LogInfo("  hr-daemon.exe --quiet      不附加控制台，日志只写文件（hr-manager 拉起时用）");
    LogInfo("  hr-daemon.exe --version    显示版本号后退出");
    LogInfo("  hr-daemon.exe --help       显示本帮助");
    LogInfo("");
    LogInfo("  hr-daemon.exe --scan [秒数]");
    LogInfo("                             只扫描附近的心率广播设备，边扫边往 stdout");
    LogInfo("                             按行输出 JSON（{\"type\":\"device\",...}），");
    LogInfo("                             最后一行 {\"type\":\"done\",...}，然后退出");
    LogInfo("                             （给 hr-manager 的选表面板用）");
    LogInfo("");
    LogInfo("配置: %s（不存在则用默认值，可改）", ToUtf8(HrDaemonIniPath()).c_str());
    LogInfo("日志: exe 同级 log\\ 子目录里的 hr-daemon.log（写完即刷盘，可边跑边看）。");
    LogInfo("      每次启动开新的一份，最多留 3 份（.1 .2 是之前几次的）；");
    LogInfo("      单份超过 log.max_kb 也会就地轮转。");
    LogInfo("");
}

// ---------------------------------------------------------------- 主流程

int Run(HINSTANCE hInst, const Options& opt) {
    // ---- 单实例
    HANDLE mutex = CreateMutexW(nullptr, TRUE, HR_MUTEX_DAEMON);
    if (!mutex) {
        // 没有互斥体就没有单实例保护：第二个实例会跟自己抢共享内存和蓝牙，
        // 后果比不起守护进程更糟，所以直接退出
        LogError("创建单实例互斥体失败 err=%lu，退出", GetLastError());
        return 1;
    }
    if (GetLastError() == ERROR_ALREADY_EXISTS) {
        LogWarn("已经有一个 hr-daemon 在运行（互斥体 %s），本次启动退出", ToUtf8(HR_MUTEX_DAEMON).c_str());
        CloseHandle(mutex);
        return 0;
    }

    // ---- 隐藏窗口：只为拿到一个消息循环与可被 WM_CLOSE 打断的退出路径
    WNDCLASSEXW wc{};
    wc.cbSize        = sizeof(wc);
    wc.lpfnWndProc   = WndProc;
    wc.hInstance     = hInst;
    wc.lpszClassName = HR_WNDCLASS_DAEMON;
    if (!RegisterClassExW(&wc) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
        LogError("RegisterClassExW 失败 err=%lu", GetLastError());
        CloseHandle(mutex);
        return 1;
    }
    // 不调用 ShowWindow —— 窗口始终不可见，但仍是顶层窗口，
    // 因此 taskkill（不带 /F）和系统注销都能送 WM_CLOSE 进来。
    g_hwnd = CreateWindowExW(0, HR_WNDCLASS_DAEMON, L"hr-daemon", WS_OVERLAPPED,
                             0, 0, 0, 0, nullptr, nullptr, hInst, nullptr);
    if (!g_hwnd) {
        LogError("CreateWindowExW 失败 err=%lu", GetLastError());
        CloseHandle(mutex);
        return 1;
    }

    SetConsoleCtrlHandler(ConsoleCtrlHandler, TRUE);
    if (!SetTimer(g_hwnd, kTimerId, kTimerMs, nullptr)) {
        // 定时器起不来 = 主循环什么都不做，日志里却什么都看不出来
        LogError("SetTimer 失败 err=%lu", GetLastError());
        DestroyWindow(g_hwnd);
        if (mutex) CloseHandle(mutex);
        return 1;
    }

    // ---- 数据源。命令行参数优先于配置文件。
    const bool demo = opt.demo || g_cfg.demo;

    std::string err;
    if (demo) {
        LogInfo("模式: 模拟心率源（%s）",
                opt.demo ? "--demo 命令行" : "配置 source.demo=1");
        g_source = MakeDemoSource(g_sink);
    } else {
        BleConfig bc;
        bc.scan_timeout_ms = g_cfg.scan_timeout_ms;
        bc.backoff_min_sec = g_cfg.backoff_min_sec;
        bc.backoff_max_sec = g_cfg.backoff_max_sec;
        bc.timeout_ms      = g_cfg.timeout_ms;

        if (opt.haveAddress) {
            bc.haveAddress = true;
            bc.address     = opt.address;
            bc.nameHint    = HrFormatMac(opt.address);
            LogInfo("模式: 直连命令行指定地址 %s", ToUtf8(bc.nameHint).c_str());
        } else if (!g_cfg.address.empty()) {
            unsigned long long a = 0;
            if (HrParseMac(g_cfg.address, a)) {
                bc.haveAddress = true;
                bc.address     = a;
                bc.nameHint    = HrFormatMac(a);
                LogInfo("模式: 直连配置里的地址 %s", ToUtf8(bc.nameHint).c_str());
            } else {
                LogWarn("配置 source.address=\"%s\" 不是合法 MAC，保持不连接", ToUtf8(g_cfg.address).c_str());
            }
        }
        if (bc.haveAddress) {
            g_source = MakeBleSource(g_sink, bc);
        } else {
            // 旧行为是扫描并连第一台 0x180D 设备——多设备环境下会连错表，而且
            // 一旦连上手表就停止广播，想换表都不好扫。现在地址留空就保持断开，
            // 让用户在 hr-manager 的面板里明确选表（--scan 一次性扫描就是给它用的）。
            LogInfo("模式: 未配置手表地址，不连接（避免连错设备）；请在 hr-manager 面板里扫描选表");
            g_unconfigured = true;   // Tick 的状态行据此显示“未配置手表地址”
        }
    }

    // 未配置地址时 g_source 为空：不采集，但主循环照常跑（共享内存持续写 --）
    if (g_source && !g_source->Start(err)) {
        LogError("数据源启动失败: %s", err.c_str());
        g_source.reset();
    }

    if (opt.quiet)
        LogInfo("hr-daemon %s 已启动（PID %lu），可通过 hr-manager 面板或 hr-manager stop 停止",
                HR_VERSION_STRING, GetCurrentProcessId());
    else
        LogInfo("hr-daemon %s 已启动（PID %lu），按 Ctrl+C 退出",
                HR_VERSION_STRING, GetCurrentProcessId());

    // ---- 消息循环
    MSG msg;
    while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    // ---- 正常退出：关映射
    LogInfo("正在退出...");
    if (g_source) { g_source->Stop(); g_source.reset(); }
    g_writer.Close();
    if (mutex) CloseHandle(mutex);
    LogInfo("hr-daemon 已退出");
    return 0;
}

// 参数在日志就绪之前就要解析（--scan 决定日志要不要轮转），所以警告先攒进
// warns（UTF-8），等 LogInit 之后再统一打出来。
Options ParseArgs(int argc, wchar_t** argv, std::vector<std::string>& warns) {
    Options o;
    auto warn = [&warns](const char* fmt, ...) {
        char buf[512];
        va_list ap; va_start(ap, fmt);
        vsnprintf(buf, sizeof(buf), fmt, ap);
        va_end(ap);
        warns.emplace_back(buf);
    };
    for (int i = 1; i < argc; ++i) {
        const std::wstring a = argv[i];
        if (a == L"--demo" || a == L"-d") {
            o.demo = true;
        } else if (a == L"--debug") {
            o.debug = true;
        } else if (a == L"--quiet") {
            o.quiet = true;
        } else if (a == L"--version") {
            o.version = true;
        } else if (a == L"--help" || a == L"-h" || a == L"/?") {
            o.help = true;
        } else if (a == L"--address" || a == L"-a") {
            o.addressGiven = true;   // 显式指定过：解析失败就得拦下来，不能静默回退到配置
            if (i + 1 >= argc) { warn("--address 后面缺少 MAC 地址"); continue; }
            const std::wstring v = argv[++i];
            if (HrParseMac(v, o.address)) {
                o.haveAddress = true;
            } else {
                warn("无法解析 MAC 地址 \"%s\"，应形如 AA:BB:CC:DD:EE:FF", ToUtf8(v).c_str());
            }
        } else if (a == L"--scan") {
            o.scan = true;
            // 可选：紧跟一个纯数字当秒数
            if (i + 1 < argc) {
                wchar_t* end = nullptr;
                long sec = wcstol(argv[i + 1], &end, 10);
                if (end && *end == L'\0' && sec > 0) { o.scanSec = (int)sec; ++i; }
            }
        } else {
            warn("忽略未知参数 \"%s\"", ToUtf8(a).c_str());
        }
    }
    return o;
}

} // namespace

int WINAPI wWinMain(HINSTANCE hInst, HINSTANCE, PWSTR, int) {
    // 先读配置：日志上限在里面，所以要赶在 LogInit 之前。
    // 配置文件不存在/读不了就用默认值，不报错。
    const std::wstring iniPath = HrDaemonIniPath();
    const bool haveIni = (GetFileAttributesW(iniPath.c_str()) != INVALID_FILE_ATTRIBUTES);
    // Local 必须在 LogInit 之前拿到（日志上限从里面来），所以钳位提示先攒着。
    // notes 必须由 Load 内部填充 —— 拿 Load 之后的结果再 Sanitize 一遍是永远
    // 不会产生提示的，因为那时候值已经合法了。
    std::vector<std::wstring> notes;
    g_cfg = HrConfig::Load(iniPath, &notes);

    // 参数也要赶在 LogInit 之前：--scan 是一次性进程，不该轮转日志、也不该
    // 顶掉 daemon 的历史。它的警告先攒着，等日志开了再打。
    int argc = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    std::vector<std::string> argWarns;
    const Options opt = ParseArgs(argc, argv, argWarns);
    if (argv) LocalFree(argv);

    LogSetDebug(opt.debug || g_cfg.log_debug);
    LogSetQuiet(opt.quiet);   // 必须赶在 LogInit 之前：它决定要不要附加父控制台

    // 日志进 exe 同级的 log\ 子目录（发行目录的根只放 exe 和 README.md）。
    // 目录不存在就先建；建不出来（只读目录等）LogInit 自己会兜底到
    // %LOCALAPPDATA%\BleHR，不用在这里报错。
    const std::wstring logDir = HrJoinPath(HrExeDir(), L"log");
    CreateDirectoryW(logDir.c_str(), nullptr);   // 已存在则失败，忽略

    const std::wstring logPath = LogInit(logDir, g_cfg.log_max_kb, /*rotate=*/!opt.scan);
    if (!logPath.empty()) LogInfo("日志文件: %s", ToUtf8(logPath).c_str());
    else                  LogWarn("无法创建日志文件，仅输出到控制台");
    LogInfo("配置文件: %s%s", ToUtf8(iniPath).c_str(),
            haveIni ? "" : "（不存在，使用默认值）");
    for (const auto& w : argWarns) LogWarn("%s", w.c_str());
    for (const auto& n : notes) LogWarn("配置: %s", ToUtf8(n).c_str());

    if (opt.help) { PrintUsage(); LogShutdown(); return 0; }

    // 版本号来自 common\version.h（与 hr-manager 的 Cargo.toml 双副本同步）。
    if (opt.version) { LogInfo("hr-daemon %s", HR_VERSION_STRING); LogShutdown(); return 0; }

    // 命令行显式给了 --address 但没能用上（缺参数/格式错）：拒绝启动。
    // 静默回退去连配置里的旧手表，用户会以为连的是新指定的那台——更糟。
    if (opt.addressGiven && !opt.haveAddress) {
        LogError("--address 指定失败（缺参数或格式不是 AA:BB:CC:DD:EE:FF），拒绝启动，"
                 "不回退到配置里的地址");
        LogShutdown();
        return 2;
    }

    // ---- --scan：只扫设备，按行往 stdout 输出 JSON 后退出（给 hr-manager 用）。
    // 没在命令行给秒数时用配置里的 scan_timeout_ms——这个键以前只喂给走不到的
    // 自动扫描分支，现在它至少管着扫描的默认时长，不再是死配置。
    if (opt.scan) {
        int secs = opt.scanSec;
        if (secs <= 0)
            secs = g_cfg.scan_timeout_ms / 1000;
        if (secs < 1)    secs = 1;
        if (secs > 120)  secs = 120;
        LogInfo("扫描模式: 扫 %d 秒，结果按行输出到 stdout", secs);
        const int n = BleScanStream(secs);
        if (n < 0) LogError("扫描失败（蓝牙适配器不可用？）");
        LogShutdown();
        return n < 0 ? 1 : 0;
    }

    const int rc = Run(hInst, opt);
    LogShutdown();
    return rc;
}
