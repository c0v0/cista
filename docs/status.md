# 平台状态与发布步骤

## 各端状态

| 端 | 用户端代码 | 构建/打包 | 说明 |
|---|---|---|---|
| 服务端 | ✅ | ✅ 自动（Linux/Windows） | 认证、笔记、分享、Yjs、归档、限速、备份/恢复脚本 |
| Web | ✅ | ✅ 自动（`web/dist`） | 分享页、配对、笔记编辑器、附件；首次启动可填服务器地址 |
| Linux 桌面 | ✅ | ✅ 自动（deb + AppImage） | 同一 SPA + Tauri 壳；钥匙串（Secret Service）；首次启动强制填服务器 |
| Windows 桌面 | ✅ | ✅ 自动（nsis 安装包） | 同一 SPA + Tauri 壳；钥匙串（Credential Manager）；首次启动强制填服务器 |
| Android | ✅ | ✅ CI 构建/签名 APK（`android-apk`） | AndroidKeyStore；模拟器安装/启动步骤通过 |

Android 密钥实现：`plugins/keystore`（Kotlin `AndroidKeyStore` + AES/GCM，密钥不可导出，密文存
SharedPreferences）；插件不可用时回退到应用私有目录（`cista-secrets.json`，0600）。

## 客户端服务器地址

- 实现：`web/src/pages/ServerSetupPage.tsx` + `web/src/lib/serverSettings.ts`
  （校验、探测 `/healthz`、localStorage 持久化）。
- 原生端（桌面/Android）未配置时必须先保存地址；Web 端默认同源，可改成跨域地址。
- 优先级：用户保存值 > 构建期 `VITE_API_BASE` > 同源。
- 换服务器 = 换身份：当前 session 会被锁定，需要重新配对。
- 测试：`web/src/lib/serverSettings.test.ts`（13 项）+ `web/src/App.test.tsx`（4 项）。

## 发布步骤

1. `git push` 到 `origin/main`。
2. 确认 Actions：`.github/workflows/ci.yml`（verify）与 `build.yml`（产物）全绿。
3. `git tag v0.1.1 && git push origin v0.1.1`。
4. `build.yml` 的 release job 自动把产物挂到 Release：
   `cista-server`（linux/windows）、`Cista_*_amd64.deb`、`Cista_*_amd64.AppImage`、
   `Cista_*_x64-setup.exe`、`web` 静态包。
5. 自托管部署：按 `deploy/` 与 README 的部署小节配置 `CISTA_*` 环境变量。

产物签名：Android APK 由 release keystore 签名；服务端与桌面安装包为未签名构建。
