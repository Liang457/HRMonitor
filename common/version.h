// common/version.h — 产品版本号的 C++ 侧唯一定义。
//
// 与 tools/hr-manager/Cargo.toml 的 [package] version 构成跨语言双副本
// （约定同 hr_names.h：Rust 没法 include 这个头文件）。发版改版本号必须
// 两边同步改，scripts\pack.ps1 打包时会校验两处一致，不一致直接报错。
//
// 注意区分：共享内存的记录布局版本是 hr_shared.h 里的 HRSM_VERSION（协议
// 版本，只在改 64 字节布局时才动），与本文件的产品版本号无关。
#ifndef HR_VERSION_H_INCLUDED
#define HR_VERSION_H_INCLUDED

#define HR_VERSION_MAJOR 1
#define HR_VERSION_MINOR 2
#define HR_VERSION_PATCH 0

// 下面两行必须同值：窄/宽字符版本串。宽字符版给 tm-plugin 的
// TMI_VERSION（宿主要 wchar_t*）用，窄字符版给 .rc 和日志用。
#define HR_VERSION_STRING  "1.2.0"
#define HR_VERSION_WSTRING L"1.2.0"

#endif // HR_VERSION_H_INCLUDED
