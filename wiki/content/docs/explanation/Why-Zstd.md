---
title: Why Zstd
weight: 1
---

What the two PowerShell hosts offer for compression, what this module adds, and how fast it runs against a managed port of zstd and the hosts' Deflate. Source: `Cargo.toml`, `.cargo/config.toml`, `src/lib.rs`.

## What the hosts have

PowerShell has no zstd of its own. `Compress-Archive` writes Deflate zip files, and the compression streams in .NET Framework, which Windows PowerShell 5.1 runs on, are Deflate and GZip. The .NET under PowerShell 7 adds Brotli and ZLib streams, and no zstd one.

.NET's own zstd stream, `System.IO.Compression.ZstandardStream`, is new in .NET 11: Microsoft's [API reference](https://learn.microsoft.com/dotnet/api/system.io.compression.zstandardstream?view=net-11.0) documents it for .NET 11 only, and it is listed in [What's new in .NET 11](https://learn.microsoft.com/dotnet/core/whats-new/dotnet-11/libraries#compression-and-archive-formats). PowerShell 7.6 runs on .NET 10. Windows PowerShell 5.1 runs on .NET Framework 4.8, which Microsoft services with security and reliability fixes ([What's new in .NET Framework](https://learn.microsoft.com/dotnet/framework/whats-new/)), so 5.1 will not get it from .NET.

Neither host that ran these pages has the type, and the only commands with `zstd` in their names are this module's:

```powershell
$null -eq ('System.IO.Compression.ZstandardStream' -as [type])
[System.Runtime.InteropServices.RuntimeInformation]::FrameworkDescription
Get-Command -Name *zstd* | ForEach-Object Name
```

```text
True
.NET 10.0.12
Compress-Zstd
Expand-Zstd
New-ZstdDictionary
```

In Windows PowerShell 5.1:

```text
True
.NET Framework 4.8.9345.0
Compress-Zstd
Expand-Zstd
New-ZstdDictionary
```

## What the module is

The module compiles libzstd 1.5.7, the reference implementation of zstd, into one native library, through the zstd, zstd-safe and zstd-sys crates. [PWRS](https://github.com/Variably-Constant/PWRS) turns the Rust crate into a module folder: it generates the cmdlet classes both hosts reflect over, compiles them into the folder's `net10.0` half for PowerShell 7 and its `netstandard2.0` half for Windows PowerShell 5.1, and routes each call into the native library. One folder imports into PowerShell 7 and into Windows PowerShell 5.1, and the module carries no managed dependencies of its own.

The native library is built for the x86-64 baseline with the C runtime linked in statically. It imports only `KERNEL32.dll`, `ntdll.dll` and `api-ms-win-core-synch-l1-2-0.dll`, which are part of Windows, and nothing from the Visual C++ Redistributable. libzstd is compiled by LLVM's clang-cl so that it carries its BMI2 code paths beside the baseline ones, and picks them at run time on a CPU that has BMI2. It is built with its worker threads and its dictionary trainer, and without its decoders for the formats from before zstd 1.0, so data in those formats is refused as not zstd data.

## What it writes

Every result is one zstd frame, in the format RFC 8878 describes, and every frame carries a checksum of its content, which the expander checks. A file's frame, and the frame for a single `byte[]`, also record the content size in their header. [Frames And Records](Frames-And-Records.md) takes a frame apart.

Files are compressed from a map of them and expanded a mebibyte at a time, or in place when a frame's window needs it, and written through a temporary file that becomes the destination only when it is complete, and `-MaxMemory` bounds what a command may commit: [Streaming And Memory](Streaming-And-Memory.md) has the details and the measured cost.

## Against ZstdSharp and Deflate, timed

The cmdlets were timed against ZstdSharp.Port 0.8.8, a managed port of zstd, and against the `DeflateStream` of each host's .NET, on two corpora of 128 MiB: log lines generated from a fixed seed, and 64 DLLs from `C:\Windows\System32` joined end to end. Every arm compressed a corpus and expanded the result, zstd at level 3 and Deflate at its optimal and its fastest level, and the round trip was compared with the corpus byte for byte outside the timed spans.

- On files, the cmdlets ran with `-Path`, ZstdSharp ran its `CompressionStream` and `DecompressionStream` over file streams with 1 MiB copies, and Deflate ran over file streams.
- On bytes, the cmdlets took the corpus through `-InputObject`, ZstdSharp its `Compressor.Wrap` and `Decompressor.Unwrap` with each output array allocated in the timed span, and Deflate a `MemoryStream`.
- With 8 threads, the cmdlets ran `-Threads 8` and ZstdSharp 8 workers.

Each round was a fresh process for one host and corpus: one untimed pass of every arm, then every arm once, timed, in an order rotated by round, with a full garbage collection before each arm; 7 rounds for each host and corpus. It ran on Windows 11 Pro 10.0.26200 on an AMD Ryzen 9 7900X, in Windows PowerShell 5.1 and PowerShell 7.6.6, with the module built as it ships. Every round ran whatever else the machine was doing, and the load outside the run was sampled every 5 seconds: the median of a round's samples was 2.3 to 8.3 cores busy, with single readings up to 21. The median of those round medians was 3.7 cores for Windows PowerShell 5.1 on the log corpus, 5.2 on the DLLs, and 2.7 and 5.2 for PowerShell 7.6.

Medians over the 7 rounds, in MB (10^6 bytes) of the corpus per second, compressing / expanding:

| Arm | 5.1, log | 5.1, DLLs | 7.6, log | 7.6, DLLs |
|---|---|---|---|---|
| `Compress-Zstd`, `Expand-Zstd` on files | 330 / 1063 | 322 / 965 | 327 / 1085 | 307 / 933 |
| the same, `-Threads 8` | 1359 / 1104 | 1235 / 979 | 1315 / 1067 | 1252 / 964 |
| `Compress-Zstd` on bytes | 359 | 369 | 355 | 335 |
| `Compress-Zstd` on bytes, `-Threads 8` | 1635 | 1552 | 1290 | 1239 |
| ZstdSharp on files | 196 / 861 | 195 / 754 | 259 / 1000 | 256 / 903 |
| ZstdSharp on files, 8 workers | 1105 / 823 | 1025 / 703 | 1411 / 1023 | 1269 / 917 |
| ZstdSharp in memory | 274 / 1044 | 245 / 866 | 378 / 1500 | 354 / 1072 |
| ZstdSharp in memory, 8 workers | 1089 / 1050 | 1039 / 873 | 1482 / 1516 | 1504 / 1324 |
| Deflate optimal on files | 59 / 267 | 33 / 202 | 89 / 863 | 86 / 624 |
| Deflate fastest on files | 173 / 230 | 104 / 174 | 372 / 788 | 278 / 648 |
| Deflate optimal in memory | 59 / 283 | 33 / 213 | 90 / 871 | 82 / 635 |
| Deflate fastest in memory | 164 / 240 | 104 / 178 | 383 / 788 | 293 / 662 |

zstd at level 3 made the log corpus 3.66 times smaller and the DLLs 2.75 to 2.76 times from the cmdlets, and 3.63 to 3.66 and 2.75 to 2.76 times from ZstdSharp. Deflate at its optimal level made the log corpus 3.80 times smaller in Windows PowerShell 5.1 and 3.77 in PowerShell 7.6, and the DLLs 2.61 and 2.55 times; at its fastest, 3.15 and 2.45 times, and 2.39 and 2.02.

The median of each round's ratio, the cmdlets over the other arm, with the least and the most of the 7:

| Against | 5.1, log | 5.1, DLLs | 7.6, log | 7.6, DLLs |
|---|---|---|---|---|
| ZstdSharp on files, compressing | 1.67 [1.60-1.71] | 1.67 [1.55-2.04] | 1.29 [1.19-1.35] | 1.23 [1.18-1.53] |
| ZstdSharp on files, expanding | 1.24 [1.20-1.49] | 1.26 [1.20-1.39] | 1.07 [0.94-1.14] | 1.03 [0.98-1.08] |
| ZstdSharp on files, 8 threads, compressing | 1.25 [1.02-1.39] | 1.29 [1.16-1.38] | 0.90 [0.76-1.03] | 0.99 [0.77-2.77] |
| ZstdSharp on files, 8 threads, expanding | 1.33 [1.17-1.44] | 1.34 [1.26-1.45] | 1.03 [0.96-1.21] | 1.05 [0.81-1.18] |
| ZstdSharp in memory, compressing | 1.41 [1.31-1.45] | 1.50 [1.44-1.57] | 0.93 [0.90-0.99] | 0.95 [0.87-1.22] |
| Deflate optimal on files, compressing | 5.69 [5.41-5.95] | 9.94 [9.08-10.11] | 3.68 [3.47-3.93] | 3.85 [2.55-4.83] |
| Deflate optimal on files, expanding | 4.02 [3.59-4.52] | 4.73 [4.13-5.19] | 1.23 [1.06-1.28] | 1.49 [0.96-1.92] |
| Deflate fastest on files, compressing | 1.95 [1.85-2.04] | 3.09 [2.94-3.37] | 0.89 [0.86-0.92] | 1.09 [0.75-1.25] |
| Deflate fastest on files, expanding | 4.65 [4.22-9.50] | 5.56 [5.07-6.09] | 1.37 [1.22-1.42] | 1.43 [0.93-1.61] |

On files, on the calling thread, the cmdlets compressed faster than ZstdSharp in every cell and expanded faster in Windows PowerShell 5.1, and with 8 threads they were faster at both there. On bytes, they compressed faster than ZstdSharp in Windows PowerShell 5.1, and in PowerShell 7.6 ZstdSharp compressed the log corpus faster. Against Deflate, the cmdlets were faster in every cell that names a winner but one, compressing the log corpus in PowerShell 7.6 against Deflate's fastest level, whose output was half again as large. Where a cell's least and most lie on both sides of 1.0, its rounds spread wider than the difference it reads, and it names no winner. Every such cell is in PowerShell 7.6: on files, expanding both corpora on the calling thread, compressing and expanding both with 8 threads, expanding the DLLs against both Deflate levels and compressing them against the fastest; on bytes, compressing the DLLs.
