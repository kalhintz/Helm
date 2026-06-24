@echo off
rem Launch Helm (native Tauri release build).
rem If the window is blank/white, the WebView2 cache is stale: delete
rem "%LOCALAPPDATA%\com.helm.app" and relaunch.
start "" "%~dp0src-tauri\target\release\helm.exe"
