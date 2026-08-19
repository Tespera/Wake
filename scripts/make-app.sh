#!/bin/zsh
# 构建 Wake.app:release 二进制 + icns 图标 + Info.plist,ad-hoc 签名。
# 用法: scripts/make-app.sh [--run]
set -euo pipefail
cd "$(dirname "$0")/.."

ASSETS=crates/wake/assets
BUILD=target/release
APP=dist/Wake.app

# 1. 图标:svg → 1024 png(ImageIO 渲染,保透明;qlmanage 会把透明边距填成白底) → iconset → icns
if [ ! -f "$ASSETS/icon-1024.png" ] || [ "$ASSETS/icon.svg" -nt "$ASSETS/icon-1024.png" ]; then
  swift scripts/render-icon.swift "$ASSETS/icon.svg" "$ASSETS/icon-1024.png" 1024
fi
ICONSET=$(mktemp -d)/wake.iconset
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size "$ASSETS/icon-1024.png" --out "$ICONSET/icon_${size}x${size}.png" > /dev/null
  double=$((size * 2))
  sips -z $double $double "$ASSETS/icon-1024.png" --out "$ICONSET/icon_${size}x${size}@2x.png" > /dev/null
done
mkdir -p dist
iconutil -c icns "$ICONSET" -o dist/wake.icns

# 2. release 构建
cargo build --release -p wake

# 3. bundle 组装
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BUILD/Wake" "$APP/Contents/MacOS/Wake"
cp dist/wake.icns "$APP/Contents/Resources/wake.icns"
# 版本号从 workspace Cargo.toml 单一来源读取,勿在此硬编码
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Wake</string>
    <key>CFBundleDisplayName</key><string>Wake</string>
    <key>CFBundleIdentifier</key><string>dev.corey.wake</string>
    <key>CFBundleExecutable</key><string>Wake</string>
    <key>CFBundleIconFile</key><string>wake</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <!-- 恢复会话要驱动 Terminal/iTerm,删除会话要驱动 Finder。缺这个键,
         macOS 的自动化授权框没有说明文字,启用 hardened runtime 后更会被
         TCC 直接拒绝——两个核心功能都会在别人的机器上静默失效。 -->
    <key>NSAppleEventsUsageDescription</key>
    <string>Wake uses automation to reopen sessions in your terminal and to move session files to the Trash.</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>© 2026 Corey Chiu · MIT License</string>
</dict>
</plist>
PLIST

# 4. ad-hoc 签名(本机运行足够;分发需开发者证书 + notarize)
codesign --force --deep -s - "$APP"

echo "✓ $APP"
if [ "${1:-}" = "--run" ]; then
  open "$APP"
fi
