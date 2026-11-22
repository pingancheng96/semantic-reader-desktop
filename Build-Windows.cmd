@echo off
setlocal
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-windows-local.ps1"
set "BUILD_RESULT=%ERRORLEVEL%"
echo.
if not "%BUILD_RESULT%"=="0" echo Build failed. See build-windows.log and the messages above.
pause
exit /b %BUILD_RESULT%
