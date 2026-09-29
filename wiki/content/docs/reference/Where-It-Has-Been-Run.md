---
title: Where It Has Been Run
weight: 5
---

What has been built and run, on what, and what it showed. The first table is the Windows 11 machine every example on these pages ran on: each row is a run of the release module that `cargo pwrs build --release` wrote from the repository's committed files, imported from its folder in both hosts, except where a row names another build. [Windows Server 2019](#windows-server-2019) lists the runs in a fresh install of Windows Server 2019, and [CI](#ci) what the repository's CI runs. Nothing else has been run; the list at the end names what has not.

| | |
|---|---|
| Machine | AMD Ryzen 9 7900X, Windows 11 Pro 10.0.26200, with the Visual Studio 2022 Build Tools (MSVC) and LLVM's `clang-cl` 22.1.8 on `PATH` |
| Hosts | PowerShell 7.6.6 on .NET 10.0.12 and Windows PowerShell 5.1.26100.9444 on .NET Framework 4.8.9345, with Pester 6.2.0 in both; for the Pester suite and the end-to-end runs, also PowerShell 7.4.20 on .NET 8.0.31 and 7.5.11 on .NET 9.0.20 |
| Built with | Rust 1.98.1, cargo-pwrs 0.3.0 and PoWerRuSt 0.3.0 from crates.io, the zstd crate 0.14.0 with zstd-safe 8.0.0 and zstd-sys 2.1.0, which compiles libzstd 1.5.7, and memmap2 0.9.11; `.cargo/config.toml`'s x86-64 baseline, static C runtime and `clang-cl`, and no `RUSTFLAGS` |
| Result | the release build and clippy with warnings denied, both clean; the 43 Rust unit tests passed, and the 87 Pester tests passed in PowerShell 7.6.6, Windows PowerShell 5.1, PowerShell 7.4.20 and 7.5.11 |
| Stopped | four of the Pester tests stop a command partway, `Compress-Zstd` and `Expand-Zstd` on a file, once with `-Force` over an existing file, and `New-ZstdDictionary` while it trains, and find no partial output left and a file already at the destination unchanged |
| Large files | `tests/LargeFile.Gate.ps1` in both hosts, on a 5 GiB file of the files under `C:\Windows\System32` and its first 1 GiB, at level 3 on the calling thread and at level 19 with 8 worker threads: every expansion matched its source byte for byte, and each command's peak commit on 5 GiB was at most 1.14 times its peak on 1 GiB, within the bound of 1.5 |
| Bytes | in all four hosts, on 149244064 bytes of System32 DLLs: a `byte[]` whose frames record their size expanded straight into the `byte[]` written, one written from a file and one from a `byte[]`; a frame after a first record, a frame split across two records and a frame that records no size each fed to libzstd as it arrived; every result equal to its source; `-MaxMemory` counting the result once, refusing the budget one byte short of what it needs with `ZstdBudgetTooSmall` and expanding within that budget; a header recording one byte more refused with "a frame records a content size of 149244065 bytes and holds 149244064", and one byte less with libzstd's "Destination buffer is too small", both `ZstdInvalidData` |
| Folders | in all four hosts, a folder given to `-Path`, `-LiteralPath` or `-DictionaryPath` of each cmdlet that takes them refused with `ZstdInputIsDirectory`, its message naming the parameter |
| Dependents | `dumpbin /dependents` on the native library lists only `KERNEL32.dll`, `ntdll.dll` and `api-ms-win-core-synch-l1-2-0.dll`, which are part of Windows |
| Examples | the 119 examples on these pages, each page's in order in a process of its own, in PowerShell 7.6.6 and Windows PowerShell 5.1: every one printed the output its page shows |
| Timed against ZstdSharp and Deflate | against ZstdSharp.Port 0.8.8 and each host's `DeflateStream`, at level 3 on a 128 MiB log corpus and 128 MiB of System32 DLLs, on files and on bytes, with 1 and 8 threads, 7 rounds in each host, with a median of 2.3 to 8.3 cores busy outside the run in each round: on files, compressing 1.23 to 1.67 times as fast as ZstdSharp on the calling thread, and expanding 1.24 to 1.26 times in Windows PowerShell 5.1 and 1.03 to 1.07 in PowerShell 7.6, where rounds went either way; on bytes, compressing 1.41 to 1.50 times in Windows PowerShell 5.1 and 0.93 to 0.95 times in PowerShell 7.6. The tables are in [Why Zstd](Why-Zstd.md#against-zstdsharp-and-deflate-timed) |
| Timed, mapped against a mebibyte at a time | libzstd compiled by MSVC's cl.exe in both modules: this module against one that read and wrote every file a mebibyte at a time, file to file on 5 GiB, 7 rounds in each host, with a median of 4.7 cores busy outside the run in Windows PowerShell 5.1 and 6.2 to 7.5 in PowerShell 7.6: compressing from a map 8 to 12% faster by the median at level 3 on the calling thread, with rounds going either way, the same at level 19 with 8 worker threads, and expanding the same within the spread of the rounds; the table is in [Streaming And Memory](Streaming-And-Memory.md#mapped-against-a-mebibyte-at-a-time-timed) |
| Timed, clang-cl against cl.exe | libzstd compiled by clang-cl, with its BMI2 paths, against the same source compiled by MSVC's cl.exe, at level 3 on two 128 MiB corpora, on bytes and file to file, on the calling thread and with 8 worker threads, 7 rounds in each host, each round timing both, with a median of 8.7 cores busy outside the run in PowerShell 7.6 and 4.3 in Windows PowerShell 5.1, from 1.4 to 13 across rounds: expanding 1.08 to 1.32 times as fast, the medians of the 16 cells, and faster in 100 of the 112 rounds; compressing 0.90 to 1.11, with rounds going either way in every cell |
| Estimates | `-MaxMemory`'s estimates against the peak commit of `Compress-Zstd`, at levels 3, 19 and 22 with 0, 1, 8 and 24 worker threads, on 64 MiB to 5 GiB of the same data, each case once in each host, with the processors 41 to 97% busy as each case began: every peak stayed within its estimate, at 0.19 to 0.998 of it; the table is in [Streaming And Memory](Streaming-And-Memory.md#how-close-the-estimates-come) |

## Windows Server 2019

A fresh install of Windows Server 2019 Standard Evaluation 10.0.17763, in a QEMU virtual machine on the machine above with software CPU emulation, without the Visual C++ Redistributable. It imported the module the Windows 11 machine built.

| | |
|---|---|
| Hosts | Windows PowerShell 5.1.17763.2931 on .NET Framework 4.7.3946, and PowerShell 7.6.6 on .NET 10.0.12, extracted from its official zip |
| Import and use | in each host the module imported, compressed and expanded `shell32.dll`, `ntdll.dll` and the `services` file at level 19 with 4 worker threads, `ntdll.dll` as bytes, a file through a dictionary it trained, and a file within `-MaxMemory 8MB`, every expansion matching its source, files by SHA-256 and bytes element by element, and refused a `$null` record with `ZstdNoInput` |
| Pester suite | the suite but for its two largest tests, the 32 MiB `byte[]` bound within 3 seconds and the 512 MiB file of zeros, with Pester 6.2.0: 85 passed and none failed in each host |

## CI

The repository's CI builds the module and runs the suite at every push, in both hosts on an x64 and an Arm64 Windows runner; the package's Arm64 library is the one CI built and tested from the released commit.

## Not run

- Speed on another machine, at levels other than 3, or against tools other than those above.
- Other Windows versions, and the large-file check on Windows Server 2019 or on Arm64.
- A CPU without BMI2, on which libzstd takes its baseline paths.
