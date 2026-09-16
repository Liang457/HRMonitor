@echo off
REM tools/hr-config/build.cmd -- build hr-config.exe (Rust, no crates, no network)
REM Usage: cmd /c build.cmd   (also works when called with an absolute path)
setlocal
cd /d "%~dp0"

where cargo >nul 2>&1
if errorlevel 1 (
    echo [build] ERROR: 找不到 cargo
    echo         装一个 Rust 工具链: https://rustup.rs  ^(rustup-init.exe，默认选项即可^)
    echo         本工具不依赖任何 crate，所以不需要联网也不需要 MSVC。
    exit /b 1
)

echo [build] cargo build --release --offline ...
REM --offline : Cargo.toml 里没有任何依赖，显式禁网，构建完全可复现
cargo build --release --offline
if errorlevel 1 (
    echo [build] ERROR: cargo build 失败
    exit /b 1
)

if not exist "..\..\build" mkdir "..\..\build"
copy /y "target\release\hr-config.exe" "..\..\build\hr-config.exe" >nul
if errorlevel 1 (
    echo [build] ERROR: 拷贝到 build\ 失败（目标文件可能在运行中）
    exit /b 1
)

echo [build] OK: %~dp0target\release\hr-config.exe
echo [build] OK: %~dp0..\..\build\hr-config.exe
endlocal
exit /b 0
