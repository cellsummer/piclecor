#!/bin/sh
# Regenerates assets/AppIcon.icns from assets/icon.svg.
# Each size renders from the SVG so small sizes stay sharp.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
set_dir=$(mktemp -d)/AppIcon.iconset
mkdir -p "$set_dir"

render() {
    swift "$root/scripts/render-svg.swift" "$root/assets/icon.svg" "$set_dir/$1" "$2"
}

for size in 16 32 128 256 512; do
    render "icon_${size}x${size}.png" "$size"
    render "icon_${size}x${size}@2x.png" "$((size * 2))"
done

iconutil -c icns "$set_dir" -o "$root/assets/AppIcon.icns"
echo "$root/assets/AppIcon.icns"
