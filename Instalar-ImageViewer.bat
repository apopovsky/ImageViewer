@echo off
setlocal
set "EXE_PATH=%~dp0target\release\ImageViewer.exe"
if not exist "%EXE_PATH%" (
    if exist "%~dp0ImageViewer.exe" (
        set "EXE_PATH=%~dp0ImageViewer.exe"
    ) else (
        echo ImageViewer.exe not found. Build first with: cargo build --release -j 2
        pause
        exit /b 1
    )
)
start "" "%EXE_PATH%" --setup
