// daemon/log.cpp
#include "log.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <vector>

namespace {

HANDLE   g_file = INVALID_HANDLE_VALUE;
HANDLE   g_con  = INVALID_HANDLE_VALUE;
CRITICAL_SECTION g_cs;
bool     g_csReady = false;
ULONGLONG g_maxBytes = 4ull * 1024 * 1024;   // 由 LogInit 按配置覆盖
ULONGLONG g_written  = 0;                    // 当前这份已经写了多少字节
std::wstring g_dir;                          // 当前日志所在目录（轮转时要用）
bool     g_rotate = true;                    // 这次启动要不要开新的一份
bool     g_rotateBroken = false;             // 轮转失败过，就别每行都再试
bool     g_debug = false;                    // DEBUG 级日志开关（默认关）

// 磁盘上最多留几份：hr-daemon.log + .1 + .2
constexpr int kKeepFiles = 3;

// 轮转后写进新文件的第一行，免得看日志的人以为程序重启了
const char kRotatedNote[] = "--- 上一份写满了，已轮转（那份现在是 hr-daemon.1.log）---\r\n";

// exe 目录写不进去（例如装到 C:\Program Files）时的兜底位置。
// 环境变量可能长过 MAX_PATH，所以先用 nullptr 问长度再取；失败也不要退回
// "." + "\HuaWeiHR" —— 那是相对当前工作目录，会莫名其妙在别处建目录。
std::wstring LocalAppDataFallbackDir() {
    auto env = [](const wchar_t* name) -> std::wstring {
        const DWORD need = GetEnvironmentVariableW(name, nullptr, 0);
        if (need == 0) return std::wstring();
        std::vector<wchar_t> buf((size_t)need);
        const DWORD got = GetEnvironmentVariableW(name, buf.data(), need);
        if (got == 0 || got >= need) return std::wstring();
        return std::wstring(buf.data(), got);
    };

    std::wstring dir = env(L"LOCALAPPDATA");
    if (dir.empty()) dir = env(L"TEMP");          // 总会是个绝对路径
    if (dir.empty()) return std::wstring();

    dir += L"\\HuaWeiHR";
    CreateDirectoryW(dir.c_str(), nullptr);   // 已存在则失败，忽略
    return dir;
}

// 第 idx 份的名字：0 = hr-daemon.log（本次），1/2 = 之前几次的
std::wstring LogPathAt(const std::wstring& dir, int idx) {
    std::wstring path = dir;
    if (!path.empty() && path.back() != L'\\' && path.back() != L'/') path += L'\\';
    path += L"hr-daemon";
    if (idx > 0) {
        path += L'.';
        path += std::to_wstring(idx);
    }
    path += L".log";
    return path;
}

// 把每份都往后顶一格，最老的删掉：当前 → .1、.1 → .2、.2 删。
// 返回 false 表示"当前这份挪不走"（多半被别的进程占着），调用方就别指望轮转了。
bool ShiftLogs(const std::wstring& dir) {
    DeleteFileW(LogPathAt(dir, kKeepFiles - 1).c_str());
    for (int i = kKeepFiles - 2; i >= 0; --i) {
        if (MoveFileW(LogPathAt(dir, i).c_str(), LogPathAt(dir, i + 1).c_str())) continue;
        const DWORD err = GetLastError();
        if (err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND) continue;  // 还没这一份，正常
        if (i == 0) return false;
    }
    return true;
}

// 打开第 0 份。fresh=true 覆盖重建（开新的一份），false 追加（--scan 模式）。
// FILE_SHARE_DELETE 是为了让"开着句柄也能改名"——运行中轮转要用。
bool OpenCurrent(const std::wstring& dir, bool fresh) {
    if (dir.empty()) return false;
    const std::wstring path = LogPathAt(dir, 0);

    // 追加模式（--scan 这种一次性进程，不轮转）没别的封顶手段：
    // 已经超过上限就先清掉重来，别让它无限长下去。轮转模式下文件刚开过，不用查。
    if (!fresh) {
        WIN32_FILE_ATTRIBUTE_DATA fad{};
        if (GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &fad)) {
            ULARGE_INTEGER sz{};
            sz.LowPart  = fad.nFileSizeLow;
            sz.HighPart = fad.nFileSizeHigh;
            if (sz.QuadPart > g_maxBytes) DeleteFileW(path.c_str());
        }
    }

    HANDLE h = CreateFileW(path.c_str(), FILE_APPEND_DATA,
                           FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr,
                           fresh ? CREATE_ALWAYS : OPEN_ALWAYS,
                           FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) return false;
    g_file    = h;
    g_dir     = dir;
    g_written = 0;
    return true;
}

// 底层写入（不做轮转判断，rotate 时写说明行要用它，避免递归）
void WriteRawBytes(const char* s, int len) {
    if (len <= 0) return;
    if (g_file != INVALID_HANDLE_VALUE) {
        DWORD written = 0;
        if (WriteFile(g_file, s, (DWORD)len, &written, nullptr)) g_written += written;
        FlushFileBuffers(g_file);   // 崩溃/被杀时也能看到最后一行
    }
    if (g_con != INVALID_HANDLE_VALUE) {
        // 日志是 UTF-8，转成 UTF-16 再写控制台，避免中文变乱码。
        int wlen = MultiByteToWideChar(CP_UTF8, 0, s, len, nullptr, 0);
        if (wlen > 0) {
            std::wstring w((size_t)wlen, L'\0');
            MultiByteToWideChar(CP_UTF8, 0, s, len, w.data(), wlen);
            DWORD written = 0;
            WriteConsoleW(g_con, w.data(), (DWORD)w.size(), &written, nullptr);
        }
    }
}

// 当前这份写满了就轮转：顶成 .1，另开一份接着写。
// 挪不动/开不了就把当前这份继续用下去（宁可不轮转，也不能把日志写丢）。
void RotateIfNeeded(int len) {
    if (!g_rotate || g_rotateBroken) return;
    if (g_written == 0 || g_written + (ULONGLONG)len <= g_maxBytes) return;

    if (!ShiftLogs(g_dir)) {
        g_rotateBroken = true;
        return;
    }
    HANDLE old = g_file;                 // 改名不影响已打开的句柄，留着兜底
    if (OpenCurrent(g_dir, /*fresh=*/true)) {
        if (old != INVALID_HANDLE_VALUE) CloseHandle(old);
        WriteRawBytes(kRotatedNote, (int)(sizeof(kRotatedNote) - 1));
        return;
    }
    // 新文件开不了：把刚挪走的那份挪回来，继续往它写
    MoveFileW(LogPathAt(g_dir, 1).c_str(), LogPathAt(g_dir, 0).c_str());
    g_rotateBroken = true;
}

void WriteRaw(const char* s, int len) {
    RotateIfNeeded(len);
    WriteRawBytes(s, len);
}

void LogV(bool debug, const char* level, const char* fmt, va_list ap) {
    if (debug && !g_debug) return;

    char msg[2048];
    vsnprintf(msg, sizeof(msg), fmt, ap);

    SYSTEMTIME st;
    GetLocalTime(&st);

    char line[2200];
    const int n = _snprintf_s(line, sizeof(line), _TRUNCATE,
                              "[%04u-%02u-%02u %02u:%02u:%02u.%03u] [%s] %s\r\n",
                              st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
                              st.wMilliseconds, level, msg);
    // _TRUNCATE 下输出被截断时返回的是 -1 —— 内容是写好了的（截断后的文本，
    // 且以 NUL 结尾）。以前这里把 n <= 0 当"没内容"整行丢掉，于是"日志行太长"
    // 的后果变成"这一行彻底不见了"，对排查来说是最糟的失败方式。
    if (n == 0 && line[0] == '\0') return;
    const int len = (n > 0) ? n : (int)strlen(line);
    if (len <= 0) return;

    if (g_csReady) EnterCriticalSection(&g_cs);
    WriteRaw(line, len);
    if (g_csReady) LeaveCriticalSection(&g_cs);
}

} // namespace

std::wstring LogInit(const std::wstring& exeDir, int maxKb, bool rotate) {
    if (maxKb < 64) maxKb = 4096;                 // 0 / 负值 → 默认
    g_maxBytes     = (ULONGLONG)maxKb * 1024ull;
    g_rotate       = rotate;
    g_rotateBroken = false;

    if (!g_csReady) {
        InitializeCriticalSection(&g_cs);
        g_csReady = true;
    }

    // 用 GUI 子系统编译，默认没有控制台。分两种情况：
    //   1) 从 cmd 直接启动 —— 没有控制台，附加到父进程的；
    //   2) 由 `start` 启动、继承了控制台 —— AttachConsole 会以 ERROR_ACCESS_DENIED
    //      失败（已经附加），此时 GetConsoleWindow() 已经有效。
    // 两种情况都统一用 GetConsoleWindow() 判断是否拿得到控制台。
    if (g_con == INVALID_HANDLE_VALUE) {
        if (GetConsoleWindow() == nullptr)
            AttachConsole(ATTACH_PARENT_PROCESS);   // 失败也无所谓，下面再判断
        if (GetConsoleWindow() != nullptr) {
            g_con = CreateFileW(L"CONOUT$", GENERIC_WRITE, FILE_SHARE_WRITE,
                                nullptr, OPEN_EXISTING, 0, nullptr);
        }
    }
    SetConsoleOutputCP(CP_UTF8);

    // 开新的一份：先把上次的顶成 .1（未启动时没有句柄占用，这里总能成功）
    if (!exeDir.empty()) {
        if (g_rotate) ShiftLogs(exeDir);
        if (OpenCurrent(exeDir, g_rotate)) return LogPathAt(exeDir, 0);
    }
    const std::wstring fallback = LocalAppDataFallbackDir();
    if (!fallback.empty()) {
        if (g_rotate) ShiftLogs(fallback);
        if (OpenCurrent(fallback, g_rotate)) return LogPathAt(fallback, 0);
    }
    return std::wstring();
}

void LogSetDebug(bool on) {
    g_debug = on;
}

void LogShutdown() {
    if (g_csReady) EnterCriticalSection(&g_cs);
    if (g_file != INVALID_HANDLE_VALUE) { CloseHandle(g_file); g_file = INVALID_HANDLE_VALUE; }
    if (g_con  != INVALID_HANDLE_VALUE) { CloseHandle(g_con);  g_con  = INVALID_HANDLE_VALUE; }
    if (g_csReady) LeaveCriticalSection(&g_cs);
}

std::string ToUtf8(const std::wstring& w) {
    if (w.empty()) return std::string();
    int n = WideCharToMultiByte(CP_UTF8, 0, w.c_str(), (int)w.size(),
                                nullptr, 0, nullptr, nullptr);
    if (n <= 0) return std::string();
    std::string s((size_t)n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, w.c_str(), (int)w.size(),
                        s.data(), n, nullptr, nullptr);
    return s;
}

void LogF(const char* fmt, ...) {
    va_list ap; va_start(ap, fmt);
    LogV(false, "INFO", fmt, ap);
    va_end(ap);
}

void LogInfo(const char* fmt, ...) {
    va_list ap; va_start(ap, fmt);
    LogV(false, "INFO", fmt, ap);
    va_end(ap);
}

void LogWarn(const char* fmt, ...) {
    va_list ap; va_start(ap, fmt);
    LogV(false, "WARN", fmt, ap);
    va_end(ap);
}

void LogError(const char* fmt, ...) {
    va_list ap; va_start(ap, fmt);
    LogV(false, "ERRO", fmt, ap);
    va_end(ap);
}

// 默认不输出（见 log.h 的 LogSetDebug）
void LogDebug(const char* fmt, ...) {
    va_list ap; va_start(ap, fmt);
    LogV(true, "DEBG", fmt, ap);
    va_end(ap);
}
