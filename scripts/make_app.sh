#!/usr/bin/env bash
# 바탕화면에 mytripo_public.app 을 만든다. 실행 파일을 번들에 복사한다(레포가 외장 볼륨에 있어도 앱이 돈다).
# 번들이 있으면 verify.sh 가 끝에서 이 스크립트를 다시 돌려 번들을 새 코드로 바꾼다.
# 사용: bash scripts/make_app.sh [넣을 폴더, 기본 ~/Desktop]
set -euo pipefail
cd "$(dirname "$0")/.."

id="local.mytripo-public"
dest="${1:-$HOME/Desktop}"
app="$dest/mytripo_public.app"

# 같은 이름의 다른 앱은 지우지 않는다.
if [ -e "$app" ]; then
  old=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$app/Contents/Info.plist" 2>/dev/null || true)
  if [ "$old" != "$id" ]; then
    echo "make_app: $app 이 이미 있고 mytripo_public 번들이 아니다. 멈춘다." >&2
    exit 1
  fi
fi

cargo build --release
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
new="$work/mytripo_public.app"
mkdir -p "$new/Contents/MacOS" "$new/Contents/Resources" "$work/AppIcon.iconset"

for s in 16 32 128 256 512; do
  sips -z "$s" "$s" assets/icon.png --out "$work/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" assets/icon.png --out "$work/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$work/AppIcon.iconset" -o "$new/Contents/Resources/AppIcon.icns"

cp target/release/mytripo_public "$new/Contents/MacOS/mytripo_public"
cat >"$new/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>mytripo_public</string>
  <key>CFBundleDisplayName</key><string>mytripo_public</string>
  <key>CFBundleIdentifier</key><string>$id</string>
  <key>CFBundleExecutable</key><string>mytripo_public</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
</dict>
</plist>
PLIST
plutil -lint "$new/Contents/Info.plist" >/dev/null
# Apple Development 인증서가 있으면 그것으로 서명한다. 서명 주체가 빌드마다 같아서 macOS 가 문서 폴더 허용을 기억한다.
# 없으면 ad-hoc 서명이고, 다시 만들 때마다 macOS 가 허용을 다시 묻는다.
sign=$(security find-identity -v -p codesigning 2>/dev/null | awk '/Apple Development/ {print $2; exit}')
codesign --force --sign "${sign:--}" "$new"

rm -rf "$app"
mv "$new" "$app"
touch "$app"
echo "app $app"
