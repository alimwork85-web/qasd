@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo Installing Qasd...
qasd.exe install --login
echo.
echo Press Alt+Q anywhere to search.
pause
