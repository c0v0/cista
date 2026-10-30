# ADR 0008：Android 客户端方案

- 状态：接受（2026-10-26，已落地 CI 构建与模拟器验证）
- 关联：`docs/specs/phase4-clients.md`、`design.md` §11 Phase 4

## 背景

Android 与桌面共用同一套 React SPA；需要系统 WebView 渲染、复用 `cista-core` 的加密/签名/同步，
并把设备私钥放进系统安全存储。

## 选择

1. **Tauri 2 mobile**：与桌面共用 `web/` SPA 与 `apps/desktop` 的 Rust 胶水；系统 WebView 渲染。
2. **Rust 核心复用**：`cista-core` 交叉编译进 APK（`aarch64-linux-android`
   等 target），加密（XChaCha20-Poly1305）、B+ 签名、同步状态机与桌面完全同一份代码。
3. **密钥存放**：`plugins/keystore`（Kotlin `AndroidKeyStore`，AES/GCM，密钥不可导出，密文存
   SharedPreferences）；插件不可用时敏感键**失败关闭**，不回退明文/普通私有目录。旧版明文
   JSON 由启动时的 `purge_legacy_secrets` 清理。设备种子与可选的 MK 都走 AndroidKeyStore，
   不写普通 SharedPreferences 明文。
4. **服务端零改动**：Android 只是 B+ 的另一个设备；分享/只读/配额规则不变。

## 构建

CI（`.github/workflows/build.yml` 的 `android-apk` job）安装 JDK 17 + Android SDK/NDK，
`pnpm -C web build` → `tauri android build --apk` → `zipalign` + `apksigner` 用 release keystore 签名，
上传 `cista-android-apk` artifact；`android install check (emulator)` job 在模拟器上
`adb install` 并 `am start`。

本地构建前置：

```bash
sudo apt-get install -y openjdk-17-jdk unzip
sdkmanager "platform-tools" "platforms;android-36" "build-tools;36.0.0" "ndk;26.3.11579264"
rustup target add aarch64-linux-android
pnpm -C web build && pnpm -C apps/desktop tauri android build --apk
```

## 后果

- Android 与桌面/Web 共用客户端代码；密钥存 AndroidKeyStore，重启/升级不丢设备种子。
- 平台差异只存在于 `plugins/keystore` 与 `apps/desktop/src-tauri/src/lib.rs` 的
  `#[cfg(target_os = "android")]` 分支。
- 回滚：删除 `apps/desktop/src-tauri/gen/android` 即可回到仅桌面/Web。
