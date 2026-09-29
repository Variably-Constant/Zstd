---
title: How To Expand Files
weight: 2
---

`Expand-Zstd` on files: where the output goes, how several frames and several files are taken, and what it does with data that is not whole. Source: `src/lib.rs`, `ExpandZstd`, `Expansion` and the path helpers `named_files`, `default_name` and `destination`.

Each file is expanded on its own into a temporary file beside the destination, which is renamed into place only once every frame in the input has expanded and passed its checksum. A file is expanded a mebibyte at a time, unless a frame's window is too large for that, in which case it is written in place (see below). A file that fails gets its own error record, leaves nothing behind, and the rest go on.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, in an empty folder. The output shown is PowerShell 7.6's; Windows PowerShell 5.1 printed the same apart from column widths.

## Some files to expand

Three logs, compressed into a folder:

```powershell
$random = [System.Random]::new(7)
$words = -split 'GET POST PUT DELETE /api/users /api/orders /api/items /login /health 200 201 204 304 400 404 500'
New-Item -ItemType Directory -Path .\logs, .\archive | Out-Null
foreach ($name in 'api', 'web', 'worker') {
    1..7000 | ForEach-Object {
        '12:{0:d2}:{1:d2} {2} {3} {4} {5}ms' -f ($_ % 60), ($_ * 7 % 60), $words[$random.Next(0, 4)], $words[$random.Next(4, 9)], $words[$random.Next(9, 16)], $random.Next(1, 900)
    } | Set-Content -Path ".\logs\$name.log"
}
Compress-Zstd -Path .\logs\*.log -DestinationPath .\archive
Get-ChildItem .\archive | Select-Object Name, Length
```

```text
Name           Length
----           ------
api.log.zst     50893
web.log.zst     50815
worker.log.zst  50813
```

## One file, written beside itself

Without `-DestinationPath`, the output goes beside the input under its name without `.zst`:

```powershell
Expand-Zstd -Path .\archive\api.log.zst -PassThru | Select-Object Name, Length
```

```text
Name    Length
----    ------
api.log 242415
```

## Every file a wildcard matches, into a folder

A `-DestinationPath` that names an existing folder receives each file under its default name:

```powershell
New-Item -ItemType Directory -Path .\restored | Out-Null
Expand-Zstd -Path .\archive\*.zst -DestinationPath .\restored -PassThru | Select-Object Name, Length
```

```text
Name       Length
----       ------
api.log    242415
web.log    242542
worker.log 242567
```

Each one matches its source byte for byte:

```powershell
Get-ChildItem .\logs | ForEach-Object {
    [pscustomobject]@{
        Name      = $_.Name
        Identical = (Get-FileHash $_.FullName).Hash -eq (Get-FileHash ".\restored\$($_.Name)").Hash
    }
}
```

```text
Name       Identical
----       ---------
api.log         True
web.log         True
worker.log      True
```

## Files piped from Get-ChildItem

A file piped from `Get-ChildItem` or `Get-Item` binds to `-LiteralPath` by its `PSPath` property. The files are in `.\restored` already, so this run needs `-Force`:

```powershell
Get-ChildItem .\archive -Filter *.zst | Expand-Zstd -DestinationPath .\restored -Force -PassThru | Select-Object Name, Length
```

```text
Name       Length
----       ------
api.log    242415
web.log    242542
worker.log 242567
```

## A destination that does not exist

A `-DestinationPath` that is not an existing folder names one file, whatever it looks like:

```powershell
Expand-Zstd -Path .\archive\web.log.zst -DestinationPath .\unpacked -PassThru | Select-Object Name, Length
Test-Path .\unpacked -PathType Leaf
```

```text
Name     Length
----     ------
unpacked 242542
True
```

As with `Compress-Zstd`, only the first input of a command may take such a file; a second is refused with `ZstdDestinationReused`.

## A name without .zst

The default name drops `.zst`, so a file named otherwise has none, and needs `-DestinationPath`:

```powershell
Copy-Item .\archive\api.log.zst .\api.bin
Expand-Zstd -Path .\api.bin -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
Expand-Zstd -Path .\api.bin -DestinationPath .\api-from-bin.log -PassThru | Select-Object Name, Length
```

```text
ZstdNoDestination,Pwrs.Modules.Zstd.ExpandZstdCommand
'C:\Temp\zstd-docs\api.bin' does not end in .zst, so it has no default destination. Name one with -DestinationPath.

Name             Length
----             ------
api-from-bin.log 242415
```

The name is only for the default destination: the file's content decides whether it is zstd data.

## Several frames in one file

zstd data can hold several frames one after another, which is what joining two `.zst` files end to end makes. `Expand-Zstd` expands every frame, in order, into one output:

```powershell
[byte[]] $two = [System.IO.File]::ReadAllBytes("$PWD\archive\api.log.zst") + [System.IO.File]::ReadAllBytes("$PWD\archive\web.log.zst")
[System.IO.File]::WriteAllBytes("$PWD\two.log.zst", $two)
Expand-Zstd -Path .\two.log.zst -PassThru | Select-Object Name, Length
(Get-Item .\two.log).Length -eq (Get-Item .\logs\api.log).Length + (Get-Item .\logs\web.log).Length
```

```text
Name    Length
----    ------
two.log 484957
True
```

## A frame with a window over 128 MiB

A frame expanded a mebibyte at a time needs a window of recent output as large as its header asks for, and without `-MaxMemory` a window over 128 MiB, libzstd's own limit, does not fit. The largest window any level of this module writes is 128 MiB, at level 22, so this concerns frames written elsewhere. A file with such a frame is written in place when every frame in it records its content size: libzstd then writes into a map of the output, and needs no window of its own. This frame is built by hand from RFC 8878: its header asks for a window of 256 MiB and records its content size, and one raw block holds 300 bytes of text:

```powershell
[byte[]] $text = [System.Text.Encoding]::ASCII.GetBytes(('in place ' * 40).Substring(0, 300))
[byte[]] $sized = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x40, 0x90, 0x2C, 0x00, 0x61, 0x09, 0x00) + $text
[System.IO.File]::WriteAllBytes("$PWD\sized.zst", $sized)
Expand-Zstd -Path .\sized.zst -Verbose
(Get-Item .\sized).Length
```

```text
VERBOSE: Performing the operation "Expand File" on target "Item: C:\Temp\zstd-docs\sized.zst Destination: C:\Temp\zstd-docs\sized".
VERBOSE: Expanding 'C:\Temp\zstd-docs\sized.zst' in place: a frame in it has a window of 268435456 bytes, which does not fit a chunk at a time.
VERBOSE: Expanded 'C:\Temp\zstd-docs\sized.zst' (311 bytes) to 'C:\Temp\zstd-docs\sized' (300 bytes).
300
```

A frame that records no content size cannot be written in place, since the length of the output is not known before it is expanded. This one asks for a window of 256 MiB, records no content size, and one raw block holds the five bytes of `hello`:

```powershell
[byte[]] $wide = 0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x90, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F
[System.IO.File]::WriteAllBytes("$PWD\wide.zst", $wide)
Expand-Zstd -Path .\wide.zst -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
Test-Path .\wide
```

```text
ZstdWindowTooLarge,Pwrs.Modules.Zstd.ExpandZstdCommand
Cannot expand 'C:\Temp\zstd-docs\wide.zst': a frame in it has a window of 268435456 bytes, more than the 134217728 bytes libzstd expands without -MaxMemory; -MaxMemory of 273119008 bytes or more lets it.
False
```

The message names the budget that lets it. With `-MaxMemory` at least that, the frame is expanded a mebibyte at a time:

```powershell
Expand-Zstd -Path .\wide.zst -MaxMemory 512MB
Get-Content .\wide
```

```text
hello
```

## Data that is not whole

Every frame the module writes carries a checksum of its content, which libzstd checks as it expands. Here one byte in the middle of a frame is flipped:

```powershell
[byte[]] $broken = [System.IO.File]::ReadAllBytes("$PWD\archive\api.log.zst")
$broken[100] = $broken[100] -bxor 0xFF
[System.IO.File]::WriteAllBytes("$PWD\broken.log.zst", $broken)
Expand-Zstd -Path .\broken.log.zst -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
Test-Path .\broken.log
```

```text
ZstdInvalidData,Pwrs.Modules.Zstd.ExpandZstdCommand
Cannot expand 'C:\Temp\zstd-docs\broken.log.zst': it is not valid zstd data (Restored data doesn't match checksum).
False
```

The text in parentheses is libzstd's own. No `broken.log` is left behind: the partial output went to a temporary file, which was removed.

A file cut short ends inside a frame:

```powershell
[byte[]] $cut = [System.IO.File]::ReadAllBytes("$PWD\archive\web.log.zst")[0..999]
[System.IO.File]::WriteAllBytes("$PWD\cut.log.zst", $cut)
Expand-Zstd -Path .\cut.log.zst -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.Exception.Message
```

```text
Cannot expand 'C:\Temp\zstd-docs\cut.log.zst': it is not valid zstd data (the data ends inside a frame).
```

And an empty file holds no frame at all:

```powershell
New-Item -ItemType File -Path .\empty.log.zst | Out-Null
Expand-Zstd -Path .\empty.log.zst -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.Exception.Message
```

```text
Cannot expand 'C:\Temp\zstd-docs\empty.log.zst': it is empty, and zstd data holds at least one frame.
```

All three are `ZstdInvalidData`. The [Error Reference](Error-Reference.md) lists every id, and the [Expand-Zstd reference](Expand-Zstd.md) every parameter.
