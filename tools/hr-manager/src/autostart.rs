// src/autostart.rs —— 开机自启：HKCU 的 Run 注册表键。
//
// 刻意不用计划任务（老版本的方案，已废弃）：Run 键不需要管理员，
// 天然在当前登录会话里启动（共享内存 Local\ 命名空间因此一定对），
// GUI 里的一个开关就能管理。
use crate::proc;
use crate::win;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "BleHRManager";

/// 当前自启项的内容（None = 没开）。只看键在不在，内容不校验。
pub fn get() -> Option<String> {
    let sub = win::wide(RUN_KEY);
    let name = win::wide(VALUE_NAME);
    let mut hkey: win::Hkey = std::ptr::null_mut();
    let opened =
        unsafe { win::RegOpenKeyExW(win::HKEY_CURRENT_USER, sub.as_ptr(), 0, win::KEY_READ, &mut hkey) };
    if opened != 0 {
        return None;
    }
    let mut typ: u32 = 0;
    let mut buf = [0u8; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        win::RegQueryValueExW(hkey, name.as_ptr(), std::ptr::null_mut(), &mut typ, buf.as_mut_ptr(), &mut len)
    };
    unsafe { win::RegCloseKey(hkey) };
    if ok != 0 || (typ != win::REG_SZ && typ != win::REG_EXPAND_SZ) {
        return None;
    }
    // REG_SZ 是 UTF-16；按截断到的字节数解（len 含结尾 NUL）
    let units: Vec<u16> = buf[..len as usize]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// 开启自启：写 "exe 路径 --minimized"（登录后只出托盘，不弹面板）。
pub fn enable() -> Result<(), String> {
    let exe = crate::config::exe_dir();
    let exe = exe.join("hr-manager.exe");
    let path = exe.to_string_lossy();
    if path.starts_with("\\\\") {
        return Err("程序在网络路径（UNC）上，登录时网络未必就绪，不能设自启".into());
    }
    let data = proc::quote_arg(&path) + " --minimized";
    let mut data_u16: Vec<u16> = data.encode_utf16().collect();
    data_u16.push(0); // REG_SZ 以 NUL 结尾

    let sub = win::wide(RUN_KEY);
    let name = win::wide(VALUE_NAME);
    let mut hkey: win::Hkey = std::ptr::null_mut();
    let opened = unsafe {
        win::RegOpenKeyExW(win::HKEY_CURRENT_USER, sub.as_ptr(), 0, win::KEY_WRITE, &mut hkey)
    };
    if opened != 0 {
        return Err(format!("打不开注册表 Run 键（错误码 {}）", opened));
    }
    let ok = unsafe {
        win::RegSetValueExW(
            hkey,
            name.as_ptr(),
            0,
            win::REG_SZ,
            data_u16.as_ptr() as *const u8,
            (data_u16.len() * 2) as u32,
        )
    };
    unsafe { win::RegCloseKey(hkey) };
    if ok != 0 {
        return Err(format!("写入注册表失败（错误码 {}）", ok));
    }
    Ok(())
}

/// 关闭自启。键本来就不存在也算成功。
pub fn disable() -> Result<(), String> {
    let sub = win::wide(RUN_KEY);
    let name = win::wide(VALUE_NAME);
    let mut hkey: win::Hkey = std::ptr::null_mut();
    let opened = unsafe {
        win::RegOpenKeyExW(win::HKEY_CURRENT_USER, sub.as_ptr(), 0, win::KEY_WRITE, &mut hkey)
    };
    if opened != 0 {
        return Ok(()); // Run 键都在 = 从没开过自启
    }
    let ok = unsafe { win::RegDeleteValueW(hkey, name.as_ptr()) };
    unsafe { win::RegCloseKey(hkey) };
    if ok != 0 && ok != 2 {
        // 2 = ERROR_FILE_NOT_FOUND，本来就没设
        return Err(format!("删除注册表值失败（错误码 {}）", ok));
    }
    Ok(())
}

/// 老版本用计划任务（HuaweiHRDaemon）自启，查一下在不在——在的话面板会提示
/// 用户用 scripts/uninstall-task.ps1 清掉，免得登录时起了两个采集实例。
pub fn old_task_exists() -> bool {
    let schtasks = std::path::PathBuf::from("schtasks.exe");
    matches!(
        proc::run_and_wait(
            &schtasks,
            &["/query".to_string(), "/tn".to_string(), "HuaweiHRDaemon".to_string()],
            15_000,
        ),
        Ok(0)
    )
}
