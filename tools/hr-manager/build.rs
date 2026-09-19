// tools/hr-manager/build.rs —— 构建期给 hr-manager.exe 嵌 PE 版本信息，
// 资源管理器的文件属性里能看到版本号（C++ 侧 exe/DLL 走 common\version.rc.in）。
//
// 版本来源就是 Cargo.toml 的 [package] version（CARGO_PKG_VERSION 环境变量），
// 它与 common\version.h 构成跨语言双副本，scripts\pack.ps1 打包时强制校验。
use std::process::exit;

fn main() {
    let v = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let parts: Vec<u64> = v.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let g = |i: usize| parts.get(i).copied().unwrap_or(0);
    // FILEVERSION 数字形式：从高 16 位起依次 major.minor.patch.0
    let numeric = (g(0) << 48) | (g(1) << 32) | (g(2) << 16);

    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", "hr-manager - BLE heart rate manager (tray + panel)");
    res.set("ProductName", "HRMonitor");
    res.set("FileVersion", &v);
    res.set("ProductVersion", &v);
    res.set("LegalCopyright", "MIT License");
    res.set_version_info(winresource::VersionInfo::FILEVERSION, numeric);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric);
    if let Err(e) = res.compile() {
        eprintln!("[build.rs] 嵌入版本资源失败：{}", e);
        exit(1);
    }
}
