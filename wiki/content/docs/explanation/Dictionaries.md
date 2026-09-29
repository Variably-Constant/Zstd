---
title: Dictionaries
weight: 4
---

What a dictionary is, the two kinds the module takes, and how a frame names the one it needs. Source: `src/lib.rs`, `Dictionary`, `given_dictionary`, `refusal` and `NewZstdDictionary`.

## What a dictionary does

zstd compresses by finding data it has seen before and referring back to it. At the start of an input it has seen nothing, so a small input, a few hundred bytes of JSON, has almost nothing to refer back to and barely shrinks. A dictionary is data both sides agree on beforehand: the compressor may refer into it from the first byte, and the expander, holding the same dictionary, follows those references. One trained from samples of a kind of data holds what those samples share, such as field names and repeated values.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, and printed the same in both apart from line wrapping. They train on 300 small JSON records:

```powershell
New-Item -ItemType Directory -Path .\samples | Out-Null
foreach ($i in 1..300) {
    $role = ('reader', 'writer', 'admin')[$i % 3]
    $active = ($i % 2 -eq 0).ToString().ToLower()
    '{{"id":{0},"name":"user{0}","email":"user{0}@example.com","role":"{1}","active":{2},"quota":{3}}}' -f $i, $role, $active, ($i * 37 % 1000) |
        Set-Content -Path ".\samples\user$i.json"
}
New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\users.dict -Verbose
```

```text
VERBOSE: Performing the operation "New Dictionary" on target "Destination: C:\Temp\zstd-docs\users.dict".
VERBOSE: Training a dictionary of at most 112640 bytes from 300 samples of 30002 bytes in all.
VERBOSE: Wrote dictionary 1027205458, 21971 bytes, to 'C:\Temp\zstd-docs\users.dict'.
```

## Two kinds

The module takes a dictionary in either of two forms, from a file with `-DictionaryPath` or as bytes with `-Dictionary`, and tells them apart by their first bytes.

A dictionary in zstd's format, which is what `New-ZstdDictionary` trains, begins with zstd's dictionary magic number, `0xEC30A437` stored little-endian, and then a four-byte id. After those come tables that set libzstd up for the data it was trained on, and then content to refer into:

```powershell
[byte[]] $dictionary = [System.IO.File]::ReadAllBytes("$PWD\users.dict")
[System.BitConverter]::ToString($dictionary, 0, 4)
[System.BitConverter]::ToUInt32($dictionary, 4)
```

```text
37-A4-30-EC
1027205458
```

Any other bytes are raw content: nothing but data to refer into, as if it came just before the input. Raw content has no id and no tables.

## A frame names its dictionary

A frame compressed with a dictionary in zstd's format records the dictionary's id in its header. Here the header's descriptor byte, `27`, says a four-byte dictionary id follows, and it is `52 E9 39 3D`, which is 1027205458 read little-endian:

```powershell
[byte[]] $one = [System.IO.File]::ReadAllBytes("$PWD\samples\user42.json")
[byte[]] $frame = Compress-Zstd -InputObject $one -Dictionary $dictionary
[System.BitConverter]::ToString($frame, 0, 10)
```

```text
28-B5-2F-FD-27-52-E9-39-3D-62
```

That is how `Expand-Zstd` knows which dictionary a frame needs, and why a missing or different one is refused with `ZstdDictionaryMismatch`, naming the id:

```powershell
Expand-Zstd -InputObject $frame -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.Exception.Message
```

```text
Cannot expand the input: a frame in it needs dictionary 1027205458, and no dictionary was given.
```

A frame compressed with raw content records no id, so nothing in it says a dictionary was used. Expanded without the content, its references point at nothing, and libzstd finds the data corrupt:

```powershell
[byte[]] $raw = [System.Text.Encoding]::UTF8.GetBytes('{"id":0,"name":"user0","email":"user0@example.com","role":"reader","active":true,"quota":0}')
[byte[]] $withRaw = Compress-Zstd -InputObject $one -Dictionary $raw
[System.BitConverter]::ToString($withRaw, 0, 6)
Expand-Zstd -InputObject $withRaw -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.Exception.Message
```

```text
28-B5-2F-FD-24-62
Cannot expand the input: it is not valid zstd data (Data corruption detected).
```

Its descriptor byte, `24`, says no dictionary id follows.

## What each buys

The same record, with no dictionary, with one other record as raw content, and with the trained dictionary:

```powershell
[pscustomobject]@{
    Record         = $one.Length
    Plain          = (Compress-Zstd -InputObject $one).Length
    RawContent     = $withRaw.Length
    Trained        = $frame.Length
}
```

```text
Record Plain RawContent Trained
------ ----- ---------- -------
    98    96         42      44
```

Raw content made the smaller frame here, because the one record given was nearly the same as the one compressed, and its frame has no four-byte id. The trained dictionary is not tied to one record: all 300 records compressed to 12241 bytes with it, against 29675 without, as [How To Use A Dictionary](How-To-Use-A-Dictionary.md) shows.

## Checked before use

A dictionary in zstd's format is loaded before any input is read, and libzstd reads its tables both ways, compressing nothing with it and expanding an empty frame with it. A dictionary whose tables libzstd refuses stops the command there, with `ZstdDictionaryInvalid`, before anything is read or written. Raw content has no tables, so there is nothing to check. [How To Use A Dictionary](How-To-Use-A-Dictionary.md) shows both refusals and the rest of the dictionary tasks.
