# 平台状态与发布步骤

## 各端状态

| 端 | 用户端代码 | 构建/打包 | 说明 |
|---|---|---|---|
| 服务端 | ✅ 完成 | ✅ 自动（Linux/Windows） | 认证、笔记、分享、Yjs、归档、限速、备份/恢复脚本 |
| Web | ✅ 完成 | ✅ 自动（`web/dist`） | 分享页、配对、笔记编辑器、附件 |
| Linux 桌面 | ✅ 完成 | ✅ 自动（deb + AppImage） | 同一 SPA + Tauri 壳；钥匙串（Secret Service） |
| Windows 桌面 | ✅ 完成 | ✅ 自动（nsis 安装包） | 同一 SPA + Tauri 壳；钥匙串（Credential Manager） |
| Android | ⚠️ 可运行（Keystore 未适配） | ✅ CI 构建 APK（`android-apk`） | 会话内可用；刷新后需重新入网，见下 |

## Android 还差什么

1. ~~`tauri android init` + `tauri android build --apk`~~ **已接入 CI**（`android-apk` job）；
2. 把 Android 分支的 `secret_save/load/delete`（当前明确返回「未实现」）换成 Android Keystore；
3. 真机 `adb install`、旧版本升级、Keystore 别名稳定性验证。

核心与壳已能编译；第 2 步是平台适配，第 3 步需要设备。

## 发布步骤

1. `git push` 到托管平台（如 `origin/main`）。
2. 确认 Actions：`.github/workflows/ci.yml`（verify）与 `build.yml`（产物）全绿。
3. `git tag v0.1.0 && git push origin v0.1.0`。
4. `build.yml` 的 release job 自动把产物挂到 Release：
   `cista-server`（linux/windows）、`Cista_0.1.0_amd64.deb`、`Cista_0.1.0_amd64.AppImage`、
   `Cista_0.1.0_x64-setup.exe`、`web` 的静态包。
5. 自托管部署：按 `deploy/` 与 README 的部署小节配置 `CISTA_*` 环境变量，跑一次
   `scripts/backup.sh` 与 `scripts/restore-drill.sh`。

说明：Actions 里的产物是**未签名**构建；公开分发前建议补代码签名（Windows）与 Android 签名（keystore）。
