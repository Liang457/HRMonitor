@echo off
REM tools/hr-manager/build.cmd -- build hr-manager.exe (Rust + wry GUI)
REM Usage: cmd /c build.cmd   (also works when called with an absolute path)
setlocal
cd /d "%~dp0"

where cargo >nul 2>&1
if errorlevel 1 (
    echo [build] ERROR: cannot find cargo
    echo         Install a Rust toolchain: https://rustup.rs  ^(rustup-init.exe, default options^)
    exit /b 1
)

REM First build needs network to fetch crates (wry/tao/tray-icon); Cargo.lock
REM pins the versions, so later rebuilds are reproducible. Repo policy: the
REM GUI stack is the only place third-party crates are allowed.
echo [build] cargo build --release ...
cargo build --release
if errorlevel 1 (
    echo [build] ERROR: cargo build failed
    exit /b 1
)

if not exist "..\..\build" mkdir "..\..\build"
copy /y "target\release\hr-manager.exe" "..\..\build\hr-manager.exe" >nul
if errorlevel 1 (
    echo [build] ERROR: copy to build\ failed ^(target file may be running^)
    exit /b 1
)

echo [build] OK: %~dp0target\release\hr-manager.exe
echo [build] OK: %~dp0..\..\build\hr-manager.exe
endlocal
exit /b 0
