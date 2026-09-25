@echo off
setlocal EnableExtensions
cd /d "%~dp0"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

if /I "%~1"=="dev" goto dev
if /I "%~1"=="build" goto build
if /I "%~1"=="" goto dev
goto usage

:dev
echo [run] launching editor...
cargo run -p ezicode
goto :eof

:build
echo [run] release build...
cargo build --release -p ezicode
goto :eof

:usage
echo Usage: run [dev^|build]
exit /b 1
