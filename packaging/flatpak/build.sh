#!/bin/sh
# Builds the Flatpak into a bundle, target/dist/RedForge-<version>.flatpak
# (install it with `flatpak install --user <bundle>`). Needs flatpak-builder;
# the runtime, SDK and Rust extension come from Flathub.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
app="$(sed -n 's/^app-id: //p' "$root/packaging/flatpak/redforge.yml")"
cd "$root"
mkdir -p target/dist target/flatpak
flatpak-builder --user --install-deps-from=flathub --force-clean --repo=target/flatpak/repo \
  --state-dir=target/flatpak/state target/flatpak/build packaging/flatpak/redforge.yml
flatpak build-bundle target/flatpak/repo "target/dist/RedForge-$version.flatpak" "$app"
echo "target/dist/RedForge-$version.flatpak"
