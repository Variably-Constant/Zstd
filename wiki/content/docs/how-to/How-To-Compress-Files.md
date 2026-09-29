---
title: How To Compress Files
weight: 1
---

`Compress-Zstd` on files: which files it reads, where it writes, and when it refuses. Source: `src/lib.rs`, `CompressZstd` and the path helpers `named_files`, `destination` and `check_destination`.

Each file is compressed on its own into one zstd frame: it is mapped into memory and handed to libzstd a mebibyte more at a time, or read a mebibyte at a time when Windows will not map it or, within `-MaxMemory`, the page tables of its map do not fit, and the output is written to a temporary file beside the destination that is renamed into place once it is complete. A file that fails gets its own error record and the rest go on.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, in an empty folder. The output shown is PowerShell 7.6's; Windows PowerShell 5.1 printed the same apart from column widths and line wrapping.

## Some files to compress

Three logs of 7000 lines each:

```powershell
$random = [System.Random]::new(7)
$words = -split 'GET POST PUT DELETE /api/users /api/orders /api/items /login /health 200 201 204 304 400 404 500'
New-Item -ItemType Directory -Path .\logs | Out-Null
foreach ($name in 'api', 'web', 'worker') {
    1..7000 | ForEach-Object {
        '12:{0:d2}:{1:d2} {2} {3} {4} {5}ms' -f ($_ % 60), ($_ * 7 % 60), $words[$random.Next(0, 4)], $words[$random.Next(4, 9)], $words[$random.Next(9, 16)], $random.Next(1, 900)
    } | Set-Content -Path ".\logs\$name.log"
}
Get-ChildItem .\logs | Select-Object Name, Length
```

```text
Name       Length
----       ------
api.log    242415
web.log    242542
worker.log 242567
```

## One file, written beside itself

Without `-DestinationPath`, the output goes beside the input under its name with `.zst` added:

```powershell
Compress-Zstd -Path .\logs\api.log -PassThru | Select-Object Name, Length
```

```text
Name        Length
----        ------
api.log.zst  50893
```

## Every file a wildcard matches, into a folder

`-Path` takes wildcards and several paths. A `-DestinationPath` that names an existing folder receives each file under its default name:

```powershell
New-Item -ItemType Directory -Path .\archive | Out-Null
Compress-Zstd -Path .\logs\*.log -DestinationPath .\archive -PassThru | Select-Object Name, Length
```

```text
Name           Length
----           ------
api.log.zst     50893
web.log.zst     50815
worker.log.zst  50813
```

A folder that does not exist yet is not a folder to the cmdlet: it is the name of one file. Create the folder first, as above.

## Files piped from Get-ChildItem

A file piped from `Get-ChildItem` or `Get-Item` binds to `-LiteralPath` by its `PSPath` property. This run replaces the files above with level 19 output:

```powershell
Get-ChildItem .\logs -Filter *.log | Compress-Zstd -DestinationPath .\archive -Level 19 -Force -PassThru | Select-Object Name, Length
```

```text
Name           Length
----           ------
api.log.zst     34479
web.log.zst     34392
worker.log.zst  34418
```

## A name with wildcard characters in it

`-Path` reads `[`, `]`, `*` and `?` as wildcards. `-LiteralPath` takes the name exactly as written:

```powershell
New-Item -ItemType Directory -Path .\odd | Out-Null
Copy-Item .\logs\web.log '.\odd\web[1].log'
Compress-Zstd -LiteralPath '.\odd\web[1].log' -PassThru | Select-Object Name, Length
```

```text
Name           Length
----           ------
web[1].log.zst  50815
```

Through `-Path`, `[1]` is a set of one character that matches `1`, so the pattern names `web1.log`, which does not exist:

```powershell
Compress-Zstd -Path '.\odd\web[1].log' -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
```

```text
ZstdInputNotFound,Pwrs.Modules.Zstd.CompressZstdCommand
Cannot find '.\odd\web[1].log'.
```

## One file as the destination

A `-DestinationPath` that is not an existing folder names one file:

```powershell
Compress-Zstd -Path .\logs\web.log -DestinationPath .\web-backup.zst -PassThru | Select-Object Name, Length
```

```text
Name           Length
----           ------
web-backup.zst  50815
```

Only the first input of a command may take that file. A second input is refused with its own error record rather than written over the first:

```powershell
Compress-Zstd -Path .\logs\web.log, .\logs\worker.log -DestinationPath .\both.zst -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
(Get-Item .\both.zst).Length -eq (Get-Item .\web-backup.zst).Length
```

```text
ZstdDestinationReused,Pwrs.Modules.Zstd.CompressZstdCommand
-DestinationPath 'C:\Temp\zstd-docs\both.zst' is one file, and an earlier input of this command took it, so 'C:\Temp\zstd-docs\logs\worker.log' was not written.
True
```

`both.zst` holds `web.log` only, the same frame as `web-backup.zst`.

## Replacing a file that exists

A destination that exists is refused unless `-Force` is given:

```powershell
Compress-Zstd -Path .\logs\api.log -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
```

```text
ZstdDestinationExists,Pwrs.Modules.Zstd.CompressZstdCommand
'C:\Temp\zstd-docs\logs\api.log.zst' already exists. Use -Force to replace it.
```

```powershell
Compress-Zstd -Path .\logs\api.log -Level 9 -Force -PassThru | Select-Object Name, Length
```

```text
Name        Length
----        ------
api.log.zst  43256
```

Under `-Force` the new output is still written to a temporary file first, so the file already there stays whole until the new one is complete.

## Seeing what would happen

`-WhatIf` names each file and its destination and writes nothing; `-Confirm` asks before each file:

```powershell
Compress-Zstd -Path .\logs\worker.log -DestinationPath .\preview.zst -WhatIf
Test-Path .\preview.zst
```

```text
What if: Performing the operation "Compress File" on target "Item: C:\Temp\zstd-docs\logs\worker.log Destination: C:\Temp\zstd-docs\preview.zst".
False
```

## Seeing the sizes

`-Verbose` reports both sizes of each file, the level and the worker threads:

```powershell
Compress-Zstd -Path .\logs\worker.log -DestinationPath .\worker-19.zst -Level 19 -Verbose
```

```text
VERBOSE: Performing the operation "Compress File" on target "Item: C:\Temp\zstd-docs\logs\worker.log Destination: C:\Temp\zstd-docs\worker-19.zst".
VERBOSE: Compressed 'C:\Temp\zstd-docs\logs\worker.log' (242567 bytes) to 'C:\Temp\zstd-docs\worker-19.zst' (34418 bytes) at level 19 with 0 worker threads.
```

Levels and worker threads have [a page of their own](How-To-Choose-A-Level-And-Threads.md). Every parameter is in the [Compress-Zstd reference](Compress-Zstd.md).
