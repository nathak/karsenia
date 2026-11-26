# Building the Windows program on Linux

`karsenia-desktop.exe` is cross-compiled for `x86_64-pc-windows-gnu` with `zig cc` as the C compiler and the linker (zig 0.17.0, from the
`ziglang` Python package); there is no mingw-w64 install. The two scripts here are what rustc and the cc crate call:

```sh
export ZIG="$(python3 -c 'import ziglang,os;print(os.path.join(os.path.dirname(ziglang.__file__),"zig"))')"
export PATH="$PWD/packaging/windows-build:$PATH"            # finds x86_64-w64-mingw32-dlltool
export CC_x86_64_pc_windows_gnu="$PWD/packaging/windows-build/zig-cc.sh"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$PWD/packaging/windows-build/zig-cc.sh"
cargo fetch --locked
cargo build --release --target x86_64-pc-windows-gnu --features desktop --bin karsenia-desktop --locked
packaging/make-windows-zip.sh <version> <commit> target/x86_64-pc-windows-gnu/release/karsenia-desktop.exe dist
```

`.github/workflows/release.yml` does exactly this for a tag `vX.Y.Z`.
