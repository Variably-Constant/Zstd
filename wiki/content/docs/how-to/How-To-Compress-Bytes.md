---
title: How To Compress Bytes
weight: 3
---

`Compress-Zstd` and `Expand-Zstd` on bytes in memory, through `-InputObject`. Source: `src/lib.rs`, `CompressZstd::take_record` and `finish_piped`, `ExpandZstd::take_record` and `finish_piped`, `expand_whole`, and `Compression` and `Expansion`.

On bytes, each cmdlet writes one `byte[]` to the pipeline when the pipeline ends. The input stays in memory as it arrives, and so does the output until it is written.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, and printed the same in both.

## Text

`-InputObject` takes a `byte[]`, so text goes through an encoding each way:

```powershell
$text = 'The quick brown fox jumps over the lazy dog. ' * 20
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes($text))
'{0} bytes of text, {1} bytes compressed, as a {2}' -f $text.Length, $packed.Length, $packed.GetType().FullName
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $packed)) -eq $text
```

```text
900 bytes of text, 68 bytes compressed, as a System.Byte[]
True
```

## Between bytes and files

A frame is the same whether it came from a file or from bytes, so either cmdlet reads what the other wrote:

```powershell
[System.IO.File]::WriteAllBytes("$PWD\hello.txt.zst", (Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('hello from bytes'))))
Expand-Zstd -Path .\hello.txt.zst
Get-Content .\hello.txt
```

```text
hello from bytes
```

```powershell
Set-Content -Path .\notes.txt -Value 'notes from a file'
Compress-Zstd -Path .\notes.txt
$expanded = Expand-Zstd -InputObject ([System.IO.File]::ReadAllBytes("$PWD\notes.txt.zst"))
[System.Text.Encoding]::UTF8.GetString($expanded).TrimEnd()
```

```text
notes from a file
```

## How the bytes arrive

The pipeline delivers records, and a `byte[]` held in a variable is enumerated when it is piped: `$bytes | Compress-Zstd` sends one record per byte, each arriving as a `byte[]` of one. `-InputObject $bytes`, or a comma in front (`, $bytes | Compress-Zstd`), sends the array whole. All three give the same data back, in frames that differ by one byte:

```powershell
[byte[]] $bytes = [System.Text.Encoding]::UTF8.GetBytes('abc' * 1000)
$whole = Compress-Zstd -InputObject $bytes
$piped = , $bytes | Compress-Zstd
$each = $bytes | Compress-Zstd
[pscustomobject]@{ Sent = '-InputObject $bytes'; Records = 1; FrameBytes = $whole.Length }
[pscustomobject]@{ Sent = ', $bytes | Compress-Zstd'; Records = 1; FrameBytes = $piped.Length }
[pscustomobject]@{ Sent = '$bytes | Compress-Zstd'; Records = $bytes.Length; FrameBytes = $each.Length }
```

```text
Sent                     Records FrameBytes
----                     ------- ----------
-InputObject $bytes            1         24
, $bytes | Compress-Zstd       1         24
$bytes | Compress-Zstd      3000         23
```

```powershell
(Expand-Zstd -InputObject $each).Length
```

```text
3000
```

A single record is compressed in one piece, without being copied, and its frame's header records the content size. From a second record on, each record is fed to libzstd as it arrives; that frame begins before its length is known, so its header holds no content size. [Frames And Records](Frames-And-Records.md) reads both headers.

Several arrays piped together are joined in order into one frame:

```powershell
$parts = [System.Text.Encoding]::UTF8.GetBytes('first part, '), [System.Text.Encoding]::UTF8.GetBytes('second part')
$frame = $parts | Compress-Zstd
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $frame))
```

```text
first part, second part
```

`Expand-Zstd` takes a first record whole when every frame in it records its content size, as the frame of a single record does: libzstd expands it straight into a `byte[]` as long as the sizes add up to, and that array is the one written. Any other record, such as the frame of several records, is fed to libzstd as it arrives, and gives the same bytes back.

## Seeing the sizes

`-Verbose` reports both sizes, and for compression the level and the worker threads:

```powershell
$frame = Compress-Zstd -InputObject $bytes -Level 19 -Verbose
$back = Expand-Zstd -InputObject $frame -Verbose
```

```text
VERBOSE: Compressed 3000 bytes to 24 bytes at level 19 with 0 worker threads.
VERBOSE: Expanded 24 bytes to 3000 bytes.
```

## Empty input, numbers and text

An empty `byte[]` compresses to a frame of nothing, which expands to an empty `byte[]`:

```powershell
$empty = Compress-Zstd -InputObject ([byte[]]::new(0))
$empty.Length
(Expand-Zstd -InputObject $empty).Length
```

```text
13
0
```

PowerShell's parameter binder converts each record to a `byte[]` before the cmdlet sees it. A number from 0 to 255 becomes a `byte[]` of one:

```powershell
$small = 1, 2, 3 | Compress-Zstd
(Expand-Zstd -InputObject $small) -join ','
```

```text
1,2,3
```

A record the binder cannot convert, such as text, gets the binder's own error, `InputObjectNotBound`, and never reaches the cmdlet. The records around it are compressed as if it had not been sent. When no record reaches a cmdlet at all, it writes nothing, output or error.

```powershell
'text' | Compress-Zstd -ErrorVariable refused -ErrorAction SilentlyContinue
$refused.FullyQualifiedErrorId
```

```text
InputObjectNotBound,Pwrs.Modules.Zstd.CompressZstdCommand
```

A `$null` record reaches the cmdlet, which refuses it with `ZstdNoInput` and goes on with the records around it:

```powershell
$joined = ([byte[]] (1, 2)), $null, ([byte[]] (3)) | Compress-Zstd -ErrorVariable refused -ErrorAction SilentlyContinue
$refused.FullyQualifiedErrorId
(Expand-Zstd -InputObject $joined) -join ','
```

```text
ZstdNoInput,Pwrs.Modules.Zstd.CompressZstdCommand
1,2,3
```

## Several frames

`Expand-Zstd` expands every frame in its input, in order, into one `byte[]`:

```powershell
$frames = (Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('one '))) + (Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('two')))
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject ([byte[]] $frames)))
```

```text
one two
```

## -WhatIf and bytes

`-WhatIf` and `-Confirm` act on files, where the cmdlets write something other than their output. On bytes neither cmdlet asks, so `-WhatIf` changes nothing and the frame is still written:

```powershell
$whatIf = Compress-Zstd -InputObject $bytes -WhatIf
$whatIf.Length
```

```text
24
```

Every parameter, and which ones `-InputObject` combines with, is in the [Compress-Zstd](Compress-Zstd.md) and [Expand-Zstd](Expand-Zstd.md) references.
