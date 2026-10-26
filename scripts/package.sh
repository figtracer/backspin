#!/bin/sh
# Builds the macOS installer: target/Backspin-<version>-<arch>.pkg installs Backspin.vst3 and
# Backspin.clap into /Library/Audio/Plug-Ins. Honours CARGO_TARGET_DIR.
set -eu
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
arch=$(uname -m)
target=${CARGO_TARGET_DIR:-target}
work=$target/pkg
plugins=$work/root/Library/Audio/Plug-Ins

cargo xtask bundle backspin --release

rm -rf "$work"
mkdir -p "$plugins/VST3" "$plugins/CLAP"
cp -R "$target/bundled/Backspin.vst3" "$plugins/VST3/"
cp -R "$target/bundled/Backspin.clap" "$plugins/CLAP/"
# Drop extended attributes such as quarantine (the payload keeps them as AppleDouble entries).
xattr -cr "$work/root"

# Install to the fixed paths even when another copy of the bundle exists elsewhere.
pkgbuild --analyze --root "$work/root" "$work/components.plist"
i=0
while plutil -extract "$i" xml1 -o /dev/null "$work/components.plist" 2>/dev/null; do
    plutil -replace "$i.BundleIsRelocatable" -bool NO "$work/components.plist"
    i=$((i + 1))
done

pkgbuild \
    --root "$work/root" \
    --component-plist "$work/components.plist" \
    --identifier com.figtracer.backspin \
    --version "$version" \
    --install-location / \
    "$work/backspin.pkg"
productbuild --package "$work/backspin.pkg" "$target/Backspin-$version-$arch.pkg"
echo "$target/Backspin-$version-$arch.pkg"
