#!/bin/sh
# Builds the Windows installer from Linux: the exes with cargo-xwin (the MSVC
# target), the MSI with wixl (msitools). The MSI goes to target/msi/.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
target=x86_64-pc-windows-msvc
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

cd "$root"
cargo xwin build --release --locked --target "$target" -p rf-app -p rf-cli
mkdir -p target/msi
wixl -a x64 \
  -D Version="$version" \
  -D Root="$root" \
  -D Bin="$root/target/$target/release" \
  -D Assets="$root/crates/rf-app/assets" \
  -o "target/msi/RedForge-$version-x64.msi" \
  packaging/windows/redforge.wxs
echo "target/msi/RedForge-$version-x64.msi"
