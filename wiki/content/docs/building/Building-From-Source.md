---
title: Building From Source
weight: 1
---

From a checkout of the repository to a module folder that imports in PowerShell 7 and in Windows PowerShell 5.1. Installing from the PowerShell Gallery, as [Getting Started](Getting-Started.md) does, needs none of this. Source: `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml`, `src/lib.rs`.

## What the build needs

- Windows x64 or Arm64. A build makes the native library for the machine it runs on, `win-x64` or `win-arm64`. The module has been built and its suite run on x64, as [Where It Has Been Run](Where-It-Has-Been-Run.md) reports, and on Arm64 by the repository's CI; the package joins the two module folders with `cargo pwrs merge`.
- Rust 1.98 or later with the MSVC toolchain. `Cargo.toml` declares `rust-version = "1.98"`, so Cargo refuses to build the crate, which is written in Rust's 2024 edition, with an older Rust.
- [PoWerRuSt](https://crates.io/crates/PoWerRuSt) 0.3.0 from crates.io, which `Cargo.toml` names and `Cargo.lock` pins, and cargo-pwrs 0.3.0, the tool of the same release, which `cargo install cargo-pwrs --version 0.3.0 --locked` installs. PWRS's [Getting Started](https://variably-constant.github.io/PWRS/docs/tutorials/getting-started/) lists what cargo-pwrs needs beside Rust.
- On x64, LLVM's `clang-cl` on `PATH`, which `.cargo/config.toml` names as the C compiler of libzstd. The Arm64 build uses the toolchain's own C compiler.

## Build

From the repository's root:

```text
cargo install cargo-pwrs --version 0.3.0 --locked
cargo pwrs build --release
```

The module folder is `target\pwrs\Zstd`, under `CARGO_TARGET_DIR` when that is set. Beside the manifest, `Zstd.psd1`, and `Zstd.psm1`, it holds the managed assemblies for PowerShell 7 under `net10.0` and for Windows PowerShell 5.1 under `netstandard2.0`, each with the cmdlets' help; the native library under `runtimes\win-x64\native` or `runtimes\win-arm64\native`; and two `THIRD-PARTY-NOTICES.txt` files, one at its root for the assemblies PWRS compiles, and one beside the native library's folder with the license and license files of each crate compiled into it.

`.cargo/config.toml` makes every x64 build of the repository the x86-64 baseline, and links the C runtime into the native library on x64 and Arm64 alike, so the module needs no Visual C++ Redistributable. It names `clang-cl` as the C compiler of libzstd, with `-fgnuc-version`: under that flag clang-cl defines `__GNUC__`, and libzstd compiles its BMI2 code paths beside the baseline ones and picks them at run time on a CPU that has BMI2, while the library stays the x86-64 baseline. `cl.exe` compiles no BMI2 path. A `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` variable replaces the file's rustc flags, and a `CC_x86_64_pc_windows_msvc` or `CFLAGS_x86_64_pc_windows_msvc` variable replaces its compiler or its flag, so a build leaves them unset. The zstd crate compiles libzstd 1.5.7 with its worker threads and its dictionary trainer, and without its decoders for the formats from before zstd 1.0.

`cargo pwrs test --release` runs the Rust unit tests, builds the module, and runs `tests/Zstd.Tests.ps1` in both hosts; see [How To Run The Tests](How-To-Run-The-Tests.md).

## Import the folder

```powershell
Import-Module .\target\pwrs\Zstd\Zstd.psd1
Get-Command -Module Zstd
```

```text
CommandType     Name                                               Version    Source
-----------     ----                                               -------    ------
Cmdlet          Compress-Zstd                                      0.1.0      Zstd
Cmdlet          Expand-Zstd                                        0.1.0      Zstd
Cmdlet          New-ZstdDictionary                                 0.1.0      Zstd
```

The same folder imports into Windows PowerShell 5.1. From here the module behaves as the installed one does, and [Getting Started](Getting-Started.md) goes on from its first command.
