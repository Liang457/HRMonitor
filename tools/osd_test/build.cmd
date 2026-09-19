@echo off
rem tools\osd_test\build.cmd - builds osd_test.exe and copies it into <repo>\build
rem ASCII only on purpose: this file is read by cmd, not by the C++ compiler.
setlocal

rem Work from the directory this script lives in, whatever the caller's cwd is.
cd /d "%~dp0"

rem Locate MSVC the same way the other build scripts do: vswhere first, then the
rem usual install paths. (This used to hardcode one VS 18 path, so it only built
rem on the machine it was written on.)
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
    echo [osd_test] ERROR: vcvars64.bat not found.
    echo           Install "Visual Studio Build Tools" with the C++ workload,
    echo           or call vcvars64.bat manually before running this script.
    exit /b 1
)
call "%VCVARS%" >nul
if errorlevel 1 (
    echo [osd_test] ERROR: vcvars64.bat failed.
    exit /b 1
)

if not exist "obj" mkdir "obj"

rem --- Version resource: same scheme as the shipping components (common\version.h).
rc /nologo /Fo"obj\version.res" version.rc
if errorlevel 1 (
    echo [osd_test] ERROR: rc.exe failed on version.rc
    exit /b 1
)

rem /Fo"obj\\" places the intermediate .obj in obj\ (the trailing double
rem backslash keeps cl from treating the following argument as the filename).
rem /W4 /sdl /guard:cf : keep the test tool on the same hardening level as the
rem shipping components. /utf-8 : repo rule for every C++ compile.
cl /nologo /std:c++20 /EHsc /O2 /MT /utf-8 /W4 /sdl /guard:cf /DWIN32_LEAN_AND_MEAN /DUNICODE /D_UNICODE ^
   /Fo"obj\\" ^
   main.cpp obj\version.res /Fe:osd_test.exe /link d3d11.lib dxgi.lib user32.lib
if errorlevel 1 (
    echo [osd_test] ERROR: compile or link failed, no exe produced.
    exit /b 1
)

if not exist "osd_test.exe" (
    echo [osd_test] ERROR: cl reported success but osd_test.exe is missing.
    exit /b 1
)

rem Copy to <repo>\build - derived from this script's location, so it resolves
rem to <repo>\build regardless of the console code page.
set "OUTDIR=%~dp0..\..\build"
if not exist "%OUTDIR%" mkdir "%OUTDIR%"
copy /y "osd_test.exe" "%OUTDIR%\osd_test.exe" >nul
if errorlevel 1 (
    echo [osd_test] ERROR: could not copy osd_test.exe to "%OUTDIR%".
    exit /b 1
)

echo [osd_test] OK: built osd_test.exe  -^>  "%OUTDIR%\osd_test.exe"
endlocal
exit /b 0
