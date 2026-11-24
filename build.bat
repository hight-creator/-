@echo off
chcp 65001 >nul
cd /d "%~dp0"

where cargo >nul 2>&1
if errorlevel 1 (
  echo [エラー] Rust が見つかりません。README の「1. Windows 10 での環境構築」を先に実行してください。
  pause
  exit /b 1
)

echo テストを実行します...
cargo test --release
if errorlevel 1 (
  echo.
  echo [エラー] テストに失敗しました。上のメッセージを確認してください。
  pause
  exit /b 1
)

echo.
echo ビルドします（初回は数分かかります）...
cargo build --release
if errorlevel 1 (
  echo.
  echo [エラー] ビルドに失敗しました。上のメッセージを確認してください。
  pause
  exit /b 1
)

copy /y "target\release\ig-story-dl.exe" "ig-story-dl.exe" >nul
echo.
echo 完了: %~dp0ig-story-dl.exe
echo このexe 1つだけを好きなフォルダに置いて使えます（config.json は同じフォルダに作られます）。
pause
