@echo off
set SDK=D:\apps\DevEco Studio\app\sdk\default\openharmony\native
"%SDK%\llvm\bin\clang++" -target aarch64-linux-ohos --sysroot="%SDK%\sysroot" -D__MUSL__ %*
