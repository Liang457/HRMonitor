// src/win.rs —— 直接手写 Windows API 声明，不引入 windows-sys 之类的 crate。
// 只用得到很少几个函数，量很小。
#![allow(non_snake_case)]

use std::ffi::c_void;

pub type Handle = *mut c_void;
pub type Hwnd = *mut c_void;

pub const WM_CLOSE: u32 = 0x0010;

// WaitForSingleObject 的返回值
pub const WAIT_OBJECT_0: u32 = 0x0000_0000;

pub const SYNCHRONIZE: u32 = 0x0010_0000;

/// MoveFileExW 的标志位：目标已存在时直接替换
pub const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;

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
    pub fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    pub fn Sleep(ms: u32);
    pub fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    pub fn GetExitCodeProcess(h: Handle, exit_code: *mut u32) -> i32;
    pub fn TerminateProcess(h: Handle, exit_code: u32) -> i32;
    /// 只打开已存在的命名互斥体（不存在就失败）——用来问"那个进程还在吗"
    pub fn OpenMutexW(desired_access: u32, inherit_handle: i32, name: *const u16) -> Handle;
    /// 原子替换文件（配合 MOVEFILE_REPLACE_EXISTING）
    pub fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    /// buf 传 null / size 传 0 时返回需要的长度（含结尾的 0）
    pub fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
}

#[link(name = "user32")]
extern "system" {
    pub fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
    pub fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
}

// ---------------------------------------------------------------- 小工具

/// Rust 字符串 → NUL 结尾的 UTF-16
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
