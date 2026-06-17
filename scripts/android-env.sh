#!/bin/bash
# Android 构建环境变量。用法：source scripts/android-env.sh && pnpm tauri android dev
# 设置 Tauri Mobile 必需的 JDK 17 + Android SDK/NDK 路径。

export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/25.1.8937393"
export JAVA_HOME="/usr/local/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home"
export PATH="$JAVA_HOME/bin:$PATH"

echo "Android env set: JDK $($JAVA_HOME/bin/java -version 2>&1 | head -1 | awk '{print $3}'), NDK 25.1.8937393"
