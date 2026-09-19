// common/hr_names.h — 跨进程、跨语言共享的标识符常量。
//
// daemon 在这里定义；hr-manager 的 Rust 侧在 tools/hr-manager/src/names.rs
// 保有同名副本（Rust 没法直接 include 这个头文件）。
// 改任何一个名字都必须两边同步改，否则 manager 的探活/优雅停止全部失效。
#pragma once

// hr-daemon 的单实例互斥体：整个生命周期都握着，manager 靠 OpenMutexW 探活
#define HR_MUTEX_DAEMON L"Local\\BleHR_daemon"

// hr-daemon 的隐藏窗口类名：manager 靠 FindWindowW 找到它再发 WM_CLOSE，
// 让 daemon 走正常退出路径（关共享内存映射、收 BLE 线程）
#define HR_WNDCLASS_DAEMON L"BleHRDaemonWnd"
