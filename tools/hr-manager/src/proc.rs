// src/proc.rs —— 起进程 / 等进程 / 带管道起进程。
//
// 用途三类：
//   1. 把 hr-daemon 拉起来（不等它）—— spawn_detached
//   2. 跑一次性命令拿退出码（schtasks 查旧计划任务）—— run_and_wait
//   3. 起 `hr-daemon --scan` 并实时读它的 stdout JSON 流 —— spawn_child
use crate::win;
use std::path::Path;

/// 把一个参数包成能安全拼进命令行的形式，规则和 CommandLineToArgvW 一致：
/// 空格/制表符/引号才需要引号；引号前的反斜杠要翻倍，引号本身写成 \"，
/// 结尾的反斜杠也要翻倍（否则会把收尾的引号转义掉）。
///
/// 调用点目前都是内部生成的参数，但拼命令行这件事不该依赖"调用点都很乖" ——
/// 一个带引号的环境变量（TMP 之类）就能把参数拆成两个。
pub fn quote_arg(s: &str) -> String {
    let needs_quotes = s.is_empty() || s.chars().any(|c| c == ' ' || c == '\t' || c == '"');
    if !needs_quotes {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in s.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                for _ in 0..(backslashes * 2 + 1) {
                    out.push('\\');
                }
                backslashes = 0;
                out.push('"');
            }
            _ => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(c);
            }
        }
    }
    for _ in 0..(backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}

/// exe + 参数拼成完整命令行（每段都过 quote_arg）
fn build_command_line(exe: &Path, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(quote_arg(&exe.to_string_lossy()));
    parts.extend(args.iter().map(|a| quote_arg(a)));
    parts.join(" ")
}

fn spawn_raw(
    exe: &Path,
    args: &[String],
    inherit_handles: i32,
    creation_flags: u32,
    si_flags: u32,
    h_std_out: win::Handle,
) -> Result<win::ProcessInformation, String> {
    let exe_w = win::wide(&exe.to_string_lossy());
    // CreateProcessW 需要可写的命令行缓冲
    let mut cmdline = win::wide(&build_command_line(exe, args));
    let dir_w = win::wide(&crate::config::exe_dir().to_string_lossy());

    let mut si = win::StartupInfoW {
        cb: std::mem::size_of::<win::StartupInfoW>() as u32,
        dw_flags: si_flags,
        h_std_input: std::ptr::null_mut(),
        h_std_output: h_std_out,
        h_std_error: h_std_out,
        ..Default::default()
    };
    let mut pi = win::ProcessInformation::default();

    let ok = unsafe {
        win::CreateProcessW(
            exe_w.as_ptr(),
            cmdline.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inherit_handles,
            creation_flags,
            std::ptr::null_mut(),
            dir_w.as_ptr(),
            &mut si,
            &mut pi,
        )
    };
    if ok == 0 {
        // 先取错误码再格式化：中间的任何分配都可能把它冲掉。
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return Err(format!("启动失败：{}（错误码 {}）", exe.display(), err));
    }
    Ok(pi)
}

/// 起一个进程就走，不等它（daemon 常驻就是这样起的）。
/// 参数统一带 --quiet：daemon 是常驻后台，不附加任何控制台，
/// 日志只进文件（hr-daemon.log）。
pub fn spawn_detached(exe: &Path, args: &[&str]) -> Result<(), String> {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let pi = spawn_raw(exe, &owned, 0, 0, 0, std::ptr::null_mut())?;
    unsafe {
        win::CloseHandle(pi.h_thread);
        win::CloseHandle(pi.h_process);
    }
    Ok(())
}

/// 起进程、等它自己结束，返回退出码。超时就强制结束并报错。
/// CREATE_NO_WINDOW：我们自己没有控制台时（GUI/托盘里点按钮），别给控制台
/// 子进程（schtasks 之类）闪黑窗。
pub fn run_and_wait(exe: &Path, args: &[String], timeout_ms: u32) -> Result<u32, String> {
    let pi = spawn_raw(exe, &args.to_vec(), 0, win::CREATE_NO_WINDOW, 0, std::ptr::null_mut())?;

    let waited = unsafe { win::WaitForSingleObject(pi.h_process, timeout_ms) };
    let mut code: u32 = 0;
    if waited == win::WAIT_OBJECT_0 {
        unsafe { win::GetExitCodeProcess(pi.h_process, &mut code) };
    } else {
        unsafe { win::TerminateProcess(pi.h_process, 1) };
    }
    unsafe {
        win::CloseHandle(pi.h_thread);
        win::CloseHandle(pi.h_process);
    }

    if waited != win::WAIT_OBJECT_0 {
        return Err(format!(
            "{} 跑了 {} 秒还没结束，已强制结束",
            exe.file_name().unwrap_or_default().to_string_lossy(),
            timeout_ms / 1000
        ));
    }
    Ok(code)
}

// ---------------------------------------------------------------- 带管道的子进程

/// 一个带 stdout 读端的子进程。句柄非 Send，封一层保证只在线程内挪动。
pub struct Child {
    pub process: win::Handle,
    thread: win::Handle,
    /// stdout 读端。None = 没接管道。
    pub stdout: Option<std::fs::File>,
}

/// 跨线程传"可以 TerminateProcess 的裸句柄"用（扫描取消按钮 → 扫描线程）。
pub struct CancelHandle(pub win::Handle);
// 句柄本身跨线程可用（内核对象），只是裸指针类型系统不认
unsafe impl Send for CancelHandle {}

impl Child {
    /// 等进程退出，返回退出码；超过 timeout_ms 强杀并返回 1。
    pub fn wait(&self, timeout_ms: u32) -> u32 {
        let waited = unsafe { win::WaitForSingleObject(self.process, timeout_ms) };
        let mut code: u32 = 1;
        if waited == win::WAIT_OBJECT_0 {
            unsafe { win::GetExitCodeProcess(self.process, &mut code) };
        } else {
            unsafe { win::TerminateProcess(self.process, 1) };
        }
        code
    }

    pub fn kill(&self) {
        unsafe { win::TerminateProcess(self.process, 1) };
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        unsafe {
            win::CloseHandle(self.thread);
            win::CloseHandle(self.process);
        }
    }
}

/// 起进程并接管它的 stdout（匿名管道）。用于 `hr-daemon --scan` 的流式 JSON。
/// 子进程跑在隐藏窗口里：不管我们自己有没有控制台都不闪黑窗。
pub fn spawn_child(exe: &Path, args: &[String]) -> Result<Child, String> {
    let mut read_end: win::Handle = std::ptr::null_mut();
    let mut write_end: win::Handle = std::ptr::null_mut();
    let sa = win::SecurityAttributes {
        n_length: std::mem::size_of::<win::SecurityAttributes>() as u32,
        lp_security_descriptor: std::ptr::null_mut(),
        b_inherit_handle: 1,
    };
    let ok = unsafe { win::CreatePipe(&mut read_end, &mut write_end, &sa, 0) };
    if ok == 0 {
        return Err(format!("建管道失败（错误码 {}）", std::io::Error::last_os_error()));
    }
    // 写端可继承，读端明确不可继承（否则读端句柄被子进程攥着，EOF 永远不来）
    unsafe {
        win::SetHandleInformation(write_end, win::HANDLE_FLAG_INHERIT, win::HANDLE_FLAG_INHERIT);
        win::SetHandleInformation(read_end, win::HANDLE_FLAG_INHERIT, 0);
    }

    let spawned = spawn_raw(
        exe,
        args,
        1,
        win::CREATE_NO_WINDOW,
        win::STARTF_USESTDHANDLES,
        write_end,
    );
    unsafe { win::CloseHandle(write_end) }; // 父进程这头用完就关，让 EOF 能到达
    let pi = spawned?;

    let stdout = unsafe { std::fs::File::from_raw_handle(read_end) };
    Ok(Child { process: pi.h_process, thread: pi.h_thread, stdout: Some(stdout) })
}

use std::os::windows::io::FromRawHandle;

/// 跑一个控制台命令并收走它的全部 stdout/stderr（隐藏窗口）。
/// 给部署脚本用：输出回显到面板里。超时强杀。
pub fn run_and_wait_capture(
    exe: &Path,
    args: &[String],
    timeout_ms: u32,
) -> Result<(u32, String), String> {
    let mut child = spawn_child(exe, args)?;
    let stdout = child.stdout.take().expect("spawn_child 一定带 stdout");
    let mut out = String::new();
    let mut file = stdout;
    use std::io::Read as _;
    let _ = file.read_to_string(&mut out); // 字节流不是合法 UTF-8 也会尽力解
    let code = child.wait(timeout_ms);
    Ok((code, out))
}
