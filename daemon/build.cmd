@echo off
REM Build hr-daemon.exe (x64, static CRT, GUI subsystem).
REM Depends only on the Windows SDK: C++/WinRT BLE via windowsapp.lib, no third-party libs.
REM Writes the neutral shared memory Local\HuaWeiHR_SM only; the OSD is Afterburner's job.
setlocal
cd /d "%~dp0"

REM --- 定位 MSVC：优先 vswhere（不绑死 VS 版本/安装路径），失败再退回常见位置
set "VCVARS="
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if exist "%VSWHERE%" (
    for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do (
        if exist "%%i\VC\Auxiliary\Build\vcvars64.bat" set "VCVARS=%%i\VC\Auxiliary\Build\vcvars64.bat"
    )
)
if not defined VCVARS (
    for %%p in (
        "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
        "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
        "C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat"
        "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat"
    ) do if not defined VCVARS if exist %%p set "VCVARS=%%~p"
)
if not defined VCVARS (
    echo [ERROR] 找不到 vcvars64.bat
    echo         装一个 "Visual Studio Build Tools"（勾选 C++ 生成工具）再试，
    echo         或者设好 VSINSTALLDIR / 手动 call vcvars64.bat 后再跑本脚本。
    exit /b 1
)
echo [build] MSVC: %VCVARS%
call "%VCVARS%" >nul
if errorlevel 1 (
    echo [ERROR] failed to initialize MSVC environment
    exit /b 1
)

if not exist obj mkdir obj

REM /utf-8 : sources are UTF-8, without it MSVC reads them as the system codepage
REM          and the Chinese log strings come out as mojibake.
REM /MT    : static CRT, so the exe runs without a VC++ redistributable.
REM /W4 /sdl /guard:cf : high warning level + extra security checks + control-flow guard.
REM /Zi    : generate a PDB next to the exe so a crash dump can be symbolized.
cl /nologo /std:c++20 /EHsc /O2 /MT /utf-8 /W4 /sdl /guard:cf /Zi ^
   /DUNICODE /D_UNICODE /DWIN32_LEAN_AND_MEAN /DNOMINMAX /D_WIN32_WINNT=0x0A00 ^
   /I"..\common" ^
   /Fo"obj\\" ^
   main.cpp log.cpp demo.cpp ble.cpp ..\common\hr_config.cpp ^
   /Fe:hr-daemon.exe ^
   /link /SUBSYSTEM:WINDOWS windowsapp.lib user32.lib shell32.lib
if errorlevel 1 (
    echo [ERROR] build failed
    exit /b 1
)

echo.
echo [OK] daemon\hr-daemon.exe
if not exist "..\build" mkdir "..\build"
copy /y hr-daemon.exe "..\build\hr-daemon.exe" >nul
if errorlevel 1 (
    echo [ERROR] 拷不到 ..\build\hr-daemon.exe —— 多半是 hr-daemon 正在运行占着文件。
    echo         先结束它: taskkill /IM hr-daemon.exe   然后再跑一次本脚本。
    exit /b 1
)
if exist hr-daemon.pdb copy /y hr-daemon.pdb "..\build\hr-daemon.pdb" >nul
echo [OK] build\hr-daemon.exe
endlocal
