#!/usr/bin/env python3
"""在 `tauri android init` 生成的 AndroidManifest.xml 上追加安全属性。

- `android:allowBackup="false"`：避免云备份/adb backup 把应用私有目录（含旧版明文密钥）带走。
- `android:usesCleartextTraffic="false"`：客户端只允许 https（本地开发可用 adb reverse + 显式配置）。

幂等：已存在同名属性时不改动。
"""
import pathlib
import re
import sys

MANIFEST = pathlib.Path("apps/desktop/src-tauri/gen/android/app/src/main/AndroidManifest.xml")


def add_attribute(tag: str, attr: str, value: str) -> str:
    if re.search(rf'{re.escape(attr)}="', tag):
        return tag
    return tag.replace("<application", f'<application {attr}="{value}"', 1)


def main() -> int:
    if not MANIFEST.exists():
        print(f"manifest not found: {MANIFEST}", file=sys.stderr)
        return 1
    text = MANIFEST.read_text(encoding="utf-8")
    match = re.search(r"<application\b[^>]*>", text)
    if not match:
        print("no <application> tag", file=sys.stderr)
        return 1
    tag = match.group(0)
    new_tag = add_attribute(tag, "android:allowBackup", "false")
    new_tag = add_attribute(new_tag, "android:usesCleartextTraffic", "false")
    if new_tag != tag:
        text = text.replace(tag, new_tag, 1)
        MANIFEST.write_text(text, encoding="utf-8")
        print("manifest hardened")
    else:
        print("manifest already hardened")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
