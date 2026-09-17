@echo off
REM Build HeartRate.dll -- MSI Afterburner hardware monitoring plugin (x86, static CRT).
REM
REM Why x86: MSIAfterburner.exe is a 32-bit process (PE machine 0x14C) and cannot
REM load an x64 DLL. Unrelated to hr-daemon.exe (x64); the two only exchange data
REM through the shared memory mapping.
REM
REM Why no MFC: this plugin does not implement SetupSource, so it never receives the
REM MFC CWnd the host passes in. Official samples are all MFC extension DLLs only
REM because their SetupSource dialogs need MFC.
REM
REM NOTE 1: keep this file ASCII-only. cmd.exe re-reads a .cmd byte-wise and misparses
REM when the console codepage differs from the file encoding, so non-ASCII text in a
REM batch file is environment-dependent breakage. Chinese notes live in plugin.cpp.
REM NOTE 2: never put a bare ) or ( in text echoed from inside a ( ) block -- it closes
REM the block early and cmd reports a nonsense token error.
setlocal
cd /d "%~dp0"

REM --- Locate MSVC: prefer vswhere (no hardcoded VS version/path), fall back to common paths
set "VCVARS="
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if exist "%VSWHERE%" (
    for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do (
        if exist "%%i\VC\Auxiliary\Build\vcvarsall.bat" set "VCVARS=%%i\VC\Auxiliary\Build\vcvarsall.bat"
    )
)
if not defined VCVARS (
    for %%p in (
        "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
        "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvarsall.bat"
        "C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvarsall.bat"
        "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvarsall.bat"
    ) do if not defined VCVARS if exist %%p set "VCVARS=%%~p"
)
if not defined VCVARS (
    echo [ERROR] vcvarsall.bat not found.
    echo         Install "Visual Studio Build Tools" with the C++ tools workload and
    echo         make sure the x86 target is included, then retry. Required component:
    echo         Microsoft.VisualStudio.Component.VC.Tools.x86.x64
    echo         Or call vcvarsall.bat x86 yourself and run this script in that shell.
    exit /b 1
)
echo [build] MSVC: %VCVARS%  - target x86
call "%VCVARS%" x86 >nul
if errorlevel 1 (
    echo [ERROR] failed to initialize the MSVC x86 environment
    exit /b 1
)

if not exist obj mkdir obj

REM /utf-8 : sources are UTF-8; without it MSVC reads them as the system codepage
REM          and the Chinese comments in plugin.cpp break the build.
REM /MT    : static CRT, so the DLL has no VC++ runtime dependency.
REM /W4 /sdl /guard:cf : high warning level + extra security checks + control-flow guard.
REM /Zi    : PDB next to the DLL for crash symbolization.
REM UNICODE is deliberately NOT defined: MONITORING_SOURCE_DESC uses char[], so the
REM          plugin must stay MBCS.
cl /nologo /LD /std:c++20 /EHsc /O2 /MT /utf-8 /W4 /sdl /guard:cf /Zi ^
   /DWIN32_LEAN_AND_MEAN /DNOMINMAX /D_WIN32_WINNT=0x0601 ^
   /I"..\common" ^
   /Fo"obj\\" ^
   plugin.cpp ^
   /Fe:HeartRate.dll ^
   /link /INCREMENTAL:NO
if errorlevel 1 (
    echo [ERROR] build failed
    exit /b 1
)

REM The import library and exports file are /LD byproducts; the host resolves
REM everything by name with GetProcAddress, so it does not need them.
del /q HeartRate.lib HeartRate.exp vc140.pdb 2>nul

REM --- Self-check: the DLL must be x86, and the three exports must be UNDECORATED.
REM     A decorated name such as _GetSourceData@4 would make the host fail to load us.
REM     (No $ anchor on the name: /Zi makes dumpbin print "name = _name" notes,
REM     but the leading space still rejects a decorated "_Name@4".)
dumpbin /nologo /headers HeartRate.dll | findstr /i /c:"14C machine" >nul
if errorlevel 1 (
    echo [ERROR] output is not x86 -- check that vcvarsall was initialized with x86.
    exit /b 1
)
for %%n in (GetSourcesNum GetSourceDesc GetSourceData) do (
    dumpbin /nologo /exports HeartRate.dll | findstr /r /c:" %%n" >nul
    if errorlevel 1 (
        echo [ERROR] export %%n is missing or decorated.
        echo         The host resolves plugins by name, so the name must match exactly.
        dumpbin /nologo /exports HeartRate.dll
        exit /b 1
    )
)

echo.
echo [OK] ab-plugin\HeartRate.dll  - x86
echo [OK] exports: GetSourcesNum / GetSourceDesc / GetSourceData
if not exist "..\build" mkdir "..\build"
copy /y HeartRate.dll "..\build\HeartRate.dll" >nul
if errorlevel 1 (
    echo [ERROR] cannot copy to ..\build\HeartRate.dll
    exit /b 1
)
echo [OK] build\HeartRate.dll
echo.
echo Deploy into Afterburner and enable it - needs administrator:
echo     powershell -ExecutionPolicy Bypass -File scripts\deploy-afterburner-plugin.ps1
endlocal
