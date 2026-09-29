---
title: Getting Started
weight: 1
---

From installing the module to a compressed file and back, and then the same with bytes in memory, in PowerShell 7 or Windows PowerShell 5.1. Source: `src/lib.rs`, and the module manifest and `Zstd.psm1` that cargo-pwrs writes from `Cargo.toml`.

## What it runs on

- Windows PowerShell 5.1, or PowerShell 7.4 or later. On an earlier PowerShell 7 the import stops with a message saying the module needs 7.4 or later.
- Windows x64 or Arm64: the module carries a native library for each, `win-x64` and `win-arm64`.
- Nothing else: libzstd and the C runtime are compiled into the module, so no Visual C++ Redistributable is needed.

## Install

From the PowerShell Gallery, for the current user:

```powershell
Install-Module Zstd -Scope CurrentUser
```

PowerShell 7.4 and later also take `Install-PSResource Zstd`. The first install from the Gallery on a machine can ask whether to trust the PowerShell Gallery and, in Windows PowerShell, whether to install the NuGet provider first; both are needed to install from it.

PowerShell loads the module the first time one of its cmdlets runs; `Import-Module Zstd` loads it at once. To build the module yourself instead, see [Building From Source](Building-From-Source.md).

```powershell
Get-Command -Module Zstd
```

```text
CommandType     Name                                               Version    Source
-----------     ----                                               -------    ------
Cmdlet          Compress-Zstd                                      0.1.0      Zstd
Cmdlet          Expand-Zstd                                        0.1.0      Zstd
Cmdlet          New-ZstdDictionary                                 0.1.0      Zstd
```

The examples on this page ran in order in both hosts, in a folder of their own; the output shown is PowerShell 7.6's, and Windows PowerShell 5.1 printed the same apart from column widths.

## Compress a file

First something to compress: 20000 lines shaped like a web server's log. The random numbers start from a fixed seed, so the file comes out the same every time.

```powershell
$random = [System.Random]::new(42)
$words = -split 'GET POST PUT DELETE /api/users /api/orders /api/items /login /health 200 201 204 304 400 404 500'
1..20000 | ForEach-Object {
    '12:{0:d2}:{1:d2} {2} {3} {4} {5}ms' -f ($_ % 60), ($_ * 7 % 60), $words[$random.Next(0, 4)], $words[$random.Next(4, 9)], $words[$random.Next(9, 16)], $random.Next(1, 900)
} | Set-Content -Path .\app.log
Get-Content .\app.log -TotalCount 3
(Get-Item .\app.log).Length
```

```text
12:01:07 PUT /api/users 200 470ms
12:02:14 GET /api/orders 404 462ms
12:03:21 GET /login 201 232ms
693509
```

`Compress-Zstd` writes the file beside itself as `app.log.zst`. It writes nothing to the pipeline unless `-PassThru` asks for the file it wrote:

```powershell
Compress-Zstd -Path .\app.log -PassThru | Select-Object Name, Length
```

```text
Name        Length
----        ------
app.log.zst 140990
```

```powershell
'{0:N1} times smaller' -f ((Get-Item .\app.log).Length / (Get-Item .\app.log.zst).Length)
```

```text
4.9 times smaller
```

That was level 3, libzstd's default. [How To Choose A Level And Threads](How-To-Choose-A-Level-And-Threads.md) shows what the other levels do to the same file.

## Expand it

`Expand-Zstd` writes `app.log` back beside the `.zst` file by default. Here `-DestinationPath` names another file, so the copy can be compared with the original:

```powershell
Expand-Zstd -Path .\app.log.zst -DestinationPath .\app.copy.log -PassThru | Select-Object Name, Length
(Get-FileHash .\app.log).Hash -eq (Get-FileHash .\app.copy.log).Hash
```

```text
Name         Length
----         ------
app.copy.log 693509
True
```

## Bytes instead of files

`-InputObject` takes a `byte[]`, and on bytes both cmdlets write a `byte[]`:

```powershell
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('hello, zstd'))
$packed.Length
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $packed))
```

```text
24
hello, zstd
```

Eleven bytes became 24, since a frame carries a header and a checksum around its data: compression pays off on data with repetition in it, like the log above. [Frames And Records](Frames-And-Records.md) takes a frame apart.

## Help at the prompt

Each cmdlet carries its help, written from the same source as the reference pages, with examples:

```powershell
(Get-Help Compress-Zstd).Synopsis
(Get-Help Compress-Zstd).examples.example.code
```

```text
Compresses files, or bytes, with Zstandard.
Compress-Zstd -Path .\app.log
Compress-Zstd -Path .\logs\*.log -DestinationPath .\archive -PassThru
Get-ChildItem .\logs -Filter *.log | Compress-Zstd -Level 19
Compress-Zstd -Path .\app.log -DestinationPath .\app.log.zst -Level 19 -Threads 4 -Force
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('hello'))
```

## Next

- Compress and expand many files at once: [How To Compress Files](How-To-Compress-Files.md) and [How To Expand Files](How-To-Expand-Files.md).
- Work with bytes in memory: [How To Compress Bytes](How-To-Compress-Bytes.md).
- Compress many small files of one shape several times smaller: [How To Use A Dictionary](How-To-Use-A-Dictionary.md).
- Handle what fails: [How To Handle Errors](How-To-Handle-Errors.md).
- Every parameter and output type: [Compress-Zstd](Compress-Zstd.md), [Expand-Zstd](Expand-Zstd.md) and [New-ZstdDictionary](New-ZstdDictionary.md).
