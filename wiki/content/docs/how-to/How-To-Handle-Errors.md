---
title: How To Handle Errors
weight: 5
---

What a failure looks like, and how a script collects, stops on, or acts on one. Source: `src/lib.rs`, the functions that build error records, among them `failed`, `open_input`, `input_error`, `output_error`, `matched_files`, `check_destination`, `destination`, `dictionary_refused` and `training_failed`.

Every failure the module reports is an error record with an id of its own and a category, listed in the [Error Reference](Error-Reference.md). A failure on one file is a non-terminating error: the command reports it and goes on with the next file. A dictionary that cannot be loaded is reported once, before any input is read, and the command then does nothing more.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, in an empty folder. The output shown is PowerShell 7.6's; Windows PowerShell 5.1 printed the same apart from column widths.

## Collecting errors while the rest goes on

`-ErrorVariable` collects the command's error records; `-ErrorAction SilentlyContinue` keeps them off the screen. Of three files, the missing one fails and the other two are written:

```powershell
New-Item -ItemType Directory -Path .\in, .\out | Out-Null
'alpha' | Set-Content -Path .\in\a.txt
'beta' | Set-Content -Path .\in\b.txt
Compress-Zstd -Path .\in\a.txt, .\in\missing.txt, .\in\b.txt -DestinationPath .\out -PassThru -ErrorVariable failed -ErrorAction SilentlyContinue | Select-Object Name, Length
$failed.Count
```

```text
Name      Length
----      ------
a.txt.zst     20
b.txt.zst     19
1
```

## What a record carries

The id is the first part of `FullyQualifiedErrorId`. The target is the path the record is about; records about bytes on the pipeline have none:

```powershell
$failed[0] | Select-Object FullyQualifiedErrorId, CategoryInfo, TargetObject, @{ Name = 'Message'; Expression = { $_.Exception.Message } } | Format-List
```

```text
FullyQualifiedErrorId : ZstdInputNotFound,Pwrs.Modules.Zstd.CompressZstdCommand
CategoryInfo          : ObjectNotFound: (.\in\missing.txt:String) [Compress-Zstd], PwrsException
TargetObject          : .\in\missing.txt
Message               : Cannot find '.\in\missing.txt'.
```

## Stopping on the first error

`-ErrorAction Stop` turns the first error into an exception that `try` and `catch` see:

```powershell
try {
    Compress-Zstd -Path .\in\a.txt -DestinationPath .\out -ErrorAction Stop
    'not reached'
} catch {
    $_.FullyQualifiedErrorId
    "$($_.CategoryInfo.Category)"
    $_.Exception.Message
}
```

```text
ZstdDestinationExists,Pwrs.Modules.Zstd.CompressZstdCommand
ResourceExists
'C:\Temp\zstd-docs\out\a.txt.zst' already exists. Use -Force to replace it.
```

## Acting on the id

The id says what went wrong without parsing the message. Here files already compressed are left alone, and anything else is raised again:

```powershell
'gamma' | Set-Content -Path .\in\c.txt
Compress-Zstd -Path .\in\*.txt -DestinationPath .\out -PassThru -ErrorVariable failed -ErrorAction SilentlyContinue | Select-Object Name, Length
foreach ($record in $failed) {
    switch -Wildcard ($record.FullyQualifiedErrorId) {
        'ZstdDestinationExists,*' { 'kept the existing ' + $record.TargetObject }
        default { throw $record }
    }
}
```

```text
Name      Length
----      ------
c.txt.zst     20
kept the existing C:\Temp\zstd-docs\out\a.txt.zst
kept the existing C:\Temp\zstd-docs\out\b.txt.zst
```

## Refusals from PowerShell's binder

Some arguments never reach the cmdlet. PowerShell's parameter binder checks the declared ranges and parameter sets first, and its errors carry its own ids. An argument out of range ends the command:

```powershell
try {
    Compress-Zstd -Path .\in\a.txt -Level 23
} catch {
    $_.FullyQualifiedErrorId
    $_.Exception.Message
}
```

```text
ParameterArgumentValidationError,Pwrs.Modules.Zstd.CompressZstdCommand
Cannot validate argument on parameter 'Level'. The 23 argument is greater than the maximum allowed range of 22. Supply an argument that is less than or equal to 22 and then try the command again.
```

A pipeline record the binder cannot make a `byte[]` of gets `InputObjectNotBound` and is skipped, while the records around it go through:

```powershell
$output = 'text', ([byte[]] (1, 2, 3)) | Compress-Zstd -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
(Expand-Zstd -InputObject $output) -join ','
```

```text
InputObjectNotBound,Pwrs.Modules.Zstd.CompressZstdCommand
1,2,3
```

Two parameters from different parameter sets, such as `-InputObject` with `-DestinationPath`, or `-Dictionary` with `-DictionaryPath`, get `AmbiguousParameterSet`, and end the command.
