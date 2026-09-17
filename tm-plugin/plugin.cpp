// tm-plugin/plugin.cpp
// TrafficMonitor x64 插件（导出 TMPluginGetInstance）
// 读取 daemon 写入的命名共享内存 Local\HuaWeiHR_SM，作为一个显示项目 "心率"。
//
// 设计要点：
//   * GetItemValueText() 被主程序以极高频率调用，因此它只返回 DataRequired()
//     缓存好的字符串，绝不在这里碰共享内存。
//   * 所有返回给主程序的字符串都是本对象持有的定长缓冲区，不返回栈上的临时量。
//   * 所有可能抛异常的路径都在 DataRequired() 里被 try/catch(...) 兜住，
//     不允许任何异常穿过 DLL 边界。
//   * 宿主不止一个窗口线程（主窗口 / 任务栏各自带 timer）都可能调
//     DataRequired()：Refresh 全程持独占 SRWLOCK。以前"Read 失败就 Close 重开"
//     在多线程下会 unmap 掉另一个线程正在读的视图，直接崩掉 TrafficMonitor；
//     现在读失败只换提示文案、根本不再 Close（进程退出由析构统一收）。
//     返回给宿主的字符串走三缓冲原子发布，读端拿到的是一份此后不变的文本，
//     不会再看到 "--8" 之类的撕裂串。
#include <windows.h>
#include <cstdio>
#include <cstdlib>   // _countof
#include <cstring>
#include <cwchar>
#include <atomic>

#include "../common/hr_shared.h"
#include "../common/hr_config.h"

#pragma warning(push, 0)   // 第三方头（Anti-996 许可，见 THIRD_PARTY_NOTICES.md），不受 /W4 管
#include "PluginInterface.h"
#pragma warning(pop)

namespace {

// 打开映射失败后的重试间隔：DataRequired 大约 1Hz 被调用，这里再兜一层限流。
constexpr ULONGLONG kOpenRetryMs = 1000;

// 三缓冲原子发布的定长字符串。
// 写侧（Refresh，持独占锁）填 (cur+1)%3 那一格，填好才原子翻转下标；
// 读侧原子取下标，拿到的是一份**此后至少两轮翻转内不会被写**的字符串——
// 宿主取到指针后是立刻拷去绘制的，够用。单写者 + 持锁，两个原子序号操作
// 就是全部同步开销。
template <size_t N>
struct TextCell {
    std::atomic<int> cur{ 0 };
    wchar_t buf[3][N] = {};

    const wchar_t* get() const
    {
        return buf[cur.load(std::memory_order_acquire) % 3];
    }

    void set(const wchar_t* s)   // 仅写侧（持锁）调用
    {
        const int next = (cur.load(std::memory_order_relaxed) + 1) % 3;
        wchar_t* dst = buf[next];
        size_t i = 0;
        for (; i + 1 < N && s[i] != L'\0'; ++i) dst[i] = s[i];
        dst[i] = L'\0';
        cur.store(next, std::memory_order_release);
    }
};

// 把共享内存里的设备名（约定 UTF-8）转成宽字符串。失败则留空。
void CopyDeviceName(const char (&raw)[36], wchar_t* out, size_t out_count)
{
    if (out == nullptr || out_count == 0) return;
    out[0] = L'\0';

    char tmp[sizeof(raw)];
    memcpy(tmp, raw, sizeof(raw));
    tmp[sizeof(raw) - 1] = '\0';   // 防止对端忘了写 NUL
    if (tmp[0] == '\0') return;

    if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, tmp, -1,
                            out, static_cast<int>(out_count)) <= 0) {
        if (MultiByteToWideChar(CP_ACP, 0, tmp, -1,
                                out, static_cast<int>(out_count)) <= 0) {
            out[0] = L'\0';
        }
    }
}

} // namespace

// ------------------------------------------------------------------ 显示项目

class HrItem : public IPluginItem
{
public:
    HrItem()
    {
        wcscpy_s(m_value.buf[0], _countof(m_value.buf[0]), L"--");
    }

    // 显示文本来自 hr_plugin.ini（插件 DLL 同目录），由 HrPlugin 在构造时读好。
    void SetConfig(const HrPluginConfig* cfg) noexcept { m_cfg = cfg; }

    const wchar_t* GetItemName() const override
    {
        return (m_cfg && !m_cfg->name.empty()) ? m_cfg->name.c_str() : L"心率";
    }

    const wchar_t* GetItemId() const override { return L"hr"; }

    const wchar_t* GetItemLableText() const override
    {
        return (m_cfg && !m_cfg->label.empty()) ? m_cfg->label.c_str() : L"HR";
    }

    // 只做格式化：直接返回 DataRequired() 缓存好的缓冲区。
    const wchar_t* GetItemValueText() const override { return m_value.get(); }

    const wchar_t* GetItemValueSampleText() const override
    {
        return (m_cfg && !m_cfg->sample.empty()) ? m_cfg->sample.c_str() : L"128";
    }

    bool IsCustomDraw() const override { return false; }

    // 仅 Refresh()（持锁）调用。bpm < 0 表示应显示 "--"。
    void SetBpm(LONG bpm) noexcept
    {
        wchar_t tmp[16];
        if (bpm >= 0)
            swprintf_s(tmp, L"%ld", bpm);
        else
            wcscpy_s(tmp, L"--");
        m_value.set(tmp);
    }

private:
    TextCell<16>   m_value;
    const HrPluginConfig* m_cfg = nullptr;
};

// ------------------------------------------------------------------ 插件本体

class HrPlugin : public ITMPlugin
{
public:
    HrPlugin()
    {
        // hr_plugin.ini 在插件 DLL 同目录（即 TrafficMonitor 的 plugins\）。
        // 文件不存在就用默认值。改完需要重启 TrafficMonitor。
        try {
            m_cfg = HrPluginConfig::Load(HrPluginConfig::DefaultIniPath());
        } catch (...) {
            // 保持默认值
        }
        m_item.SetConfig(&m_cfg);
        wcscpy_s(m_tooltip.buf[0], _countof(m_tooltip.buf[0]), L"心率: 未连接");
    }

    IPluginItem* GetItem(int index) override
    {
        return index == 0 ? static_cast<IPluginItem*>(&m_item) : nullptr;
    }

    // 唯一读取共享内存的地方。
    void DataRequired() override
    {
        try {
            Refresh();
        } catch (...) {
            // 兜底：任何意外都不能越过 DLL 边界。
            AcquireSRWLockExclusive(&m_lock);
            SetNoData(L"心率: 读取失败");
            ReleaseSRWLockExclusive(&m_lock);
        }
    }

    const wchar_t* GetInfo(PluginInfoIndex index) override
    {
        switch (index) {
        case TMI_NAME:        return L"心率监控";
        case TMI_DESCRIPTION: return L"通过蓝牙接收华为手表心率广播，显示实时心率";
        case TMI_AUTHOR:      return L"Cool-GK";
        case TMI_COPYRIGHT:   return L"MIT License";
        case TMI_VERSION:     return L"1.0.0";
        case TMI_URL:         return L"https://github.com/Liang457/HRMonitor";
        default:              return L"";
        }
    }

    const wchar_t* GetTooltipInfo() override { return m_tooltip.get(); }

private:
    // Refresh 全程独占锁（DataRequired 可能来自宿主的多个窗口线程）。
    void Refresh()
    {
        AcquireSRWLockExclusive(&m_lock);

        if (!m_reader.IsOpen()) {
            const ULONGLONG now = GetTickCount64();
            if (m_last_open_try != 0 && now - m_last_open_try < kOpenRetryMs) {
                ReleaseSRWLockExclusive(&m_lock);
                return;                       // 低频重试；保持上一次的显示状态
            }
            m_last_open_try = now;
            if (!m_reader.Open()) {
                SetNoData(L"心率: 未连接");
                ReleaseSRWLockExclusive(&m_lock);
                return;
            }
        }

        HrSharedData d{};
        HrSharedReader::ReadError err = HrSharedReader::ReadError::None;
        if (!m_reader.Read(d, &err)) {
            // 读失败只有 magic/version 两种可能（映射句柄我们一直握着，不会消失）。
            // 以前这里 Close 重开：单线程没问题，多线程下会 unmap 掉别的线程
            // 正在读的视图。现在不 Close，只把原因说清楚。
            if (err == HrSharedReader::ReadError::BadVersion)
                SetNoData(L"心率: 版本不匹配（daemon 和插件要一起更新）");
            else
                SetNoData(L"心率: 数据异常");
            ReleaseSRWLockExclusive(&m_lock);
            return;
        }

        // timeout_ms 是「daemon 挂掉」的兜底（daemon 活着时它自己会判定超时）
        const ULONGLONG now = GetTickCount64();
        const LONG bpm = HrEffectiveBpm(d, now, (ULONGLONG)m_cfg.timeout_ms);
        if (bpm >= 0) {
            m_item.SetBpm(bpm);

            wchar_t dev[64];
            CopyDeviceName(d.device_name, dev, _countof(dev));

            wchar_t tip[128];
            if (dev[0] != L'\0')
                swprintf_s(tip, L"HR %ld bpm - %s", bpm, dev);
            else
                swprintf_s(tip, L"HR %ld bpm", bpm);
            m_tooltip.set(tip);
            ReleaseSRWLockExclusive(&m_lock);
            return;
        }

        m_item.SetBpm(-1);
        // daemon 异常退出时 status 冻结在 HRS_OK：明明 tick 已经超时，
        // 还按 status 说"无数据"会误导排查。
        if (d.status != HRS_TIMEOUT &&
            now > d.tick_ms && now - d.tick_ms > (ULONGLONG)m_cfg.timeout_ms) {
            m_tooltip.set(L"心率: 已超时（hr-daemon 没在写数据？）");
        } else {
            switch (d.status) {
            case HRS_CONNECTING: m_tooltip.set(L"心率: 连接中..."); break;
            case HRS_TIMEOUT:    m_tooltip.set(L"心率: 已超时");     break;
            default:             m_tooltip.set(L"心率: 无数据");     break;
            }
        }
        ReleaseSRWLockExclusive(&m_lock);
    }

    void SetNoData(const wchar_t* tip)   // 仅持锁时调用
    {
        m_item.SetBpm(-1);
        m_tooltip.set(tip);
    }

    SRWLOCK         m_lock = SRWLOCK_INIT;
    HrPluginConfig  m_cfg;                     // 来自 hr_plugin.ini
    HrItem          m_item;
    HrSharedReader  m_reader;                  // 懒打开后长期持有（不再中途 Close）
    ULONGLONG       m_last_open_try = 0;       // 0 表示还没试过
    TextCell<128>   m_tooltip;
};

// ------------------------------------------------------------------ 导出

extern "C" __declspec(dllexport) ITMPlugin* TMPluginGetInstance()
{
    // 进程生命周期内一直存活，永不释放。
    static HrPlugin instance;
    return &instance;
}
