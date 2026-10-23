#!/usr/bin/env bash
# Builds an AppImage into target/appimage/. Pass --install to copy it to ~/Applications.
# GTK >= 4.14, libadwaita >= 1.5 and pdftoppm are used from the host system, not bundled.
set -euo pipefail

cd "$(dirname "$0")/.."
APP_ID=io.github.fassbinb.SponsoringSlideshow
BIN=sponsoring-slideshow
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
OUT=target/appimage
APPDIR=$OUT/AppDir
TOOL=$OUT/appimagetool-x86_64.AppImage
IMAGE=$OUT/SponsoringSlideshow-x86_64.AppImage

cargo build --release

rm -rf "$APPDIR"
install -Dm755 "target/release/$BIN" "$APPDIR/usr/bin/$BIN"
install -Dm644 "packaging/$APP_ID.desktop" "$APPDIR/usr/share/applications/$APP_ID.desktop"
install -Dm644 "packaging/$APP_ID.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
ln -s "usr/share/applications/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
ln -s "usr/share/icons/hicolor/scalable/apps/$APP_ID.svg" "$APPDIR/$APP_ID.svg"
ln -s "$APP_ID.svg" "$APPDIR/.DirIcon"
cat > "$APPDIR/AppRun" <<EOF
#!/bin/sh
exec "\$(dirname "\$(readlink -f "\$0")")/usr/bin/$BIN" "\$@"
EOF
chmod +x "$APPDIR/AppRun"

if [[ ! -x $TOOL ]]; then
    curl -fsSL -o "$TOOL" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$TOOL"
fi

ARCH=x86_64 VERSION="$VERSION" APPIMAGE_EXTRACT_AND_RUN=1 \
    "$TOOL" --no-appstream "$APPDIR" "$IMAGE"

if [[ ${1:-} == --install ]]; then
    install -Dm755 "$IMAGE" -t ~/Applications
    echo "Installed to ~/Applications/$(basename "$IMAGE")"
fi
