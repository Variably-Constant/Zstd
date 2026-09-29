---
title: Frames And Records
weight: 3
---

What a zstd frame records, and how the records of a PowerShell pipeline become one. Source: `src/lib.rs`, `compress`, `Compression`, `CompressZstd::take_record` and `Expansion`; the frame format is RFC 8878, section 3.1.

## A frame

zstd data is a sequence of frames. Each frame begins with a header: the magic number `28 B5 2F FD`, a descriptor byte whose bits say which fields follow, and then those fields. The fields that matter here:

- The checksum flag. When it is set, the frame ends with four bytes of a hash of its content, which the expander checks. Every frame this module writes sets it.
- The content size: the length of the content, when the compressor knew it before it began. A frame without it is still whole; the expander learns the length as it goes.
- The dictionary id: the id of the dictionary the frame was compressed with, when it was one in zstd's format. Zero, or no field at all, means none.

The examples below read those fields out of real frames. They ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, and printed the same in both. First, a function that reads a frame header by the rules of RFC 8878:

```powershell
function Get-ZstdFrameHeader([byte[]] $Frame) {
    $descriptor = $Frame[4]
    $singleSegment = ($descriptor -band 0x20) -ne 0
    $at = 5
    if (-not $singleSegment) { $at++ }
    $idBytes = @(0, 1, 2, 4)[$descriptor -band 3]
    $id = [long] 0
    for ($i = 0; $i -lt $idBytes; $i++) { $id += [long] $Frame[$at + $i] -shl (8 * $i) }
    $at += $idBytes
    $sizeBytes = @(0, 2, 4, 8)[$descriptor -shr 6]
    if ($sizeBytes -eq 0 -and $singleSegment) { $sizeBytes = 1 }
    $size = $null
    if ($sizeBytes -gt 0) {
        $size = [long] 0
        for ($i = 0; $i -lt $sizeBytes; $i++) { $size += [long] $Frame[$at + $i] -shl (8 * $i) }
        if ($sizeBytes -eq 2) { $size += 256 }
    }
    [pscustomobject]@{
        Magic        = [System.BitConverter]::ToString($Frame, 0, 4)
        Checksum     = ($descriptor -band 4) -ne 0
        ContentSize  = $size
        DictionaryId = $id
    }
}
```

## One record or many

The same 13 bytes, compressed from one record and then piped one byte per record:

```powershell
[byte[]] $text = [System.Text.Encoding]::UTF8.GetBytes('hello, frames')
Get-ZstdFrameHeader (Compress-Zstd -InputObject $text)
Get-ZstdFrameHeader ($text | Compress-Zstd)
```

```text
Magic       Checksum ContentSize DictionaryId
-----       -------- ----------- ------------
28-B5-2F-FD     True          13            0
28-B5-2F-FD     True                        0
```

Both carry the checksum. Only the first records the content size. `Compress-Zstd` holds the first record it receives and waits to see whether another follows. When none does, it compresses that one `byte[]` in one piece, without copying it, and tells libzstd its length first, so the header records it. When a second record arrives, it begins a frame and feeds each record to libzstd as it comes; the frame has begun before its length is known, so its header has no size. The content is the same either way, and so is what `Expand-Zstd` gives back.

## A file's frame

A file's length is known when it is opened, so its frame records the content size too:

```powershell
Set-Content -Path .\notes.txt -Value ('note ' * 100)
Compress-Zstd -Path .\notes.txt
Get-ZstdFrameHeader ([System.IO.File]::ReadAllBytes("$PWD\notes.txt.zst"))
(Get-Item .\notes.txt).Length
```

```text
Magic       Checksum ContentSize DictionaryId
-----       -------- ----------- ------------
28-B5-2F-FD     True         502            0
502
```

`Compress-Zstd` gives libzstd the length the file had when it was opened, and compresses exactly that many bytes. A file that ends sooner, because something truncated it meanwhile, is refused with `ZstdReadFailed`; bytes appended meanwhile are left out.

## Several frames in one input

Frames can follow one another, and `Expand-Zstd` expands each in turn into one output:

```powershell
$first = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('first frame, '))
$second = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('second frame'))
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject ([byte[]] ($first + $second))))
```

```text
first frame, second frame
```

Bytes after a frame that do not begin another are refused, and nothing is written:

```powershell
[byte[]] $stray = $first + [byte[]] (1, 2, 3)
Expand-Zstd -InputObject $stray -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.Exception.Message
```

```text
Cannot expand the input: it is not valid zstd data (Unknown frame descriptor).
```

The text in parentheses is libzstd's own. [Dictionaries](Dictionaries.md) shows a frame whose header names its dictionary.
