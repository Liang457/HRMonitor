// daemon/log.h — 极简日志：文件 + （从 cmd 启动时）附加到父进程控制台
#pragma once

#include <string>

// exeDir: 日志目录（一般取 exe 所在目录）。失败会自动退化到 %LOCALAPPDATA%。
// maxKb : 单个日志文件的字节上限（0 = 用默认 4096）。写满就轮转（见下）。
// rotate: true = 这次启动开新的一份（daemon 正常跑）——
//         上一份顶成 hr-daemon.1.log、再上一份顶成 .2，更老的删掉，
//         所以磁盘上最多留 3 份。写超 maxKb 时也会就地轮转，总量照样封顶。
//         false = 只追加（--scan 那种一次性进程用），不轮转、不动历史。
// 返回日志文件的完整路径；空表示完全无法落盘（此时仍有控制台输出）。
std::wstring LogInit(const std::wstring& exeDir, int maxKb = 4096, bool rotate = true);

// 关闭文件句柄。正常退出路径上调用，异常退出靠 OS 回收。
void LogShutdown();

// DEBUG 级日志的开关。默认关：像"每条心率都写一行"这种会很快把日志刷爆，
// 需要排查时才打开（hr-daemon.ini 的 log.debug，或命令行 --debug）。
void LogSetDebug(bool on);

// 宽字符串 → UTF-8。日志与控制台都按 UTF-8 处理。
std::string ToUtf8(const std::wstring& w);

// printf 风格，UTF-8 源码，输出到日志文件与控制台。
void LogF(const char* fmt, ...);

// 便捷包装
void LogInfo(const char* fmt, ...);
void LogWarn(const char* fmt, ...);
void LogError(const char* fmt, ...);
void LogDebug(const char* fmt, ...);
