#!/usr/bin/env bash
# Build a portable AppImage from a release-built workspace.
# Expects: target/release/{protondrive,protondrived,protondrive-cli} already built.
# Produces: protondrive-linux-<VERSION>-x86_64.AppImage in $PWD.
set -euo pipefail

VERSION="${1:?usage: build-appimage.sh <version>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="${WORK:-$ROOT/dist/AppDir}"

rm -rf "$WORK"
mkdir -p "$WORK/usr/bin" "$WORK/usr/share/applications" "$WORK/usr/share/metainfo" \
         "$WORK/usr/share/icons/hicolor/scalable/apps" "$WORK/usr/lib"

install -Dm0755 "$ROOT/target/release/protondrive"      "$WORK/usr/bin/protondrive"
install -Dm0755 "$ROOT/target/release/protondrived"     "$WORK/usr/bin/protondrived"
install -Dm0755 "$ROOT/target/release/protondrive-cli"  "$WORK/usr/bin/protondrive-cli"

install -Dm0644 "$ROOT/packaging/desktop/me.proton.drive.Linux.desktop" \
                "$WORK/usr/share/applications/me.proton.drive.Linux.desktop"
install -Dm0644 "$ROOT/packaging/desktop/me.proton.drive.Linux.metainfo.xml" \
                "$WORK/usr/share/metainfo/me.proton.drive.Linux.metainfo.xml"
install -Dm0644 "$ROOT/data/icons/me.proton.drive.Linux.svg" \
                "$WORK/usr/share/icons/hicolor/scalable/apps/me.proton.drive.Linux.svg"

cp "$WORK/usr/share/applications/me.proton.drive.Linux.desktop" "$WORK/me.proton.drive.Linux.desktop"
cp "$WORK/usr/share/icons/hicolor/scalable/apps/me.proton.drive.Linux.svg" "$WORK/me.proton.drive.Linux.svg"
ln -sf me.proton.drive.Linux.svg "$WORK/.DirIcon"

cat > "$WORK/AppRun" <<'EOF'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HERE/usr/lib:${LD_LIBRARY_PATH:-}"
exec "$HERE/usr/bin/protondrive" "$@"
EOF
chmod +x "$WORK/AppRun"

linuxdeploy --appdir "$WORK" \
    --plugin gtk \
    --output appimage \
    --desktop-file "$WORK/me.proton.drive.Linux.desktop" \
    --icon-file    "$WORK/me.proton.drive.Linux.svg"

mv ./*-x86_64.AppImage "protondrive-linux-${VERSION}-x86_64.AppImage"
echo "built: protondrive-linux-${VERSION}-x86_64.AppImage"
