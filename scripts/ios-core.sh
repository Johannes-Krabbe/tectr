#!/bin/sh
# Builds the engine for the iOS app: the `ffi` crate for devices and the simulator, its Swift
# bindings, and the xcframework the TectrCore package links. None of it goes in git.
#
# PROFILE=dev builds faster, with a slower engine.
set -eu

cd "$(dirname "$0")/.."
# The command line tools have no iOS SDK, so Xcode's is used when they're selected.
if [ -z "${DEVELOPER_DIR:-}" ] && [ ! -d "$(xcode-select -p)/Platforms/iPhoneOS.platform" ]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi
profile="${PROFILE:-release}"
folder="$profile"
[ "$profile" = dev ] && folder=debug
core=apps/ios/TectrCore
work=target/ios-core

for target in aarch64-apple-ios aarch64-apple-ios-sim; do
    # The deployment target matches the app's, so the linker doesn't warn about every object.
    IPHONEOS_DEPLOYMENT_TARGET=26.0 cargo build -p ffi --lib --profile "$profile" --target "$target"
done

rm -rf "$work"
mkdir -p "$work/headers"
cargo run -q -p ffi --features bindgen --bin uniffi-bindgen -- generate \
    --library "target/aarch64-apple-ios/$folder/libtectr_ffi.a" \
    --language swift --out-dir "$work"
mv "$work/tectr_ffiFFI.h" "$work/headers/"
mv "$work/tectr_ffiFFI.modulemap" "$work/headers/module.modulemap"

rm -rf "$core/TectrFFI.xcframework"
xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/$folder/libtectr_ffi.a" -headers "$work/headers" \
    -library "target/aarch64-apple-ios-sim/$folder/libtectr_ffi.a" -headers "$work/headers" \
    -output "$core/TectrFFI.xcframework"
mkdir -p "$core/Sources/TectrCore/Generated"
mv "$work/tectr_ffi.swift" "$core/Sources/TectrCore/Generated/"
