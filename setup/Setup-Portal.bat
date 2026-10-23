@echo off
rem Windows: double-click to set up a connected Portal for Porchlight.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1"
echo.
pause
