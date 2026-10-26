#!/bin/sh
# Builds target/release/Piclecor.app. With --install, also copies it to /Applications.
set -eu

install_dir=/Applications
install=false
[ "${1:-}" = "--install" ] && install=true

root=$(cd "$(dirname "$0")/.." && pwd)
app="$root/target/release/Piclecor.app"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)

cargo build --release --manifest-path "$root/Cargo.toml"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/piclecor" "$app/Contents/MacOS/piclecor"
cp "$root/assets/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Piclecor</string>
    <key>CFBundleDisplayName</key><string>Piclecor</string>
    <key>CFBundleIdentifier</key><string>app.piclecor</string>
    <key>CFBundleExecutable</key><string>piclecor</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

# Ad-hoc signature so Gatekeeper runs the local build.
codesign --force --sign - "$app" >/dev/null 2>&1 || true

if $install; then
    # Quit a running copy so the replaced binary takes effect on next launch.
    pkill -x piclecor 2>/dev/null || true
    rm -rf "$install_dir/Piclecor.app"
    cp -R "$app" "$install_dir/"
    # Refresh Launch Services so Finder and Spotlight show the new icon.
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$install_dir/Piclecor.app"
    echo "$install_dir/Piclecor.app"
else
    echo "$app"
fi
