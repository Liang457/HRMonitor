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
#include <windows.h>
#include <cstdio>
#include <cstdlib>   // _countof
#include <cstring>
#include <cwchar>

#include "../common/hr_shared.h"
#include "../common/hr_config.h"
#include "PluginInterface.h"

namespace {

// 打开映射失败后的重试间隔：DataRequired 大约 1Hz 被调用，这里再兜一层限流。
constexpr ULONGLONG kOpenRetryMs = 1000;

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

// 定长缓冲拷贝，永不分配、永不抛异常，保证返回给主程序的指针始终有效。
void CopyBuf(wchar_t* dst, size_t dst_count, const wchar_t* src)
{
    if (dst == nullptr || dst_count == 0) return;
    if (src == nullptr) { dst[0] = L'\0'; return; }
    size_t i = 0;
    for (; i + 1 < dst_count && src[i] != L'\0'; ++i) dst[i] = src[i];
    dst[i] = L'\0';
}

} // namespace

// ------------------------------------------------------------------ 显示项目

class HrItem : public IPluginItem
{
public:
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
    const wchar_t* GetItemValueText() const override { return m_value; }

    const wchar_t* GetItemValueSampleText() const override
    {
        return (m_cfg && !m_cfg->sample.empty()) ? m_cfg->sample.c_str() : L"128";
    }

    bool IsCustomDraw() const override { return false; }

    // 仅允许 DataRequired() 调用。bpm < 0 表示应显示 "--"。
    void SetBpm(LONG bpm) noexcept
    {
        wchar_t tmp[16];
        if (bpm >= 0)
            swprintf_s(tmp, L"%ld", bpm);
        else
            wcscpy_s(tmp, L"--");
        CopyBuf(m_value, _countof(m_value), tmp);
    }

private:
    wchar_t m_value[16] = L"--";
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
            SetNoData(L"心率: 读取失败");
        }
    }

    const wchar_t* GetInfo(PluginInfoIndex index) override
    {
        switch (index) {
        case TMI_NAME:        return L"心率监控";
        case TMI_DESCRIPTION: return L"通过蓝牙接收华为手表心率广播，显示实时心率";
        // TODO(发布前): 换成你自己的名字/仓库地址
        case TMI_AUTHOR:      return L"<your name>";
        case TMI_COPYRIGHT:   return L"MIT License";
        case TMI_VERSION:     return L"1.0.0";
        case TMI_URL:         return L"<your repo url>";
        default:              return L"";
        }
    }

    const wchar_t* GetTooltipInfo() override { return m_tooltip; }

private:
    void SetNoData(const wchar_t* tip)
    {
        m_item.SetBpm(-1);
        CopyBuf(m_tooltip, _countof(m_tooltip), tip);
    }

    void Refresh()
    {
        if (!m_reader.IsOpen()) {
            const ULONGLONG now = GetTickCount64();
            if (m_last_open_try != 0 && now - m_last_open_try < kOpenRetryMs)
                return;                       // 低频重试；保持上一次的显示状态
            m_last_open_try = now;
            if (!m_reader.Open()) {
                SetNoData(L"心率: 未连接");
                return;
            }
        }

        HrSharedData d{};
        if (!m_reader.Read(d)) {
            // 映射已经不存在（daemon 退出/重启），关掉句柄，下次重新打开。
            m_reader.Close();
            m_last_open_try = 0;
            SetNoData(L"心率: 未连接");
            return;
        }

        // timeout_ms 是「daemon 挂掉」的兜底（daemon 活着时它自己会判定超时）
        const LONG bpm = HrEffectiveBpm(d, GetTickCount64(),
                                        (ULONGLONG)m_cfg.timeout_ms);
        if (bpm >= 0) {
            m_item.SetBpm(bpm);

            wchar_t dev[64];
            CopyDeviceName(d.device_name, dev, _countof(dev));

            wchar_t tip[128];
            if (dev[0] != L'\0')
                swprintf_s(tip, L"HR %ld bpm - %s", bpm, dev);
            else
                swprintf_s(tip, L"HR %ld bpm", bpm);
            CopyBuf(m_tooltip, _countof(m_tooltip), tip);
            return;
        }

        m_item.SetBpm(-1);
        switch (d.status) {
        case HRS_CONNECTING: CopyBuf(m_tooltip, _countof(m_tooltip), L"心率: 连接中..."); break;
        case HRS_TIMEOUT:    CopyBuf(m_tooltip, _countof(m_tooltip), L"心率: 已超时");     break;
        default:             CopyBuf(m_tooltip, _countof(m_tooltip), L"心率: 无数据");     break;
        }
    }

    HrPluginConfig m_cfg;                     // 来自 hr_plugin.ini
    HrItem         m_item;
    HrSharedReader m_reader;                  // 懒打开后长期持有
    ULONGLONG      m_last_open_try = 0;       // 0 表示还没试过
    wchar_t        m_tooltip[128]  = L"心率: 未连接";
};

// ------------------------------------------------------------------ 导出

extern "C" __declspec(dllexport) ITMPlugin* TMPluginGetInstance()
{
    // 进程生命周期内一直存活，永不释放。
    static HrPlugin instance;
    return &instance;
}
