@echo off
title Install String
rem Installs String for this Windows user from the unzipped String.zip (the work is done by app\setup\install.ps1).
if not exist "%~dp0app\String.exe" goto unzip
rem Windows PowerShell finds its own modules again if another PowerShell left its module path behind
set "PSModulePath="
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0app\setup\install.ps1"
if errorlevel 1 goto stop
timeout /t 4 >nul
exit /b 0

:unzip
echo.
echo   Unzip String.zip first: right-click it, choose "Extract All...", then "Extract".
echo   Then open "Install String" from the folder that appears.
echo.
:stop
pause
exit /b 1
