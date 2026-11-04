#!/bin/sh
# Builds RedForge's Linux releases into target/dist/: binaries that run on
# glibc 2.31 and newer (built with cargo-zigbuild, which needs zig), then the
# tarball the AUR's redforge-bin installs, the .deb (cargo-deb) and the
# AppImage (appimagetool). `build.sh tarball|deb|appimage` makes only one.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
target=x86_64-unknown-linux-gnu
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
bin="$root/target/$target/release"
dist="$root/target/dist"
cd "$root"
mkdir -p "$dist"

cargo zigbuild --release --locked --target "$target.2.31" -p rf-app -p rf-cli

# The files as the Linux packages lay them out, under $1.
stage() {
  install -Dm755 "$bin/redforge" "$1/usr/bin/redforge"
  install -Dm755 "$bin/rf" "$1/usr/bin/rf"
  strip "$1/usr/bin/redforge" "$1/usr/bin/rf"
  install -Dm644 data/rdr2/strings.txt "$1/usr/share/redforge/data/rdr2/strings.txt"
  for f in blender/redforge_bridge/*.py blender/redforge_bridge/*.toml; do
    install -Dm644 "$f" "$1/usr/share/redforge/$f"
  done
  install -Dm644 packaging/linux/redforge.desktop "$1/usr/share/applications/redforge.desktop"
  install -Dm644 packaging/linux/redforge-mime.xml "$1/usr/share/mime/packages/redforge.xml"
  for size in 16 24 32 48 64 96 128 256 512; do
    install -Dm644 "packaging/linux/icons/redforge-$size.png" \
      "$1/usr/share/icons/hicolor/${size}x${size}/apps/redforge.png"
    install -Dm644 "packaging/linux/icons/redforge-project-$size.png" \
      "$1/usr/share/icons/hicolor/${size}x${size}/mimetypes/application-x-redforge-project.png"
  done
  for f in README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE; do
    install -Dm644 "$f" "$1/usr/share/doc/redforge/$f"
  done
}

tarball() {
  tmp="$(mktemp -d)"
  stage "$tmp"
  tar -C "$tmp" -czf "$dist/redforge-$version-x86_64-linux.tar.gz" usr
  rm -rf "$tmp"
  echo "$dist/redforge-$version-x86_64-linux.tar.gz"
}

deb() {
  cargo deb -p rf-app --target "$target" --no-build --strip -o "$dist/"
}

appimage() {
  appdir="$dist/RedForge.AppDir"
  rm -rf "$appdir"
  stage "$appdir"
  cp packaging/linux/redforge.desktop "$appdir/redforge.desktop"
  cp packaging/linux/icons/redforge-256.png "$appdir/redforge.png"
  cat > "$appdir/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/redforge" "$@"
RUN
  chmod 755 "$appdir/AppRun"
  ARCH=x86_64 appimagetool --no-appstream "$appdir" "$dist/RedForge-$version-x86_64.AppImage"
  rm -rf "$appdir"
}

case "${1:-all}" in
  tarball) tarball ;;
  deb) deb ;;
  appimage) appimage ;;
  all) tarball; deb; appimage ;;
  *) echo "usage: $0 [tarball|deb|appimage|all]" >&2; exit 2 ;;
esac
