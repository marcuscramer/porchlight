@echo off
rem Windows: double-click to undo Porchlight's system changes on a connected Portal.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" -Restore
echo.
pause
