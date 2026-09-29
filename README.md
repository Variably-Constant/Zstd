<div align="center">

# Zstd

**Zstandard (zstd) compression with libzstd compiled in, built for Windows PowerShell 5.1 and PowerShell 7 with PoWerRuSt.**

[![PowerShell Gallery](https://img.shields.io/powershellgallery/v/Zstd?style=flat-square)](https://www.powershellgallery.com/packages/Zstd)
[![Wiki](https://img.shields.io/github/actions/workflow/status/Variably-Constant/Zstd/wiki-deploy.yml?branch=main&label=wiki&style=flat-square)](https://variably-constant.github.io/Zstd/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](https://github.com/Variably-Constant/Zstd/blob/main/LICENSE)
[![PowerShell](https://img.shields.io/badge/PowerShell-7.4%2B%20%7C%205.1-5391FE.svg?style=flat-square)](https://learn.microsoft.com/powershell/)

**[Wiki](https://variably-constant.github.io/Zstd/)** | [Getting started](https://variably-constant.github.io/Zstd/docs/tutorials/getting-started/) | [Reference](https://variably-constant.github.io/Zstd/docs/reference/) | [Changelog](https://github.com/Variably-Constant/Zstd/blob/main/CHANGELOG.md)

`Compress-Zstd`, `Expand-Zstd` and `New-ZstdDictionary` compress files and bytes, expand them, and train dictionaries with libzstd 1.5.7, the reference implementation of zstd, compiled into the module. It runs in Windows PowerShell 5.1 and PowerShell 7.4 or later on Windows x64 and Arm64, and is built with [PoWerRuSt](https://github.com/Variably-Constant/PWRS).

</div>

---

<details>
<summary><b>Table of contents</b></summary>

- [Features](#features)
- [Quick start](#quick-start)
- [Why](#why)
- [Cmdlets](#cmdlets)
- [Where it has been run](#where-it-has-been-run)
- [Building from source](#building-from-source)
- [Repository layout](#repository-layout)
- [Wiki](#wiki)
- [Use of AI tools](#use-of-ai-tools)
- [License](#license)
- [Contributing](#contributing)

</details>

---

## Features

- Compresses and expands files, or bytes from the pipeline, as zstd frames (RFC 8878) that carry a content checksum.
- [Files](https://variably-constant.github.io/Zstd/docs/how-to/how-to-compress-files/) by wildcard, by name as written with `-LiteralPath`, or piped from `Get-ChildItem`, each on its own: one that fails gets an error record and the rest go on.
- [Maps files to compress them, and expands them a mebibyte at a time](https://variably-constant.github.io/Zstd/docs/explanation/streaming-and-memory/), through a temporary file: 3 to 13 MB of commit above the host's own on a 5 GiB file at level 3, and nothing partial left by a failure or a stop.
- [A memory budget](https://variably-constant.github.io/Zstd/docs/explanation/streaming-and-memory/#the-memory-budget), `-MaxMemory`, from libzstd's own estimates: compression fits its worker threads to it, and a frame too large for it is written in place or refused before libzstd sees it.
- [Bytes](https://variably-constant.github.io/Zstd/docs/how-to/how-to-compress-bytes/): a single `byte[]` compressed without a copy, frames that record their size expanded straight into the `byte[]` written, and a longer pipeline fed as it arrives.
- [Dictionaries](https://variably-constant.github.io/Zstd/docs/how-to/how-to-use-a-dictionary/) trained from samples, in zstd's format or as raw content; a frame that needs another is refused, naming both.
- [Levels 1 to 22](https://variably-constant.github.io/Zstd/docs/how-to/how-to-choose-a-level-and-threads/) and libzstd's worker threads.
- `-WhatIf`, `-Confirm`, `-Force` and `-PassThru` on files.
- [Every failure](https://variably-constant.github.io/Zstd/docs/reference/error-reference/) an error record with an id and a category of its own.
- No managed dependencies, and no Visual C++ Redistributable: the C runtime is linked in statically.

## Quick start

```powershell
Install-Module Zstd -Scope CurrentUser
$text = 'The quick brown fox jumps over the lazy dog. ' * 20
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes($text))
'{0} bytes of text, {1} bytes compressed' -f $text.Length, $packed.Length
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $packed)) -eq $text
```

```text
900 bytes of text, 68 bytes compressed
True
```

The module runs on Windows x64 and Arm64 and needs nothing else installed: libzstd and the C runtime are compiled into it. The wiki's [Getting Started](https://variably-constant.github.io/Zstd/docs/tutorials/getting-started/) goes on from here to a file compressed and expanded, and its how-to guides to folders of files, dictionaries and bytes in a pipeline.

## Why

PowerShell has no zstd of its own. `Compress-Archive` writes Deflate zip files; .NET Framework, under Windows PowerShell 5.1, offers Deflate and GZip streams, and the .NET under PowerShell 7 adds Brotli and ZLib. .NET's own zstd stream is new in .NET 11. PowerShell 7.6 runs on .NET 10, and Windows PowerShell 5.1 on .NET Framework 4.8, which Microsoft services with security and reliability fixes.

This module compiles libzstd 1.5.7 into one native library that both hosts load, through [PWRS](https://github.com/Variably-Constant/PWRS). The wiki's [Why Zstd](https://variably-constant.github.io/Zstd/docs/explanation/why-zstd/) has the sources, what both hosts were checked for, and how the cmdlets compare with ZstdSharp and Deflate, timed.

## Cmdlets

| Cmdlet | Takes | Writes |
|---|---|---|
| [`Compress-Zstd`](https://variably-constant.github.io/Zstd/docs/reference/compress-zstd/) | files by `-Path` or `-LiteralPath`, or piped from `Get-ChildItem`; or bytes by `-InputObject`. `-Level`, `-Threads`, `-MaxMemory`, and a dictionary by `-DictionaryPath` or `-Dictionary` | on files, nothing, or with `-PassThru` a `System.IO.FileInfo` for each file written; on bytes, one `System.Byte[]` |
| [`Expand-Zstd`](https://variably-constant.github.io/Zstd/docs/reference/expand-zstd/) | the same inputs, dictionary parameters and `-MaxMemory` | the same |
| [`New-ZstdDictionary`](https://variably-constant.github.io/Zstd/docs/reference/new-zstddictionary/) | sample files by `-Path` or `-LiteralPath`, or piped; `-DestinationPath`, `-MaxSize` | nothing, or with `-PassThru` the dictionary's `System.IO.FileInfo` |

`Compress-Zstd` and `Expand-Zstd` take six parameter sets, so PowerShell's binder refuses `-InputObject` beside the file parameters and `-Dictionary` beside `-DictionaryPath`. The reference pages list every parameter with the refusals it raises and every property of each output type, with output from both hosts, and the [Error Reference](https://variably-constant.github.io/Zstd/docs/reference/error-reference/) lists all 20 error ids.

## Where it has been run

Built from this repository's committed files with Rust 1.98.1, PoWerRuSt 0.3.0 and cargo-pwrs 0.3.0 from crates.io, the module's release build and clippy with warnings denied were clean, its 43 Rust unit tests passed, and the 87 tests of its Pester suite passed on Pester 6.2.0 in each Windows 11 host below. Windows Server 2019 ran the suite but for its two largest tests, against the Windows 11 build.

| Platform | Hosts |
|---|---|
| Windows 11 Pro 10.0.26200, x64, AMD Ryzen 9 7900X | PowerShell 7.6.6, Windows PowerShell 5.1.26100.9444, and for the suite PowerShell 7.4.20 and 7.5.11 |
| Windows Server 2019 Standard Evaluation 10.0.17763, x64, without the Visual C++ Redistributable | PowerShell 7.6.6, Windows PowerShell 5.1.17763.2931 |

On Windows 11 the large-file check ran a 5 GiB file and its first 1 GiB through both cmdlets in both hosts: every expansion matched its source byte for byte, and each command's peak commit on 5 GiB was at most 1.14 times its peak on 1 GiB. Every example in the wiki ran in both hosts with the output shown.

Timed there against ZstdSharp.Port 0.8.8 at level 3 on a 128 MiB log corpus and 128 MiB of System32 DLLs, 7 rounds in each host, with a median of 2.3 to 8.3 cores busy outside the run in each round: on files, the cmdlets compressed 1.23 to 1.67 times as fast as ZstdSharp on the calling thread, and expanded 1.24 to 1.26 times as fast in Windows PowerShell 5.1 and 1.03 to 1.07 times in PowerShell 7.6, where rounds went either way; on bytes, they compressed 1.41 to 1.50 times as fast in Windows PowerShell 5.1 and 0.93 to 0.95 times in PowerShell 7.6.

The wiki's [Where It Has Been Run](https://variably-constant.github.io/Zstd/docs/reference/where-it-has-been-run/) has each run's machine, versions and results, and [Why Zstd](https://variably-constant.github.io/Zstd/docs/explanation/why-zstd/#against-zstdsharp-and-deflate-timed) the timing tables. The repository's CI builds the module and runs the suite at every push, in both hosts on an x64 and an Arm64 Windows runner; the package's Arm64 library is the one CI built and tested from the released commit. Not run: Windows 10 and a CPU without BMI2.

## Building from source

The module builds from this repository with [PoWerRuSt](https://crates.io/crates/PoWerRuSt) 0.3.0 from crates.io, which `Cargo.toml` names, and with cargo-pwrs 0.3.0, the tool of the same release. It needs Rust 1.98 or later with the MSVC toolchain, as `Cargo.toml`'s `rust-version` requires, and LLVM's `clang-cl` on `PATH`, which `.cargo/config.toml` names as the C compiler of libzstd:

```text
cargo install cargo-pwrs --version 0.3.0 --locked
cargo pwrs build --release
cargo clippy --release --all-targets -- -D warnings
cargo pwrs test --release
```

The module folder is `target\pwrs\Zstd`. The wiki's [Building From Source](https://variably-constant.github.io/Zstd/docs/building/building-from-source/) lists what the build needs and what the folder holds, and [How To Run The Tests](https://variably-constant.github.io/Zstd/docs/building/how-to-run-the-tests/) what each test layer covers, the large-file check, and CI.

## Repository layout

| Path | Role |
|---|---|
| `src/lib.rs` | the three cmdlets, and their Rust unit tests |
| `tests/Zstd.Tests.ps1` | the Pester suite `cargo pwrs test` runs in both hosts |
| `tests/LargeFile.Gate.ps1` | the 5 GiB large-file and memory check |
| `wiki/content/docs` | the documentation, organized by the Diataxis framework: tutorials, how-to guides, explanations and reference pages, and a section for building from source |
| `wiki/hugo.yaml`, `wiki/go.mod`, `wiki/go.sum`, `wiki/layouts`, `wiki/i18n` | the Hugo site the wiki is published as, on the Hextra theme |
| `.cargo/config.toml` | the x86-64 baseline, the static C runtime and `clang-cl`, for every build of this repository |
| `.github/workflows/ci.yml` | CI on an x64 and an Arm64 Windows runner: clippy, then `cargo pwrs test`, then the module folder uploaded, at every push to main and every pull request |
| `.github/workflows/wiki-deploy.yml` | builds the wiki with Hugo and publishes it to GitHub Pages at every push to main |
| `Cargo.toml`, `Cargo.lock` | the crate, `zstd-pwsh`, and the exact versions of its dependencies every build compiles |
| `CHANGELOG.md` | what changed in each version |
| `LICENSE` | MIT |

## Wiki

The documentation is published at [variably-constant.github.io/Zstd](https://variably-constant.github.io/Zstd/), built from [`wiki/content`](https://github.com/Variably-Constant/Zstd/tree/main/wiki/content) by the Diataxis framework: a tutorial, six how-to guides, four explanations, five reference pages (one for each cmdlet, the error ids, and where it has been run), and two pages on building from source and running the tests. Every example on them was run in both hosts, with the output it printed. `.github/workflows/wiki-deploy.yml` builds it with Hugo and deploys it on every push to main, once the repository is public.

## Use of AI tools

The author used Claude (Anthropic) via the Claude Code CLI for code development assistance, documentation drafting and test scripting during the preparation of this repository. All design decisions and the final content were determined by the author. The Rust implementation, the unit tests and the Pester suite were verified through zero-warning `cargo clippy` passes, `cargo pwrs test` in PowerShell 7 and Windows PowerShell 5.1, and end-to-end runs of the built module on Windows 11 x64.

## License

MIT, in [LICENSE](https://github.com/Variably-Constant/Zstd/blob/main/LICENSE). The module compiles in libzstd 1.5.7, which Meta licenses under BSD or GPLv2, through the zstd, zstd-safe and zstd-sys crates, each BSD-3-Clause, and maps files with the memmap2 crate, MIT or Apache-2.0. cargo-pwrs writes the notices the module carries into its folder: `THIRD-PARTY-NOTICES.txt` at the root, for the assemblies PWRS compiles, and one under each of `runtimes\win-x64` and `runtimes\win-arm64`, with the license and license files of each crate compiled into that native library.

## Contributing

Issues and pull requests go to [github.com/Variably-Constant/Zstd](https://github.com/Variably-Constant/Zstd). A change keeps `cargo clippy --release --all-targets -- -D warnings` clean and `cargo pwrs test --release` passing in both hosts, and an example added to the wiki shows the output it gave in both.
