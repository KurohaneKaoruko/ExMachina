#!/usr/bin/env bash
# 桌面端打包(unix):组装 bundle 目录 → linux tar.gz / macos .app + dmg
# 用法:package-desktop.sh <bundle_dir> <app_bin> <gateway_bin> <artifact_name>
set -eu

BUNDLE="$1"
APP_BIN="$2"
GW_BIN="$3"
ARTIFACT="$4"

rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/bin" "$BUNDLE/resources/webui" "$BUNDLE/resources/data-seed"

# 主程序 + 网关侧车
cp "$APP_BIN" "$BUNDLE/"
if [ "$(uname)" = "Darwin" ]; then
  # 可执行权限由安装器保留;网关放 bin/
  cp "$GW_BIN" "$BUNDLE/bin/"
else
  cp "$GW_BIN" "$BUNDLE/bin/"
fi
chmod +x "$BUNDLE/exmachina-desktop" "$BUNDLE/bin/"* 2>/dev/null || true

# 图标 + WebUI(高级控制台)+ 数据播种源
cp desktop/icons/icon.ico desktop/icons/icon.png "$BUNDLE/" 2>/dev/null || true
cp -r webui/dist "$BUNDLE/resources/webui/dist"
cp -r entities "$BUNDLE/resources/data-seed/entities"
cp -r skills "$BUNDLE/resources/data-seed/skills"
cp entities/default-soul.md "$BUNDLE/resources/data-seed/default-soul.md"

# 启动脚本:以 bundle 为工作目录运行(数据/资源相对定位)
cat > "$BUNDLE/start.sh" << 'EOS'
#!/usr/bin/env bash
cd "$(dirname "$0")"
./exmachina-desktop "$@"
EOS
chmod +x "$BUNDLE/start.sh"

case "$ARTIFACT" in
  *.tar.gz)
    tar -czf "$ARTIFACT" -C "$BUNDLE" .
    ;;
  *.dmg)
    # macOS:组装最小 .app 结构,再打成 dmg
    APP="EXMACHINA.app"
    rm -rf "$APP"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    cp -r "$BUNDLE/"* "$APP/Contents/MacOS/"
    cat > "$APP/Contents/Info.plist" << 'EOPL'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>EXMACHINA</string>
  <key>CFBundleIdentifier</key><string>io.exmachina.desktop</string>
  <key>CFBundleExecutable</key><string>exmachina-desktop</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOPL
    cp desktop/icons/icon.png "$APP/Contents/Resources/icon.png"
    hdiutil create -volname "EXMACHINA" -srcfolder "$APP" -ov -format UDZO "$ARTIFACT"
    ;;
  *)
    echo "未知产物类型: $ARTIFACT" >&2
    exit 1
    ;;
esac
echo "打包完成: $ARTIFACT"
