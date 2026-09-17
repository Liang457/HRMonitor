// src/win.rs —— Windows API 的 extern "system" 手写声明。
//
// 原则：能少依赖就少依赖。GUI 栈（wry/tao/tray-icon）
// 自带它们需要的 windows-* 绑定，我们自己的业务代码只手写这几个声明，
// 不直接依赖 windows crate。
#![allow(non_snake_case)]

use std::ffi::c_void;

pub type Handle = *mut c_void;
pub type Hwnd = *mut c_void;
pub type Hkey = *mut c_void;

pub const WM_CLOSE: u32 = 0x0010;

// WaitForSingleObject 的返回值
pub const WAIT_OBJECT_0: u32 = 0x0000_0000;
/// WaitForSingleObject 的超时上限：无限等
pub const INFINITE: u32 = 0xFFFF_FFFF;

pub const SYNCHRONIZE: u32 = 0x0010_0000;
/// SetEvent 要的权限位（只给 SYNCHRONIZE 的话 SetEvent 会被拒绝）
pub const EVENT_MODIFY_STATE: u32 = 0x0000_0002;

/// MoveFileExW 的标志位：目标已存在时直接替换
pub const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;

/// CreateProcessW：给控制台子进程用，免得闪黑窗
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// StartupInfoW.dwFlags：hStdInput/Output/Error 生效的前提
pub const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
/// SetHandleInformation：句柄可被子进程继承
pub const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;

/// OpenProcess：只查询基本信息（映像名等），不需要能操作进程
pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x0000_1000;
/// DuplicateHandle：新句柄获得与源句柄相同的访问权
pub const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;
/// Job：句柄全关时杀光作业里的进程（manager 死则扫描子进程必死）
pub const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
/// SetInformationJobObject 的信息类：JOBOBJECT_EXTENDED_LIMIT_INFORMATION
pub const JOB_OBJECT_EXTENDED_LIMIT: i32 = 9;

// GetStdHandle 的标准句柄号
pub const STD_INPUT_HANDLE: u32 = -10i32 as u32;
pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
pub const STD_ERROR_HANDLE: u32 = -12i32 as u32;

// CreateFileW 的 disposition
pub const OPEN_EXISTING: u32 = 3;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const GENERIC_READ: u32 = 0x8000_0000;

// 注册表
pub const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as Hkey;
pub const KEY_READ: u32 = 0x0002_0019;
pub const KEY_WRITE: u32 = 0x0002_0006;
pub const REG_SZ: u32 = 1;
pub const REG_EXPAND_SZ: u32 = 2;

// ShellExecuteW 的 Show 命令
pub const SW_SHOWNORMAL: i32 = 1;

#[repr(C)]
pub struct StartupInfoW {
    pub cb: u32,
    pub lp_reserved: *mut u16,
    pub lp_desktop: *mut u16,
    pub lp_title: *mut u16,
    pub dw_x: u32,
    pub dw_y: u32,
    pub dw_x_size: u32,
    pub dw_y_size: u32,
    pub dw_x_count_chars: u32,
    pub dw_y_count_chars: u32,
    pub dw_fill_attribute: u32,
    pub dw_flags: u32,
    pub w_show_window: u16,
    pub cb_reserved2: u16,
    pub lp_reserved2: *mut u8,
    pub h_std_input: Handle,
    pub h_std_output: Handle,
    pub h_std_error: Handle,
}

impl Default for StartupInfoW {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct ProcessInformation {
    pub h_process: Handle,
    pub h_thread: Handle,
    pub dw_process_id: u32,
    pub dw_thread_id: u32,
}

/// SECURITY_ATTRIBUTES（只为了 bInheritHandle）
#[repr(C)]
pub struct SecurityAttributes {
    pub n_length: u32,
    pub lp_security_descriptor: *mut c_void,
    pub b_inherit_handle: i32,
}

// ---- Job Object（把扫描/脚本子进程拴住：manager 死则它们必死）----
// 只用 KILL_ON_JOB_CLOSE 一个开关，其余字段全部留零。
#[repr(C)]
#[derive(Default)]
pub struct IoCounters {
    pub read_operation_count: u64,
    pub write_operation_count: u64,
    pub other_operation_count: u64,
    pub read_transfer_count: u64,
    pub write_transfer_count: u64,
    pub other_transfer_count: u64,
}

#[repr(C)]
#[derive(Default)]
pub struct JobObjectBasicLimitInformation {
    pub per_process_user_time: i64,
    pub per_job_user_time: i64,
    pub limit_flags: u32,
    pub minimum_working_set_size: usize,
    pub maximum_working_set_size: usize,
    pub active_process_limit: u32,
    pub affinity: usize,
    pub priority_class: u32,
    pub scheduling_class: u32,
}

#[repr(C)]
pub struct JobObjectExtendedLimitInformation {
    pub basic_limit_information: JobObjectBasicLimitInformation,
    pub io_info: IoCounters,
    pub process_memory_limit: usize,
    pub job_memory_limit: usize,
    pub peak_process_memory_used: usize,
    pub peak_job_memory_used: usize,
}

impl Default for JobObjectExtendedLimitInformation {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[link(name = "kernel32")]
extern "system" {
    pub fn CreateProcessW(
        app_name: *const u16,
        command_line: *mut u16,
        process_attrs: *mut c_void,
        thread_attrs: *mut c_void,
        inherit_handles: i32,
        creation_flags: u32,
        env: *mut c_void,
        current_dir: *const u16,
        startup_info: *mut StartupInfoW,
        process_info: *mut ProcessInformation,
    ) -> i32;

    pub fn CloseHandle(h: Handle) -> i32;
    pub fn GetLastError() -> u32;
    pub fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    pub fn Sleep(ms: u32);
    pub fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    pub fn GetExitCodeProcess(h: Handle, exit_code: *mut u32) -> i32;
    pub fn TerminateProcess(h: Handle, exit_code: u32) -> i32;
    /// 只打开已存在的命名互斥体（不存在就失败）——用来问"那个进程还在吗"
    pub fn OpenMutexW(desired_access: u32, inherit_handle: i32, name: *const u16) -> Handle;
    /// 创建/打开命名互斥体；已存在时 GetLastError() = ERROR_ALREADY_EXISTS (183)
    pub fn CreateMutexW(attrs: *mut c_void, initial_owner: i32, name: *const u16) -> Handle;
    /// 命名事件：第二个 manager 实例用它通知第一个实例"打开面板"
    pub fn CreateEventW(
        attrs: *mut c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> Handle;
    pub fn OpenEventW(desired_access: u32, inherit_handle: i32, name: *const u16) -> Handle;
    pub fn SetEvent(h: Handle) -> i32;
    /// 匿名管道：读端给自己、写端给子进程（--scan 的 JSON 流）
    pub fn CreatePipe(
        read_pipe: *mut Handle,
        write_pipe: *mut Handle,
        attrs: *const SecurityAttributes,
        size: u32,
    ) -> i32;
    pub fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> i32;
    /// 只读打开命名共享内存（shared_mem.rs 读 HrSharedData 用）
    pub fn OpenFileMappingW(desired_access: u32, inherit_handle: i32, name: *const u16) -> Handle;
    pub fn MapViewOfFile(
        mapping: Handle,
        desired_access: u32,
        high: u32,
        low: u32,
        bytes: usize,
    ) -> *mut c_void;
    pub fn UnmapViewOfFile(view: *const c_void) -> i32;
    pub fn GetTickCount64() -> u64;
    /// 原子替换文件（配合 MOVEFILE_REPLACE_EXISTING）
    pub fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    // ---- CLI 模式附加父控制台用（GUI 子系统默认没有标准句柄）----
    pub fn AttachConsole(pid: u32) -> i32;
    pub fn GetStdHandle(kind: u32) -> Handle;
    pub fn SetStdHandle(kind: u32, h: Handle) -> i32;
    pub fn GetConsoleWindow() -> Hwnd;
    pub fn CreateFileW(
        name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    /// 复制一份自己的句柄（CancelHandle 用：跟 Child 的 Drop 解耦，防句柄复用窗口）
    pub fn DuplicateHandle(
        source_proc: Handle,
        source: Handle,
        target_proc: Handle,
        target: *mut Handle,
        desired_access: u32,
        inherit_handle: i32,
        options: u32,
    ) -> i32;
    pub fn GetCurrentProcess() -> Handle;
    pub fn OpenProcess(desired_access: u32, inherit_handle: i32, pid: u32) -> Handle;
    pub fn QueryFullProcessImageNameW(h: Handle, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
    // ---- Job Object ----
    pub fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> Handle;
    pub fn SetInformationJobObject(
        job: Handle,
        info_class: i32,
        info: *mut c_void,
        size: u32,
    ) -> i32;
    pub fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
}

#[link(name = "user32")]
extern "system" {
    pub fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
    pub fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    pub fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
}

#[link(name = "advapi32")]
extern "system" {
    pub fn RegOpenKeyExW(key: Hkey, sub_key: *const u16, options: u32, access: u32, result: *mut Hkey) -> i32;
    pub fn RegQueryValueExW(
        key: Hkey,
        value_name: *const u16,
        reserved: *mut u32,
        typ: *mut u32,
        data: *mut u8,
        data_len: *mut u32,
    ) -> i32;
    pub fn RegSetValueExW(
        key: Hkey,
        value_name: *const u16,
        reserved: u32,
        typ: u32,
        data: *const u8,
        data_len: u32,
    ) -> i32;
    pub fn RegDeleteValueW(key: Hkey, value_name: *const u16) -> i32;
    pub fn RegCloseKey(key: Hkey) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    /// 打开文件夹/文件/URL。verb "open"；返回值 > 32 = 成功
    pub fn ShellExecuteW(
        hwnd: Hwnd,
        verb: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> isize;
}

pub const ERROR_ALREADY_EXISTS: u32 = 183;

// ---------------------------------------------------------------- 小工具

/// CreateFileW 这类"返回句柄"的 API 失败时返回的是 INVALID_HANDLE_VALUE（-1）
/// 而不是 null，两者都得当"没拿到"看。
pub fn is_invalid(h: Handle) -> bool {
    h as usize == usize::MAX
}

/// Rust 字符串 → NUL 结尾的 UTF-16
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
