@echo off
set MSYS_NO_PATHCONV=1
set MSYS2_ARG_CONV_EXCL=*
set PATH=C:\msys64\mingw64\bin;%PATH%
cd /d "C:\Users\ljm37\DeepSeek Tui"
set CARGO_TARGET_DIR=C:\cargo_target
cargo check -p deepseek-tui