// common/hr_config.h
// 配置与公共工具：hr-daemon.exe / hr-manager.exe / hr_plugin.dll 共用。
//
// 配置分两份：
//   hr-daemon.ini  与 hr-daemon.exe（和 hr-manager.exe）同目录，管采集/显示/日志
//   hr_plugin.ini  与 hr_plugin.dll 同目录（即 TrafficMonitor 的 plugins\ 下），
//                  只放插件自己的显示文本。hr-manager.exe 会一并写它。
//
// OSD 的外观（颜色/字号/量程/是否显示）**不在这里**：那归 MSI Afterburner
// 的监控设置管，见 README 与 ab-plugin/。
//
// INI 一律 UTF-8（带 BOM），方便手改和 git diff。
#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <string>
#include <vector>

// ============================================================ 路径工具

// 当前进程 exe 所在目录（不带尾部反斜杠）。失败返回 "."。
std::wstring HrExeDir();

// 拼接路径，自动补分隔符。
std::wstring HrJoinPath(const std::wstring& dir, const std::wstring& name);

// <HrExeDir()>\hr-daemon.ini
std::wstring HrDaemonIniPath();

// MAC 地址 "AA:BB:CC:DD:EE:FF" / "AA-BB-..." → 48 位整数（高字节是第一个八位组）
bool HrParseMac(const std::wstring& s, unsigned long long& out);
std::wstring HrFormatMac(unsigned long long addr);

// ============================================================ 极简 INI

// 够用就好的 INI：UTF-8(BOM 可选)，[section]，key=value，; 或 # 注释。
// 读是宽容的（缺项用默认值）；写一律重新生成整个文件（带注释模板），
// 所以手工加进去的未知键在保存时不会被保留。
class HrIni {
public:
    // 文件不存在 → 视为空，返回 true。"存在但读不了"（被占用/编码损坏）也返回
    // true（行为不变：全用默认值），但 *fileError 会被置 true，让上层有机会提示。
    bool Load(const std::wstring& path, bool* fileError = nullptr);
    bool Save(const std::wstring& path, const std::string& utf8Text,
              std::wstring* err = nullptr) const;

    bool        Has(const wchar_t* sec, const wchar_t* key) const;
    std::wstring GetStr(const wchar_t* sec, const wchar_t* key, const wchar_t* def) const;
    int         GetInt(const wchar_t* sec, const wchar_t* key, int def) const;
    float       GetFloat(const wchar_t* sec, const wchar_t* key, float def) const;
    bool        GetBool(const wchar_t* sec, const wchar_t* key, bool def) const;

    // 记录从文件里读到的、但不在已知键列表里的 [section] key，供上层提示。
    const std::vector<std::wstring>& UnknownKeys() const { return m_unknown; }

private:
    std::vector<std::pair<std::wstring, std::wstring>> m_kv;   // "sec\0key" -> value
    std::vector<std::wstring> m_unknown;
};

// 读 UTF-8 文件（自动跳过 BOM）→ UTF-16。失败返回 false。
// exists 非空时区分"文件不存在"（*exists=false）和"存在但读不了"（*exists=true）
// —— 把文件被编辑器锁住和文件没建过混为一谈，是以前覆盖事故的根源。
bool HrReadTextFileUtf8(const std::wstring& path, std::wstring& out,
                        bool* exists = nullptr);

// ============================================================ daemon 配置

struct HrConfig {
    // ---- [source] 心率来源
    bool         demo            = false;    // 模拟心率源（不要手表也能跑）
    std::wstring address;                    // 空 = 扫描；否则 "AA:BB:CC:DD:EE:FF"
    int          scan_timeout_ms = 20000;    // 每轮扫描最长时长
    int          backoff_min_sec = 1;        // 重连退避下限
    int          backoff_max_sec = 30;       // 重连退避上限

    // ---- [display] 显示时机
    int timeout_ms = 15000;                  // 数据超过这么久没更新 → 显示 "--"
    int refresh_ms = 1000;                   // 采样 / 推送周期

    // ---- [log] 日志
    int  log_max_kb = 4096;                  // 单个日志文件的上限，写满就轮转
    bool log_debug  = false;                 // 连每条心率都写进日志（排查用）

    // ---- [integration] 只给 hr-manager.exe 用
    std::wstring tm_dir;                     // TrafficMonitor 安装目录

    // 文件缺失/读失败 → 返回一份默认配置（不报错）。
    // notes 非空时，把被钳位的配置项记进去 —— 必须在 Load 里做，因为 Load 内部
    // 已经 Sanitize 过一次，拿返回值再 Sanitize 是拿不到任何提示的。
    static HrConfig Load(const std::wstring& iniPath,
                         std::vector<std::wstring>* notes = nullptr);
    static HrConfig LoadDefault();

    // 重新生成整份带注释的 INI 文本。
    std::string ToIniText() const;
    bool Save(const std::wstring& iniPath, std::wstring* err = nullptr) const;

    // 越界值拉回合法范围。notes 非空时，把每一项改动记进去（给 GUI 提示用）。
    void Sanitize(std::vector<std::wstring>* notes = nullptr);
};

// ============================================================ 插件配置

// hr_plugin.ini —— 与插件 DLL 同目录
struct HrPluginConfig {
    std::wstring label  = L"HR";      // 数值前面的标签
    std::wstring name   = L"心率";     // 在 TrafficMonitor"显示设置"里的名字
    std::wstring sample = L"128";     // 示例值（决定显示区宽度）
    // daemon 挂掉时插件自己的兜底超时（daemon 活着时由 daemon 判定超时）
    int          timeout_ms = 15000;

    static HrPluginConfig Load(const std::wstring& iniPath);
    // 与插件 DLL 同目录的 hr_plugin.ini
    static std::wstring DefaultIniPath();
    std::string ToIniText() const;
    bool Save(const std::wstring& iniPath, std::wstring* err = nullptr) const;
    void Sanitize(std::vector<std::wstring>* notes = nullptr);
};
