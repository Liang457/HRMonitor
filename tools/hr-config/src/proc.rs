// src/proc.rs —— 起进程 / 等进程。
//
// 只有 CreateProcessW 这一条路，没有别的地方再碰进程相关的 API。
// 用途就两个：把 hr-daemon 拉起来（不等它），和跑一次 `hr-daemon.exe --scan`
// 并等它退出（要它的退出码和结果文件）。
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

/// 起一个进程，返回它的句柄信息（**调用方负责 CloseHandle**）。
/// 不带控制台窗口：daemon 是 GUI 子系统，本身也不会弹窗。
pub fn spawn(exe: &Path, args: &[String]) -> Result<win::ProcessInformation, String> {
    let exe_w = win::wide(&exe.to_string_lossy());
    // CreateProcessW 需要可写的命令行缓冲
    let mut cmdline = win::wide(&build_command_line(exe, args));
    let dir_w = win::wide(&crate::config::exe_dir().to_string_lossy());

    let mut si = win::StartupInfoW {
        cb: std::mem::size_of::<win::StartupInfoW>() as u32,
        ..Default::default()
    };
    let mut pi = win::ProcessInformation::default();

    let ok = unsafe {
        win::CreateProcessW(
            exe_w.as_ptr(),
            cmdline.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            0, // 不继承句柄、不新建控制台
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
pub fn spawn_detached(exe: &Path) -> Result<(), String> {
    let pi = spawn(exe, &[])?;
    unsafe {
        win::CloseHandle(pi.h_thread);
        win::CloseHandle(pi.h_process);
    }
    Ok(())
}

/// 起进程、等它自己结束，返回退出码。超时就强制结束并报错。
pub fn run_and_wait(exe: &Path, args: &[String], timeout_ms: u32) -> Result<u32, String> {
    let pi = spawn(exe, args)?;

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
