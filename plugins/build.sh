#!/usr/bin/env sh
# Rebuild every plugin component and refresh plugins/prebuilt/.
#
# Needs wasi-sdk 34 (docs/wasm-toolchain.md) for the tree-sitter grammars,
# which are C. Point WASI_SDK_PATH at it.
set -eu
cd "$(dirname "$0")"
: "${WASI_SDK_PATH:?set WASI_SDK_PATH to a wasi-sdk 34 directory}"
export CC_wasm32_wasip2="$WASI_SDK_PATH/bin/clang"
export AR_wasm32_wasip2="$WASI_SDK_PATH/bin/llvm-ar"
export CFLAGS_wasm32_wasip2="--sysroot=$WASI_SDK_PATH/share/wasi-sysroot"
root="$(cd .. && pwd)"
export RUSTFLAGS="--remap-path-prefix=$root=/ra --remap-path-prefix=$HOME=/home"
: "${CARGO_BUILD_JOBS:=3}"
export CARGO_BUILD_JOBS
mkdir -p prebuilt
out=target/wasm32-wasip2/release

cargo build --release --target wasm32-wasip2 -p probe
cp "$out/probe.wasm" prebuilt/probe.wasm
cargo build --release --target wasm32-wasip2 -p probe3
cp "$out/probe3.wasm" prebuilt/probe3.wasm
cargo build --release --target wasm32-wasip2 -p probe4
cp "$out/probe4.wasm" prebuilt/probe4.wasm
for lang in rust ecmascript python csharp go java; do
  cargo build --release --target wasm32-wasip2 -p treesitter --features "$lang"
  cp "$out/treesitter.wasm" "prebuilt/$lang.wasm"
done
# The bodies of C# members (contract 0.3): the same crate, its own component.
cargo build --release --target wasm32-wasip2 -p treesitter --features csharp-body
cp "$out/treesitter.wasm" prebuilt/csharp-body.wasm
# Built like a third party's plugin: its own workspace, the published WIT only.
(cd ../examples/plugin-sql && cargo build --release --target wasm32-wasip2)
cp ../examples/plugin-sql/target/wasm32-wasip2/release/plugin_sql.wasm prebuilt/sql.wasm

(cd prebuilt && sha256sum *.wasm | sort -k2 > SHA256SUMS && cat SHA256SUMS)
