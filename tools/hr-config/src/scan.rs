// src/scan.rs —— 扫一遍附近的心率广播设备。
//
// 自己不碰蓝牙：调 `hr-daemon.exe --scan`（daemon 里做好的扫描-only 模式），
// 它扫完把每台设备写成一行 "MAC<TAB>名字" 到一个文件里，然后退出。
// 结果文件的格式和退出码约定见 daemon/main.cpp 的 --scan 分支。
use crate::proc;
use std::path::PathBuf;

/// 菜单里那次扫描的时长（秒）。daemon 的扫描器不会提前结束，所以这就是菜单要停多久。
pub const SCAN_SECS: u32 = 5;

pub struct Device {
    pub mac: String,
    pub name: String,
}

/// 扫 secs 秒。Err 里是给人看的失败原因（扫描通道起不来，不是"没扫到"）。
pub fn scan(secs: u32) -> Result<Vec<Device>, String> {
    let exe = crate::config::exe_dir().join("hr-daemon.exe");
    if !exe.exists() {
        return Err(format!(
            "找不到 {}，先构建：cmd /c daemon\\build.cmd",
            exe.display()
        ));
    }

    // 先删掉同名文件 —— 扫描起不来时 daemon 不写文件，留着旧的会被当成本次结果。
    let out = temp_out_path();
    let _ = std::fs::remove_file(&out);

    // 蓝牙关着的时候 daemon 返回 1（这种情况它不写结果文件）
    let args = vec![
        "--scan".to_string(),
        secs.to_string(),
        "--out".to_string(),
        out.display().to_string(),
    ];
    let code = proc::run_and_wait(&exe, &args, secs * 1000 + 15000)?;
    if code != 0 {
        return Err("扫描起不来（蓝牙适配器关了吗？原因见 hr-daemon.log）".into());
    }

    let bytes = std::fs::read(&out).map_err(|e| format!("读不了扫描结果 {}：{}", out.display(), e))?;
    let _ = std::fs::remove_file(&out);
    Ok(parse(&bytes))
}

/// 结果文件放当前用户私有的目录，不放 %TEMP%。
///
/// %TEMP% 有时是全机器共享的（C:\Windows\Temp 之类），文件名又是可预测的
/// "<固定前缀>-<pid>.txt"；而这个文件是 daemon 用 CREATE_ALWAYS（会跟随重解析点）
/// 打开的，中间存在被抢先建成符号链接/硬链接的窗口。
/// %LOCALAPPDATA%\HRMonitor 只有当前用户能写，够用了。
fn temp_out_path() -> PathBuf {
    let dir = scan_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    dir.join(format!("scan-{}-{}.txt", std::process::id(), stamp))
}

fn scan_dir() -> PathBuf {
    if let Some(base) = local_app_data() {
        let dir = base.join("HRMonitor");
        let _ = std::fs::create_dir_all(&dir);
        if dir.is_dir() {
            return dir;
        }
    }
    // exe 目录可能装在 Program Files 下写不进去，所以兜底还是 TEMP
    std::env::temp_dir()
}

fn local_app_data() -> Option<PathBuf> {
    let name = crate::win::wide("LOCALAPPDATA");
    let need = unsafe { crate::win::GetEnvironmentVariableW(name.as_ptr(), std::ptr::null_mut(), 0) };
    if need == 0 {
        return None;
    }
    let mut buf = vec![0u16; need as usize];
    let got = unsafe { crate::win::GetEnvironmentVariableW(name.as_ptr(), buf.as_mut_ptr(), need) };
    if got == 0 || got >= need {
        return None;
    }
    let s = String::from_utf16(&buf[..got as usize]).ok()?;
    Some(PathBuf::from(s))
}

/// 结果文件是 UTF-8 带 BOM，一行一台："AA:BB:CC:DD:EE:FF\t名字"。
/// 没扫到设备时文件里只有 BOM，解析出来就是空列表。
fn parse(bytes: &[u8]) -> Vec<Device> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);

    let mut v = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (mac, name) = match line.split_once('\t') {
            Some((m, n)) => (m.trim(), n.trim()),
            None => (line, ""),
        };
        if mac.is_empty() {
            continue;
        }
        v.push(Device {
            mac: mac.to_string(),
            name: name.to_string(),
        });
    }
    v
}
