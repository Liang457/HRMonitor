// tools/hr-manager/build.rs —— 构建期给 hr-manager.exe 嵌 PE 版本信息，
// 资源管理器的文件属性里能看到版本号（C++ 侧 exe/DLL 走 common\version.rc.in）。
//
// 版本来源就是 Cargo.toml 的 [package] version（CARGO_PKG_VERSION 环境变量），
// 它与 common\version.h 构成跨语言双副本，scripts\pack.ps1 打包时强制校验。
use std::process::exit;

fn main() {
    // 现象：Linux 主机上交叉 cargo check 也会跑构建脚本，winresource 只认
    // Windows 的资源编译器，找不到就直接在构建脚本里报错退出，整个 check 挂掉。
    // 原因：构建脚本里 std::env::consts::OS 指的是宿主 OS，不是目标三元组的；
    // 宿主不是 Windows 时本来也产不出 PE 资源。
    // 处理：宿主是 Windows 才嵌版本资源，其余宿主跳过——Windows 本机构建/CI
    // 不受影响，Linux 主机的交叉 check 能跑通（产物少嵌版本信息，check 无所谓）。
    if std::env::consts::OS != "windows" {
        return;
    }

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
