// common/hr_shared.h
// 华为手表心率 → 中立共享内存 HuaWeiHR_SM
// daemon（写）与两个显示端插件（读）共用的唯一定义来源：
//   hr_plugin.dll  → TrafficMonitor 任务栏
//   HeartRate.dll  → MSI Afterburner 监控数据源 / OSD
//
// 威胁模型（知情接受）：映射与互斥体都在 Local\ 会话命名空间里，用默认 DACL。
// 同一登录会话的进程本来就能互相冒充（同用户），所以没有再收紧 ACL——收不收
// 紧，同用户的恶意进程都拦不住；跨会话则被 Local\ 天然隔开。读端校验的
// magic/version 只防"版本错配"和"意外垃圾"，不防伪造。真被同会话进程篡改，
// 后果只是显示错的心率值，不涉及内存安全（读端从不把数据当代码/指针解析）。
#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

// 64 字节定长记录写入名为 Local\HuaWeiHR_SM 的共享内存。
#define HRSM_MAGIC      0x4D535248u   // 'HRSM'
#define HRSM_VERSION    1u
#define HRSM_SIZE       64u
#define HRSM_NAME       L"Local\\HuaWeiHR_SM"

// 数据超过这个时间没有更新就视为超时（显示 --）。
#define HRSM_TIMEOUT_MS 15000ull

enum HrStatus : DWORD {
    HRS_NODATA     = 0,   // 还没拿到过数据
    HRS_OK         = 1,   // 正常
    HRS_CONNECTING = 2,   // 正在扫描/连接/重连
    HRS_TIMEOUT    = 3,   // 曾经有数据，但已超时
};

// pack(4)：daemon 是 x64、Afterburner 插件是 x86，但显式固定布局避免任何编译器差异。
#pragma pack(push, 4)
struct HrSharedData {
    DWORD     magic;              // HRSM_MAGIC
    DWORD     version;            // HRSM_VERSION
    LONG      bpm;                // -1 = 无效
    ULONGLONG tick_ms;            // GetTickCount64() 采样时刻
    DWORD     status;             // HrStatus
    DWORD     battery;            // 保留（未来读 0x180F）
    char      device_name[36];    // 手表名，NUL 结尾
};
#pragma pack(pop)

// 4+4+4+8+4+4+36 = 64
static_assert(sizeof(HrSharedData) == HRSM_SIZE, "HrSharedData 必须是 64 字节");

// ---------------------------------------------------------------- 写入端（daemon）

class HrSharedWriter {
public:
    ~HrSharedWriter() { Close(); }

    HrSharedWriter() = default;
    HrSharedWriter(const HrSharedWriter&) = delete;
    HrSharedWriter& operator=(const HrSharedWriter&) = delete;

    // 创建（或打开已存在的）共享内存。失败返回 false，可稍后重试。
    bool Open() {
        if (m_view) return true;
        if (!m_map) {
            m_map = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE,
                                       0, HRSM_SIZE, HRSM_NAME);
            if (!m_map) return false;
        }
        m_view = (HrSharedData*)MapViewOfFile(m_map, FILE_MAP_WRITE, 0, 0, HRSM_SIZE);
        if (!m_view) {
            CloseHandle(m_map);
            m_map = nullptr;
            return false;
        }
        return true;
    }

    void Close() {
        if (m_view) { UnmapViewOfFile(m_view); m_view = nullptr; }
        if (m_map)  { CloseHandle(m_map);       m_map  = nullptr; }
    }

    bool IsOpen() const { return m_view != nullptr; }

    // 写入一条记录。
    //
    // 注意这里**不能**保证读者看不到"半条"记录：memcpy 是按地址递增写的，
    // 并发读者可能看到新的 magic 配旧的 bpm/status/device_name；tick_ms 是
    // 8 字节而 pack(4) 只给了 4 字节对齐，32 位宿主上的 qword 读会拆成两次
    // 32 位读，理论上也会撕裂。
    //
    // 之所以可以接受：没有校验和也没有序号，最坏结果是读者多显示一次旧值、
    // 或者多显示一次 "--"，而 daemon 每个 refresh_ms（默认 1 秒）就整条重写一次，
    // 下一拍必然自愈。真要严格一致就得上 seqlock，那要动布局和 HRSM_VERSION。
    void Write(const HrSharedData& d) {
        if (!m_view) return;
        memcpy(m_view, &d, sizeof(HrSharedData));
    }

private:
    HANDLE        m_map  = nullptr;
    HrSharedData* m_view = nullptr;
};

// ---------------------------------------------------------------- 读取端（插件）

class HrSharedReader {
public:
    ~HrSharedReader() { Close(); }

    HrSharedReader() = default;
    HrSharedReader(const HrSharedReader&) = delete;
    HrSharedReader& operator=(const HrSharedReader&) = delete;

    // 打开映射一次即可长期持有；daemon 退出后映射依然有效（内容是最后一帧）。
    bool Open() {
        if (m_view) return true;
        if (!m_map) {
            m_map = OpenFileMappingW(FILE_MAP_READ, FALSE, HRSM_NAME);
            if (!m_map) return false;
        }
        m_view = (const HrSharedData*)MapViewOfFile(m_map, FILE_MAP_READ, 0, 0, HRSM_SIZE);
        if (!m_view) {
            CloseHandle(m_map);
            m_map = nullptr;
            return false;
        }
        return true;
    }

    void Close() {
        if (m_view) { UnmapViewOfFile((LPCVOID)m_view); m_view = nullptr; }
        if (m_map)  { CloseHandle(m_map);               m_map  = nullptr; }
    }

    bool IsOpen() const { return m_view != nullptr; }

    // Read 的失败原因：给调用方区分"没装/没起 daemon"和"新版 daemon 配旧插件"。
    enum class ReadError {
        None,        // 读到了
        NoMap,       // 映射不存在（daemon 没在跑，或还没写过）
        BadMagic,    // magic 不符（不是我们的映射/垃圾数据）
        BadVersion,  // version 不认识（daemon 和插件版本错配，要一起换）
    };

    // 读一条记录。返回 false 时 err（若非空）说明原因。
    // version 一定要查：换了布局的 daemon 配上旧插件时，不查就会把新字段当旧字段读。
    bool Read(HrSharedData& out, ReadError* err = nullptr) const {
        if (err) *err = ReadError::None;
        if (!m_view) {
            if (err) *err = ReadError::NoMap;
            return false;
        }
        HrSharedData tmp;
        memcpy(&tmp, m_view, sizeof(HrSharedData));
        if (tmp.magic != HRSM_MAGIC) {
            if (err) *err = ReadError::BadMagic;
            return false;
        }
        if (tmp.version != HRSM_VERSION) {
            if (err) *err = ReadError::BadVersion;
            return false;
        }
        out = tmp;
        return true;
    }

private:
    HANDLE          m_map  = nullptr;
    const HrSharedData* m_view = nullptr;
};

// ---------------------------------------------------------------- 共用工具

// 根据记录判断当前对外的 bpm：-1 表示应显示 "--"。
// timeout_ms 用 GetTickCount64 与记录里的 tick_ms 比较。
// bpm > 300 也不认：写端 memcpy 无锁（见上），撕裂/脏帧的脏值落在 bpm 上时
// 显示一个 65536 bpm 的毛刺毫无意义，一拍就自愈的代价不值得付。
inline LONG HrEffectiveBpm(const HrSharedData& d, ULONGLONG now_ms,
                           ULONGLONG timeout_ms = HRSM_TIMEOUT_MS) {
    if (d.status == HRS_TIMEOUT) return -1;
    if (d.bpm < 0 || d.bpm > 300) return -1;
    if (now_ms > d.tick_ms && now_ms - d.tick_ms > timeout_ms) return -1;
    return d.bpm;
}
