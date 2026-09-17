// common/hr_config.cpp
#include "hr_config.h"

#include <cerrno>
#include <climits>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cwctype>

namespace {

constexpr DWORD kUtf8Bom = 0xBFBBEFu;

std::wstring Trim(const std::wstring& s) {
    size_t a = 0, b = s.size();
    while (a < b && (s[a] == L' ' || s[a] == L'\t' || s[a] == L'\r' || s[a] == L'\n')) ++a;
    while (b > a && (s[b - 1] == L' ' || s[b - 1] == L'\t' || s[b - 1] == L'\r' || s[b - 1] == L'\n')) --b;
    return s.substr(a, b - a);
}

std::string ToUtf8(const std::wstring& w) {
    if (w.empty()) return std::string();
    int n = WideCharToMultiByte(CP_UTF8, 0, w.c_str(), (int)w.size(), nullptr, 0, nullptr, nullptr);
    if (n <= 0) return std::string();
    std::string s((size_t)n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, w.c_str(), (int)w.size(), s.data(), n, nullptr, nullptr);
    return s;
}

std::wstring FromUtf8(const std::string& s) {
    if (s.empty()) return std::wstring();
    int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), nullptr, 0);
    if (n <= 0) return std::wstring();
    std::wstring w((size_t)n, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), w.data(), n);
    return w;
}

// INI 的 key 是 "section\0key" 拼成的宽字符串
std::wstring MakeKey(const wchar_t* sec, const wchar_t* key) {
    std::wstring k(sec);
    k.push_back(L'\0');
    k += key;
    return k;
}

int ClampInt(int v, int lo, int hi) { return v < lo ? lo : (v > hi ? hi : v); }

// 去掉控制字符和换行，避免把显示文本搞坏
std::wstring SanitizeText(const std::wstring& s, size_t maxLen) {
    std::wstring out;
    for (wchar_t c : s) {
        if (c == L'\r' || c == L'\n' || c == L'\t' || c < 0x20) continue;
        out.push_back(c);
        if (out.size() >= maxLen) break;
    }
    return out;
}

// UTF-16 → UTF-16 宽字符（按字节组装，避免把未对齐的缓冲指针当 wchar_t* 解引用）。
// 奇数尾字节丢弃（文件被截断的残迹）。
bool DecodeUtf16(const std::string& bytes, size_t offset, bool littleEndian, std::wstring& out) {
    const size_t count = (bytes.size() > offset) ? (bytes.size() - offset) / 2 : 0;
    out.clear();
    out.reserve(count);
    for (size_t i = 0; i < count; ++i) {
        const unsigned char b0 = (unsigned char)bytes[offset + i * 2];
        const unsigned char b1 = (unsigned char)bytes[offset + i * 2 + 1];
        out.push_back(littleEndian ? (wchar_t)(b0 | (b1 << 8)) : (wchar_t)((b0 << 8) | b1));
    }
    return true;
}

} // namespace

// ============================================================ 路径工具

std::wstring HrExeDir() {
    wchar_t buf[MAX_PATH * 2] = {};
    DWORD n = GetModuleFileNameW(nullptr, buf, (DWORD)(sizeof(buf) / sizeof(buf[0])));
    if (n == 0) return L".";
    std::wstring p(buf, n);
    const size_t k = p.find_last_of(L"\\/");
    return (k == std::wstring::npos) ? std::wstring(L".") : p.substr(0, k);
}

std::wstring HrJoinPath(const std::wstring& dir, const std::wstring& name) {
    if (dir.empty()) return name;
    std::wstring d = dir;
    if (d.back() != L'\\' && d.back() != L'/') d.push_back(L'\\');
    return d + name;
}

std::wstring HrDaemonIniPath() {
    return HrJoinPath(HrExeDir(), L"hr-daemon.ini");
}

bool HrParseMac(const std::wstring& s, unsigned long long& out) {
    // 只认"整串就是一个 MAC"：swscanf_s 只看转换成功的字段数，尾部多出来的东西
    // 它不管，于是 "AA:BB:CC:DD:EE:FF junk"、"...:FF:99" 都会被当成合法地址，
    // 然后 daemon 直接去连第一个 MAC，一声不吭。
    const std::wstring t = Trim(s);
    unsigned int b[6] = {};
    int used = 0;
    int n = swscanf_s(t.c_str(), L"%2x:%2x:%2x:%2x:%2x:%2x%n",
                      &b[0], &b[1], &b[2], &b[3], &b[4], &b[5], &used);
    if (n != 6 || used != (int)t.size())
        n = swscanf_s(t.c_str(), L"%2x-%2x-%2x-%2x-%2x-%2x%n",
                      &b[0], &b[1], &b[2], &b[3], &b[4], &b[5], &used);
    if (n != 6 || used != (int)t.size()) return false;
    out = 0;
    for (int i = 0; i < 6; ++i) out = (out << 8) | (unsigned long long)(b[i] & 0xFFu);
    return true;
}

std::wstring HrFormatMac(unsigned long long a) {
    wchar_t buf[24] = {};
    _snwprintf_s(buf, _countof(buf), _TRUNCATE, L"%02X:%02X:%02X:%02X:%02X:%02X",
                 (unsigned)((a >> 40) & 0xFF), (unsigned)((a >> 32) & 0xFF),
                 (unsigned)((a >> 24) & 0xFF), (unsigned)((a >> 16) & 0xFF),
                 (unsigned)((a >> 8) & 0xFF), (unsigned)(a & 0xFF));
    return buf;
}

// ============================================================ 极简 INI

bool HrReadTextFileUtf8(const std::wstring& path, std::wstring& out, bool* exists) {
    if (exists) *exists = false;
    HANDLE h = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                           OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        if (exists) {
            const DWORD e = GetLastError();
            *exists = (e != ERROR_FILE_NOT_FOUND && e != ERROR_PATH_NOT_FOUND);
        }
        return false;
    }

    LARGE_INTEGER sz{};
    if (!GetFileSizeEx(h, &sz) || sz.QuadPart <= 0 || sz.QuadPart > (16 << 20)) {
        CloseHandle(h);
        return false;
    }
    std::string bytes((size_t)sz.QuadPart, '\0');
    DWORD got = 0, total = 0;
    while (total < (DWORD)bytes.size()) {
        if (!ReadFile(h, bytes.data() + total, (DWORD)bytes.size() - total, &got, nullptr) || got == 0) break;
        total += got;
    }
    CloseHandle(h);
    bytes.resize(total);

    // 跳过 UTF-8 BOM；UTF-16 BOM 也兼容一下（用户可能拿记事本另存成 Unicode），
    // LE（FF FE）和 BE（FE FF）都认。
    if (bytes.size() >= 2 && (unsigned char)bytes[0] == 0xFF && (unsigned char)bytes[1] == 0xFE)
        return DecodeUtf16(bytes, 2, /*littleEndian=*/true, out);
    if (bytes.size() >= 2 && (unsigned char)bytes[0] == 0xFE && (unsigned char)bytes[1] == 0xFF)
        return DecodeUtf16(bytes, 2, /*littleEndian=*/false, out);
    size_t skip = 0;
    if (bytes.size() >= 3 && (unsigned char)bytes[0] == 0xEF && (unsigned char)bytes[1] == 0xBB &&
        (unsigned char)bytes[2] == 0xBF)
        skip = 3;
    out = FromUtf8(bytes.substr(skip));
    return true;
}

bool HrIni::Load(const std::wstring& path, bool* fileError) {
    m_kv.clear();
    m_unknown.clear();
    if (fileError) *fileError = false;

    bool exists = false;
    std::wstring text;
    if (!HrReadTextFileUtf8(path, text, &exists)) {
        // 文件不存在 = 全用默认值（常态）。"存在但读不了"也照旧用默认值——
        // 这里的所有调用方都没有能弹提示的 UI——但把这一情况报给上层，
        // 别让"被编辑器锁住"和"从没配置过"在调用方眼里一模一样。
        if (fileError) *fileError = exists;
        return true;
    }

    std::wstring sec;
    size_t pos = 0;
    while (pos <= text.size()) {
        size_t eol = text.find(L'\n', pos);
        std::wstring line = (eol == std::wstring::npos) ? text.substr(pos) : text.substr(pos, eol - pos);
        pos = (eol == std::wstring::npos) ? text.size() + 1 : eol + 1;

        line = Trim(line);
        if (line.empty() || line[0] == L';' || line[0] == L'#') continue;

        if (line.front() == L'[' && line.back() == L']') {
            sec = Trim(line.substr(1, line.size() - 2));
            continue;
        }
        const size_t eq = line.find(L'=');
        if (eq == std::wstring::npos) continue;
        const std::wstring key = Trim(line.substr(0, eq));
        if (key.empty()) continue;
        m_kv.emplace_back(MakeKey(sec.c_str(), key.c_str()), Trim(line.substr(eq + 1)));
    }
    return true;
}

bool HrIni::Save(const std::wstring& path, const std::string& utf8Text, std::wstring* err) const {
    // 写临时文件再原子替换：CREATE_ALWAYS 直接覆盖原文件的话，写到一半被杀/
    // 断电/磁盘满会留下半份 INI，而读端宽容解析——丢掉的键全静默变默认值。
    const std::wstring tmp = path + L".tmp";
    HANDLE h = CreateFileW(tmp.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                           CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        if (err) *err = L"无法写入 " + tmp + L"（错误码 " + std::to_wstring(GetLastError()) + L"）";
        return false;
    }
    // UTF-8 BOM：记事本和 VS Code 都能正确识别中文注释
    const unsigned char bom[3] = { 0xEF, 0xBB, 0xBF };
    DWORD written = 0;
    bool ok = WriteFile(h, bom, 3, &written, nullptr) &&
              WriteFile(h, utf8Text.data(), (DWORD)utf8Text.size(), &written, nullptr);
    if (ok) ok = FlushFileBuffers(h);   // 落盘再替换，断电不会留空文件
    CloseHandle(h);
    if (!ok) {
        DeleteFileW(tmp.c_str());
        if (err) *err = L"写入 " + tmp + L" 失败（磁盘满？）";
        return false;
    }
    // 旧版先挪成 .bak：替换失败还有得救
    const std::wstring bak = path + L".bak";
    DeleteFileW(bak.c_str());
    MoveFileW(path.c_str(), bak.c_str());
    if (!MoveFileExW(tmp.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        const DWORD e = GetLastError();
        MoveFileW(bak.c_str(), path.c_str());
        if (err) *err = L"替换 " + path + L" 失败（错误码 " + std::to_wstring(e) + L"）";
        return false;
    }
    return true;
}

bool HrIni::Has(const wchar_t* sec, const wchar_t* key) const {
    const std::wstring k = MakeKey(sec, key);
    for (const auto& kv : m_kv)
        if (kv.first == k) return true;
    return false;
}

std::wstring HrIni::GetStr(const wchar_t* sec, const wchar_t* key, const wchar_t* def) const {
    const std::wstring k = MakeKey(sec, key);
    for (const auto& kv : m_kv)
        if (kv.first == k) return kv.second;
    return def;
}

int HrIni::GetInt(const wchar_t* sec, const wchar_t* key, int def) const {
    const std::wstring v = GetStr(sec, key, L"");
    if (v.empty()) return def;
    // errno 和范围都要看：wcstol 溢出会静默回绕/饱和（并置 ERANGE），配上一个
    // 没被 Sanitize 覆盖的新键就会把越界值原样吃进来。long 在 Windows 上就是
    // 32 位，errno 之外不需要再有范围比较。
    errno = 0;
    wchar_t* end = nullptr;
    const long r = wcstol(v.c_str(), &end, 10);
    if (end == v.c_str() || errno == ERANGE) return def;
    while (end && *end == L' ') ++end;
    if (end && *end != L'\0') return def;          // 尾部有垃圾，不认
    return (int)r;
}

float HrIni::GetFloat(const wchar_t* sec, const wchar_t* key, float def) const {
    const std::wstring v = GetStr(sec, key, L"");
    if (v.empty()) return def;
    // 和 GetInt 同款纪律：errno、尾随垃圾都查，nan/inf 不认
    errno = 0;
    wchar_t* end = nullptr;
    const double r = wcstod(v.c_str(), &end);
    if (end == v.c_str() || errno == ERANGE) return def;
    while (end && *end == L' ') ++end;
    if (end && *end != L'\0') return def;
    if (!std::isfinite(r)) return def;
    return (float)r;
}

bool HrIni::GetBool(const wchar_t* sec, const wchar_t* key, bool def) const {
    const std::wstring v = GetStr(sec, key, L"");
    if (v.empty()) return def;
    // 大小写不敏感；不认识的值回默认值（跟 GetInt 的"解析失败回 def"一致，
    // 以前 "Yes"/"ON" 会被静默当成 false）
    std::wstring t = v;
    for (wchar_t& c : t) c = (wchar_t)towlower((wint_t)c);
    if (t == L"1" || t == L"true" || t == L"yes" || t == L"on")  return true;
    if (t == L"0" || t == L"false" || t == L"no" || t == L"off") return false;
    return def;
}

// ============================================================ daemon 配置

HrConfig HrConfig::Load(const std::wstring& iniPath, std::vector<std::wstring>* notes) {
    HrIni ini;
    bool fileError = false;
    ini.Load(iniPath, &fileError);
    if (fileError && notes) {
        notes->push_back(L"配置文件存在但读不了（被占用或编码损坏），本次使用默认值 —— "
                         L"请检查后重启，别在此时保存配置");
    }

    HrConfig c;
    c.demo            = ini.GetBool(L"source", L"demo", c.demo);
    c.address         = ini.GetStr(L"source", L"address", L"");
    c.scan_timeout_ms = ini.GetInt(L"source", L"scan_timeout_ms", c.scan_timeout_ms);
    c.backoff_min_sec = ini.GetInt(L"source", L"backoff_min_sec", c.backoff_min_sec);
    c.backoff_max_sec = ini.GetInt(L"source", L"backoff_max_sec", c.backoff_max_sec);

    c.timeout_ms = ini.GetInt(L"display", L"timeout_ms", c.timeout_ms);
    c.refresh_ms = ini.GetInt(L"display", L"refresh_ms", c.refresh_ms);

    c.log_max_kb = ini.GetInt(L"log", L"max_kb", c.log_max_kb);
    c.log_debug  = ini.GetBool(L"log", L"debug", c.log_debug);

    c.tm_dir = ini.GetStr(L"integration", L"tm_dir", L"");

    c.Sanitize(notes);
    return c;
}

HrConfig HrConfig::LoadDefault() {
    return Load(HrDaemonIniPath());
}

void HrConfig::Sanitize(std::vector<std::wstring>* notes) {
    auto note = [&](const wchar_t* what) {
        if (notes) notes->push_back(what);
    };

    if (scan_timeout_ms < 2000 || scan_timeout_ms > 600000) {
        scan_timeout_ms = ClampInt(scan_timeout_ms, 2000, 600000);
        note(L"source.scan_timeout_ms 超出 2~600 秒，已调整");
    }
    if (backoff_min_sec < 1 || backoff_min_sec > 600) {
        backoff_min_sec = ClampInt(backoff_min_sec, 1, 600);
        note(L"source.backoff_min_sec 超出 1~600 秒，已调整");
    }
    if (backoff_max_sec < backoff_min_sec || backoff_max_sec > 3600) {
        backoff_max_sec = ClampInt(backoff_max_sec, backoff_min_sec, 3600);
        note(L"source.backoff_max_sec 必须 ≥ backoff_min_sec 且 ≤ 3600 秒，已调整");
    }

    if (timeout_ms < 2000 || timeout_ms > 600000) {
        timeout_ms = ClampInt(timeout_ms, 2000, 600000);
        note(L"display.timeout_ms 超出 2~600 秒，已调整");
    }
    if (refresh_ms < 200 || refresh_ms > 60000) {
        refresh_ms = ClampInt(refresh_ms, 200, 60000);
        note(L"display.refresh_ms 超出 0.2~60 秒，已调整");
    }

    if (log_max_kb < 64 || log_max_kb > 1048576) {
        log_max_kb = ClampInt(log_max_kb, 64, 1048576);
        note(L"log.max_kb 超出 64~1048576 KB，已调整");
    }
}

std::string HrConfig::ToIniText() const {
    // 能解析就规范成 AA:BB:CC:DD:EE:FF，解析不了就原样写回去（daemon 会记警告）
    std::wstring addrOut;
    if (!address.empty()) {
        unsigned long long a = 0;
        addrOut = HrParseMac(address, a) ? HrFormatMac(a) : address;
    }

    // 格式串只写一遍：先用 _scprintf 问需要多长，再按实际长度开缓冲。
    // 定长缓冲区一旦不够就会被 _TRUNCATE 静默截断，写出一份缺尾段的 INI，
    // 而读的那边宽容解析 → 丢掉的键全变默认值，很难查。
    const char* fmt =
        "; hr-daemon.ini —— hr-daemon / hr-manager 的配置\r\n"
        "; 这个文件由 hr-manager.exe 生成，也可以手改（UTF-8）。改完重启 hr-daemon 生效。\r\n"
        "; 以 ; 或 # 开头的行是注释。\r\n"
        "; OSD 的外观（颜色/字号/量程/是否显示）不在这个文件里配 ——\r\n"
        "; 那归 MSI Afterburner 的监控设置管，见 README。\r\n"
        "\r\n"
        "[source]\r\n"
        "; demo=1 使用模拟心率源（60~180 随机游走），不需要手表\r\n"
        "demo=%d\r\n"
        "; 留空 = 不连接（避免连错设备，手表地址必须明确指定）；填了 = 直连该地址\r\n"
        "address=%s\r\n"
        "; 每轮扫描最长多少毫秒\r\n"
        "scan_timeout_ms=%d\r\n"
        "; 失败后重连退避，从 min 秒开始翻倍到 max 秒封顶\r\n"
        "backoff_min_sec=%d\r\n"
        "backoff_max_sec=%d\r\n"
        "\r\n"
        "[display]\r\n"
        "; 超过这么多毫秒没有新数据就显示 \"--\"\r\n"
        "timeout_ms=%d\r\n"
        "; 采样/推送周期（毫秒）\r\n"
        "refresh_ms=%d\r\n"
        "\r\n"
        "[log]\r\n"
        "; 单个日志文件超过这么多 KB 就轮转（磁盘上最多留 3 份，见 README）\r\n"
        "max_kb=%d\r\n"
        "; debug=1 连每条心率都写进日志（排查用；平时别开，心率几乎每秒都变，日志会涨很快）\r\n"
        "debug=%d\r\n"
        "\r\n"
        "[integration]\r\n"
        "; TrafficMonitor 安装目录（只给 hr-manager.exe 用）\r\n"
        "tm_dir=%s\r\n";

    const int need = _scprintf(fmt,
        demo ? 1 : 0,
        ToUtf8(addrOut).c_str(),
        scan_timeout_ms,
        backoff_min_sec, backoff_max_sec,
        timeout_ms, refresh_ms,
        log_max_kb,
        log_debug ? 1 : 0,
        ToUtf8(tm_dir).c_str());
    if (need < 0) return std::string();   // 格式化本身出错

    std::string out((size_t)need + 1, '\0');
    _snprintf_s(&out[0], out.size(), _TRUNCATE, fmt,
        demo ? 1 : 0,
        ToUtf8(addrOut).c_str(),
        scan_timeout_ms,
        backoff_min_sec, backoff_max_sec,
        timeout_ms, refresh_ms,
        log_max_kb,
        log_debug ? 1 : 0,
        ToUtf8(tm_dir).c_str());
    out.resize((size_t)need);
    return out;
}

bool HrConfig::Save(const std::wstring& iniPath, std::wstring* err) const {
    HrIni ini;
    return ini.Save(iniPath, ToIniText(), err);
}

// ============================================================ 插件配置

std::wstring HrPluginConfig::DefaultIniPath() {
    // 插件跑在 TrafficMonitor 进程里，所以这里拿到的是 TrafficMonitor 的目录，
    // 也就是插件 DLL 所在目录的上一级；hr_plugin.ini 放在 DLL 旁边（plugins\）。
    return HrJoinPath(HrJoinPath(HrExeDir(), L"plugins"), L"hr_plugin.ini");
}

HrPluginConfig HrPluginConfig::Load(const std::wstring& iniPath) {
    HrIni ini;
    ini.Load(iniPath);

    HrPluginConfig c;
    c.label      = ini.GetStr(L"plugin", L"label", c.label.c_str());
    c.name       = ini.GetStr(L"plugin", L"name", c.name.c_str());
    c.sample     = ini.GetStr(L"plugin", L"sample", c.sample.c_str());
    c.timeout_ms = ini.GetInt(L"plugin", L"timeout_ms", c.timeout_ms);
    c.Sanitize();
    return c;
}

void HrPluginConfig::Sanitize(std::vector<std::wstring>* notes) {
    label  = SanitizeText(label, 12);
    name   = SanitizeText(name, 24);
    sample = SanitizeText(sample, 12);
    if (label.empty())  label  = L"HR";
    if (name.empty())   name   = L"心率";
    if (sample.empty()) sample = L"128";
    if (timeout_ms < 2000 || timeout_ms > 600000) {
        timeout_ms = ClampInt(timeout_ms, 2000, 600000);
        if (notes) notes->push_back(L"plugin.timeout_ms 超出 2~600 秒，已调整");
    }
}

std::string HrPluginConfig::ToIniText() const {
    const char* fmt =
        "; hr_plugin.ini —— TrafficMonitor 插件 hr_plugin.dll 的配置\r\n"
        "; 放在 TrafficMonitor 的 plugins\\ 目录下（和 hr_plugin.dll 同级）。\r\n"
        "; 改完需要重启 TrafficMonitor 生效。\r\n"
        "\r\n"
        "[plugin]\r\n"
        "; 数值前面显示的标签\r\n"
        "label=%s\r\n"
        "; 在 TrafficMonitor\"显示设置\"列表里的名字\r\n"
        "name=%s\r\n"
        "; 示例值，决定显示区域的宽度（用最宽的情况，比如 188）\r\n"
        "sample=%s\r\n"
        "; daemon 挂掉时插件自己的兜底超时（毫秒）。daemon 活着时由 daemon 判定超时\r\n"
        "timeout_ms=%d\r\n";

    const int need = _scprintf(fmt, ToUtf8(label).c_str(), ToUtf8(name).c_str(),
                               ToUtf8(sample).c_str(), timeout_ms);
    if (need < 0) return std::string();

    std::string out((size_t)need + 1, '\0');
    _snprintf_s(&out[0], out.size(), _TRUNCATE, fmt, ToUtf8(label).c_str(),
                ToUtf8(name).c_str(), ToUtf8(sample).c_str(), timeout_ms);
    out.resize((size_t)need);
    return out;
}

bool HrPluginConfig::Save(const std::wstring& iniPath, std::wstring* err) const {
    HrIni ini;
    return ini.Save(iniPath, ToIniText(), err);
}
