// src/names.rs —— 跨进程、跨语言共享的标识符常量。
//
// 这里是 C++ 侧 common/hr_names.h 的 Rust 副本（Rust 没法直接 include 那个头）。
// 改任何一个名字都必须两边同步改，否则 daemon 的探活/优雅停止、manager 的
// 单实例判定全部失效。
//
// 共享内存名不在这里：它由 common/hr_shared.h 定义（版本化协议的一部分），
// shared_mem.rs 按字节布局读，名字也抄了同一份。

/// hr-daemon 的单实例互斥体：整个生命周期都握着，OpenMutexW 能打开 = 还在跑
pub const DAEMON_MUTEX: &str = "Local\\BleHR_daemon";

/// hr-daemon 的隐藏窗口类名：FindWindowW 找到它再发 WM_CLOSE 走优雅退出
pub const DAEMON_WNDCLASS: &str = "BleHRDaemonWnd";

/// daemon 的 exe 名（必须和 manager 同目录）
pub const DAEMON_EXE: &str = "hr-daemon.exe";

/// 等 daemon 退出的上限：卡在一次 BLE 连接尝试里时收尾要十几秒
pub const DAEMON_STOP_TIMEOUT_MS: u32 = 20_000;
/// 等新 daemon 注册上互斥体（= 真跑起来了）的上限
pub const DAEMON_START_TIMEOUT_MS: u32 = 8_000;

/// hr-manager 自己的单实例互斥体：第二个实例把"打开面板"事件发给第一个就退出
pub const MANAGER_MUTEX: &str = "Local\\BleHR_manager";
/// 手动事件：第二个实例 SetEvent 一下，第一个实例收到就打开面板
pub const MANAGER_OPEN_EVENT: &str = "Local\\BleHR_manager_open";
