---
title: How To Use A Dictionary
weight: 4
---

Train a dictionary from samples, then compress and expand with it. Source: `src/lib.rs`, `NewZstdDictionary`, `Dictionary`, `given_dictionary` and `refusal`.

A small file compresses poorly on its own, since there is little in it for zstd to find twice. A dictionary holds what files of one kind have in common, trained from samples of them, and both sides load it: the compressor refers into it, and the expander needs the same dictionary to follow those references. [Dictionaries](Dictionaries.md) explains the format and the ids.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, in an empty folder. The output shown is PowerShell 7.6's; Windows PowerShell 5.1 printed the same apart from column widths and line wrapping.

## Samples

300 small JSON records, one per file:

```powershell
New-Item -ItemType Directory -Path .\samples | Out-Null
foreach ($i in 1..300) {
    $role = ('reader', 'writer', 'admin')[$i % 3]
    $active = ($i % 2 -eq 0).ToString().ToLower()
    '{{"id":{0},"name":"user{0}","email":"user{0}@example.com","role":"{1}","active":{2},"quota":{3}}}' -f $i, $role, $active, ($i * 37 % 1000) |
        Set-Content -Path ".\samples\user$i.json"
}
Get-Content .\samples\user42.json
```

```text
{"id":42,"name":"user42","email":"user42@example.com","role":"reader","active":true,"quota":554}
```

## Train

Each file `-Path` names is one sample. The dictionary is written to `-DestinationPath`, at most `-MaxSize` bytes, 112640 unless given:

```powershell
New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\users.dict -PassThru -Verbose | Select-Object Name, Length
```

```text
VERBOSE: Performing the operation "New Dictionary" on target "Destination: C:\Temp\zstd-docs\users.dict".
VERBOSE: Training a dictionary of at most 112640 bytes from 300 samples of 30002 bytes in all.
VERBOSE: Wrote dictionary 1027205458, 21971 bytes, to 'C:\Temp\zstd-docs\users.dict'.

Name       Length
----       ------
users.dict  21971
```

The number in the last verbose line is the dictionary's id, which every frame compressed with it records.

## What it buys

One record, compressed without the dictionary and with it:

```powershell
[byte[]] $one = [System.IO.File]::ReadAllBytes("$PWD\samples\user42.json")
[pscustomobject]@{
    Record         = $one.Length
    Plain          = (Compress-Zstd -InputObject $one).Length
    WithDictionary = (Compress-Zstd -InputObject $one -DictionaryPath .\users.dict).Length
}
```

```text
Record Plain WithDictionary
------ ----- --------------
    98    96             44
```

## Files

`-DictionaryPath` names the dictionary file on both cmdlets, taken exactly as written. All 300 samples, compressed without the dictionary and with it, then expanded with it:

```powershell
New-Item -ItemType Directory -Path .\plain, .\packed, .\unpacked | Out-Null
Compress-Zstd -Path .\samples\*.json -DestinationPath .\plain
Compress-Zstd -Path .\samples\*.json -DestinationPath .\packed -DictionaryPath .\users.dict
Expand-Zstd -Path .\packed\*.zst -DestinationPath .\unpacked -DictionaryPath .\users.dict
[pscustomobject]@{
    Files          = (Get-ChildItem .\unpacked).Count
    Samples        = [long] (Get-ChildItem .\samples | Measure-Object Length -Sum).Sum
    Plain          = [long] (Get-ChildItem .\plain | Measure-Object Length -Sum).Sum
    WithDictionary = [long] (Get-ChildItem .\packed | Measure-Object Length -Sum).Sum
}
```

```text
Files Samples Plain WithDictionary
----- ------- ----- --------------
  300   30002 29675          12241
```

Every file came back as it was; this counts the ones that did not:

```powershell
@(Get-ChildItem .\samples | Where-Object { (Get-FileHash $_.FullName).Hash -ne (Get-FileHash ".\unpacked\$($_.Name)").Hash }).Count
```

```text
0
```

These records were also the training samples, which flatters the dictionary. Train from a sample of your files and compress the rest.

## Bytes

`-Dictionary` takes the dictionary as a `byte[]`, with files or with `-InputObject`:

```powershell
[byte[]] $dictionary = [System.IO.File]::ReadAllBytes("$PWD\users.dict")
$packed = Compress-Zstd -InputObject $one -Dictionary $dictionary
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $packed -Dictionary $dictionary))
```

```text
{"id":42,"name":"user42","email":"user42@example.com","role":"reader","active":true,"quota":554}
```

## Without the dictionary, or with another

A frame compressed with a dictionary names it in its header. Expanding it without that dictionary gets `ZstdDictionaryMismatch`, naming the id the frame needs:

```powershell
Expand-Zstd -InputObject $packed -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
```

```text
ZstdDictionaryMismatch,Pwrs.Modules.Zstd.ExpandZstdCommand
Cannot expand the input: a frame in it needs dictionary 1027205458, and no dictionary was given.
```

With another dictionary, trained here from the same samples with a smaller `-MaxSize`, the message names both:

```powershell
New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\small.dict -MaxSize 4096 -PassThru | Select-Object Name, Length
Expand-Zstd -InputObject $packed -DictionaryPath .\small.dict -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.Exception.Message
```

```text
Name       Length
----       ------
small.dict   4096
Cannot expand the input: a frame in it needs dictionary 1027205458, and dictionary 1389198831 was given.
```

## Any file as raw content

Bytes that do not begin with zstd's dictionary magic number are used as raw content: text the compressor may refer back into, as if it came just before the data. Raw content carries no id, so frames made with it name no dictionary:

```powershell
[byte[]] $raw = [System.IO.File]::ReadAllBytes("$PWD\samples\user1.json")
$packedRaw = Compress-Zstd -InputObject $one -Dictionary $raw
$packedRaw.Length
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $packedRaw -Dictionary $raw))
```

```text
54
{"id":42,"name":"user42","email":"user42@example.com","role":"reader","active":true,"quota":554}
```

## Samples piped to New-ZstdDictionary

Files piped from `Get-ChildItem` bind to `-LiteralPath`, as they do on the other cmdlets:

```powershell
Get-ChildItem .\samples -Filter *.json | New-ZstdDictionary -DestinationPath .\piped.dict -PassThru | Select-Object Name, Length
```

```text
Name       Length
----       ------
piped.dict  21971
```

## When training or loading fails

libzstd's trainer needs at least 7 samples and 8 bytes of them in all. Fewer is `ZstdDictionaryTrainingFailed`, with libzstd's reason, and no file is written:

```powershell
New-ZstdDictionary -Path .\samples\user1.json, .\samples\user2.json -DestinationPath .\few.dict -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
Test-Path .\few.dict
```

```text
ZstdDictionaryTrainingFailed,Pwrs.Modules.Zstd.NewZstdDictionaryCommand
Cannot train a dictionary from 2 samples of 188 bytes in all: libzstd reported 'Src size is incorrect'. Its trainer needs at least 7 samples and 8 bytes of them in all, and refuses 4294967295 bytes or more in all.
False
```

A dictionary in zstd's format is loaded, and libzstd reads its tables, before any input is read. Here the tables of a copy of `users.dict` are overwritten:

```powershell
[byte[]] $broken = $dictionary.Clone()
for ($i = 8; $i -lt 200; $i++) { $broken[$i] = 0xFF }
[System.IO.File]::WriteAllBytes("$PWD\broken.dict", $broken)
Compress-Zstd -InputObject $one -DictionaryPath .\broken.dict -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
```

```text
ZstdDictionaryInvalid,Pwrs.Modules.Zstd.CompressZstdCommand
Cannot use 'C:\Temp\zstd-docs\broken.dict' as a dictionary: it begins with zstd's dictionary magic number, and libzstd refused its tables (Dictionary is corrupted).
```

The command then does nothing more: on files, nothing is read or written.

`-Dictionary` and `-DictionaryPath` belong to different parameter sets, so PowerShell's binder refuses the two together before the cmdlet runs:

```powershell
try {
    Compress-Zstd -InputObject $one -Dictionary $raw -DictionaryPath .\users.dict
} catch {
    $_.FullyQualifiedErrorId
}
```

```text
AmbiguousParameterSet,Pwrs.Modules.Zstd.CompressZstdCommand
```

The parameters are in the [New-ZstdDictionary](New-ZstdDictionary.md), [Compress-Zstd](Compress-Zstd.md) and [Expand-Zstd](Expand-Zstd.md) references.
