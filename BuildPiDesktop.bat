@echo off
rem ================================================================
rem  Pi Desktop - one-click rebuild (double-click to run)
rem  Rebuilds the NSIS installer from current sources.
rem  Keep this window open; when "BUILD OK" appears the output
rem  folder opens automatically.
rem ================================================================
setlocal
cd /d "%~dp0"
echo.
echo  ==================================================
echo   Pi Desktop one-click build
echo   Checking environment ...
echo  ==================================================
echo.
powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\build.ps1"
if errorlevel 1 (
  echo.
  echo  BUILD FAILED - see messages above.
  pause
  exit /b 1
)
echo.
echo  DONE: installer created, output folder opened.
echo  Note: first run downloads dependencies / NSIS tools,
echo        please wait a few minutes.
echo.
pause
endlocal
