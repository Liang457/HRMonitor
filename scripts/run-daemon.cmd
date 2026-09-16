@echo off
rem scripts/run-daemon.cmd -- run hr-daemon in this console window.
rem
rem hr-daemon is a GUI-subsystem binary, so it does not open its own window. When
rem started from cmd it attaches to this console, which means the log streams here
rem live and Ctrl+C stops it cleanly. cmd will keep this window busy until the
rem daemon exits -- that is intentional for a foreground/debug run.
rem
rem To run it detached instead, use one of:
rem   start "" build\hr-daemon.exe            (in a new window)
rem   powershell scripts\install-task.ps1     (auto-start at logon)
rem
rem Usage:
rem   run-daemon.cmd                                    scan for the watch and connect
rem   run-daemon.cmd --demo                             simulated heart rate, no watch
rem   run-daemon.cmd --address AA:BB:CC:DD:EE:FF        skip scanning, connect directly
rem
rem Stop with Ctrl+C, or from another window: taskkill /IM hr-daemon.exe
setlocal
cd /d "%~dp0.."

set "EXE=build\hr-daemon.exe"
if not exist "%EXE%" (
    echo [run] %EXE% not found -- build it first:
    echo        cmd /c daemon\build.cmd
    exit /b 1
)

echo [run] %CD%\%EXE% %*
echo [run] log file: %CD%\build\hr-daemon.log
echo [run] Ctrl+C to stop.
echo.

"%EXE%" %*
set "RC=%ERRORLEVEL%"
echo.
echo [run] hr-daemon exited with code %RC%
endlocal & exit /b %RC%
