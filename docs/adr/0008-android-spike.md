# ADR 0008：Android 客户端方案与验证状态（未验证）

- 状态：**方案确定，验证未完成**（2026-10-24）
- 关联：`docs/specs/phase4-clients.md`、`design.md` §11 Phase 4

## 背景

Phase 4 要求「Android 真机安装 + 旧版本升级，记录 commit/build ID」与「Keystore 可用」。
当前尚无 APK 构建与真机验证，因此发布检查该项保持未通过；本 ADR 记录方案与验证前置条件。

## 选择

1. **Tauri 2 mobile**：与桌面共用 `web/` SPA 与 `apps/desktop` 的 Rust 胶水；系统 WebView 渲染。
2. **Rust 核心复用**：`cista-core` 通过 JNI/FFI 编进 APK（`aarch64-linux-android`、`armv7-linux-androideabi`），
   加密（XChaCha20-Poly1305）、B+ 签名、同步状态机与桌面完全同一份代码。
3. **密钥存放**：设备私钥与 MK 进 Android Keystore（`tauri-plugin-*` 的 Keystore 封装），
   恢复码仍由用户离线保存；不允许把 MK 写进普通 SharedPreferences。
4. **服务端零改动**：Android 只是 B+ 的另一个设备；分享/只读/配额规则不变。

## 前置条件（缺一不可）

1. **网络**：构建链依赖 Google 的制品仓库，受限网络必须放行
   `dl.google.com`、`maven.google.com`、`repo1.maven.org`、`plugins.gradle.org`、`services.gradle.org`
   （否则 `cmdline-tools`/NDK 无法获取）。
2. **磁盘**：构建机 ≥8 GB 空闲（NDK 解压 + `aarch64-linux-android` 全量 Rust 构建 + Gradle 缓存）。
3. **工具链**：
   ```bash
   sudo apt-get install -y openjdk-17-jdk unzip
   # cmdline-tools-latest 解压到 $HOME/android-sdk/cmdline-tools/latest
   yes | sdkmanager --licenses
   sdkmanager "platform-tools" "platforms;android-34" "build-tools;34.0.0" "ndk;26.3.11579264"
   rustup target add aarch64-linux-android armv7-linux-androideabi
   cargo install cargo-ndk --locked
   ```
4. **构建**：`pnpm -C web build && pnpm -C apps/desktop tauri android init` →
   `pnpm -C apps/desktop tauri android build --apk`（或 `cargo tauri android build`）。

## 已验证（构建环境，2026-10-25）

- 放行 `dl.google.com`/`maven.google.com`/`repo1.maven.org`/`plugins.gradle.org` 后，
  `sdkmanager` 可装 `platform-tools`、`platforms;android-34`、`build-tools;34.0.0`、`ndk;26.3.11579264`
  （合计约 2.5 GB）。
- `rustup target add aarch64-linux-android` 成功；用 NDK 的
  `aarch64-linux-android21-clang` 作为 linker/CC、`llvm-ar` 作为 AR，
  **`cargo build -p cista-core --target aarch64-linux-android --release` 通过**
  （`libcista_core.rlib`，1.8 MB）——即加密/签名/同步核心可直接进 APK。
- **未做**：`tauri android init/build --apk`。原因：Tauri 依赖全量交叉编译 + Gradle 首次下载
  需要额外 4–6 GB 磁盘，当前构建机余量不足；真机安装/升级更需设备。

## 验收证据（缺一不可）

- `adb install -r` 到真机；`adb shell dumpsys package com.cista.notes` 的 `versionName`；
- 旧版本 → 新版本升级后 MK 仍可解笔记（Keystore 别名稳定）；
- 冷启动时间、离线打开笔记、分享链接在系统浏览器打开；
- 记录 commit 与 build ID。

验收结果按发布流程留档，不写入仓库。

## 后果

- 发布检查 P10（Android 真机 + 升级）**保持未通过**，直到上面证据齐备；不得用「编译通过」代替。
- 风险：NDK 26 与 AGP/Tauri 2.12 的兼容性未验证（核心交叉编译已验证）；Gradle 首次下载耗时且受网络策略影响。
- 回滚：不引入 Android 特有代码路径，删除 `apps/desktop/src-tauri/gen/android` 即可回到仅桌面/Web。
