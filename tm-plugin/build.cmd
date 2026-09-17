@echo off
rem tm-plugin/build.cmd -- build the TrafficMonitor plugin hr_plugin.dll (x64)
rem Usage: cmd /c build.cmd   (also works when called with an absolute path)
rem
rem NOTE ON THE FILE NAME: TrafficMonitor V1.86 loads plugins by scanning
rem "<exe dir>\plugins\*.dll" (TrafficMonitor/PluginManager.cpp) and the string
rem ".tmd" does not appear anywhere in the V1.86 sources -- the .tmd extension
rem was used by an older plugin system. A .tmd file is silently ignored, so the
rem plugin is emitted as a plain .dll.
setlocal
cd /d "%~dp0"

rem --- locate MSVC: prefer vswhere so we are not tied to a VS version/path
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
    echo [build] ERROR: vcvars64.bat not found.
    echo         Install "Visual Studio Build Tools" with the C++ tools workload and retry.
    exit /b 1
)
echo [build] MSVC: %VCVARS%
call "%VCVARS%" >nul
if errorlevel 1 (
    echo [build] ERROR: failed to initialize the MSVC environment
    exit /b 1
)

rem Keep intermediates inside obj\ so the tree stays tidy.
if not exist "obj" mkdir "obj"

echo [build] compiling plugin.cpp ...
rem /LD    : build a DLL (this is what TrafficMonitor loads)
rem /MT    : static CRT, so no VC++ redistributable is needed
rem /utf-8 : plugin.cpp is UTF-8 without a BOM. Without this flag MSVC falls back
rem           to the machine's ANSI codepage, so the Chinese UI strings (e.g. Heart)
rem           only come out right by luck and break on a differently-locale'd box.
rem /W4 /sdl /guard:cf : high warning level + extra security checks + control-flow guard.
rem /Zi    : PDB next to the DLL for crash symbolization.
rem hr_config.cpp provides the hr_plugin.ini reading (display label / item name).
rem /INCREMENTAL:NO keeps the export table clean (no @ILT thunks), so the
rem self-check below can anchor the export name at end-of-line.
cl /nologo /LD /O2 /MT /EHsc /std:c++20 /utf-8 /W4 /sdl /guard:cf /Zi /DUNICODE /D_UNICODE /DWIN32_LEAN_AND_MEAN /I..\common /Foobj\ plugin.cpp ..\common\hr_config.cpp /Fe:hr_plugin.dll /link /INCREMENTAL:NO /EXPORT:TMPluginGetInstance
if errorlevel 1 (
    echo [build] ERROR: compile/link failed
    exit /b 1
)
if not exist "hr_plugin.dll" (
    echo [build] ERROR: hr_plugin.dll was not produced
    exit /b 1
)

rem --- Self-check, same idea as ab-plugin: the DLL must be x64 and the export
rem --- must be present and undecorated, or the 64-bit host fails to load us.
dumpbin /nologo /headers hr_plugin.dll | findstr /i /c:"8664 machine" >nul
if errorlevel 1 (
    echo [build] ERROR: output is not x64. TrafficMonitor is a 64-bit process.
    exit /b 1
)
dumpbin /nologo /exports hr_plugin.dll | findstr /r /c:" TMPluginGetInstance$" >nul
if errorlevel 1 (
    echo [build] ERROR: export TMPluginGetInstance is missing or decorated.
    dumpbin /nologo /exports hr_plugin.dll
    exit /b 1
)

rem Drop the linker leftovers.
if exist "hr_plugin.exp" del /q "hr_plugin.exp"
if exist "hr_plugin.lib" del /q "hr_plugin.lib"
if exist "vc140.pdb"     del /q "vc140.pdb"

rem Also place a copy under the repo build\ directory.
if not exist "..\build" mkdir "..\build"
copy /y "hr_plugin.dll" "..\build\hr_plugin.dll" >nul
if errorlevel 1 (
    echo [build] ERROR: cannot copy to ..\build\hr_plugin.dll -- the file may be in use by TrafficMonitor
    exit /b 1
)

echo [build] OK: %~dp0hr_plugin.dll  - x64, export TMPluginGetInstance
echo [build] OK: %~dp0..\build\hr_plugin.dll
endlocal
exit /b 0
