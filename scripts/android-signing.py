#!/usr/bin/env python3
"""把 release 签名配置注入 tauri 生成的 Android 工程（CI 用）。

环境变量：ANDROID_KEYSTORE_PATH / ANDROID_KEYSTORE_PASSWORD / ANDROID_KEY_ALIAS / ANDROID_KEY_PASSWORD
"""
import os
import pathlib
import sys

gradle = pathlib.Path("apps/desktop/src-tauri/gen/android/app/build.gradle.kts")
if not gradle.exists():
    sys.exit("gen/android 不存在：先跑 tauri android init")

password = os.environ["ANDROID_KEYSTORE_PASSWORD"]
key_password = os.environ.get("ANDROID_KEY_PASSWORD", password)
alias = os.environ["ANDROID_KEY_ALIAS"]
path = os.environ["ANDROID_KEYSTORE_PATH"]

snippet = (
    "    signingConfigs {\n"
    '        create("release") {\n'
    f'            storeFile = file("{path}")\n'
    f'            storePassword = "{password}"\n'
    f'            keyAlias = "{alias}"\n'
    f'            keyPassword = "{key_password}"\n'
    "        }\n"
    "    }\n"
)
text = gradle.read_text()
if "signingConfigs" not in text:
    text = text.replace("buildTypes {", snippet + "\n    buildTypes {", 1)
    text = text.replace(
        "release {",
        'release {\n            signingConfig = signingConfigs.getByName("release")',
        1,
    )
    gradle.write_text(text)
    print("signing config injected")
else:
    print("signing config already present")
