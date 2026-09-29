---
title: Expand-Zstd
weight: 2
---

Expands Zstandard-compressed data from files, or bytes. Source: `src/lib.rs`, `ExpandZstd` and `Expansion`.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, in an empty folder. The output shown is PowerShell 7.6's; Windows PowerShell 5.1 printed the same apart from column widths and line wrapping, except where its output is shown too.

## Syntax

```powershell
Get-Command Expand-Zstd -Syntax
```

```text
Expand-Zstd [-Path] <string[]> [[-DestinationPath] <string>] [-DictionaryPath <string>] [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Expand-Zstd [-Path] <string[]> [[-DestinationPath] <string>] -Dictionary <byte[]> [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Expand-Zstd [[-DestinationPath] <string>] -LiteralPath <string[]> [-DictionaryPath <string>] [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Expand-Zstd [[-DestinationPath] <string>] -LiteralPath <string[]> -Dictionary <byte[]> [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Expand-Zstd [-InputObject <byte[]>] [-DictionaryPath <string>] [-MaxMemory <long>] [-WhatIf] [-Confirm] [<CommonParameters>]

Expand-Zstd -Dictionary <byte[]> [-InputObject <byte[]>] [-MaxMemory <long>] [-WhatIf] [-Confirm] [<CommonParameters>]
```

The six lines are the parameter sets, in order: `Path` (the default), `PathDictionary`, `LiteralPath`, `LiteralPathDictionary`, `InputObject` and `InputObjectDictionary`, the same sets as [Compress-Zstd](Compress-Zstd.md) has.

## Description

With `-Path` or `-LiteralPath`, each file named is expanded on its own. A file that fails gets its own error record and the rest go on. A file is expanded a mebibyte at a time into a temporary file. A frame whose window does not fit that way sends the file to be written in place instead: libzstd expands it through a map of the file into a temporary file as long as its frames' content sizes add up to, mapped too, which takes a frame of any window. That needs every frame to record its content size, Windows to map both files, and, with `-MaxMemory`, the maps' page tables to fit; otherwise the frame is refused. The temporary file is beside the destination and is renamed onto it only when every frame in the input has expanded and passed its checksum. A failed or stopped run leaves no partial file, and a file already at the destination survives. The destination is the file's name without its `.zst`, beside it, unless `-DestinationPath` names an existing folder to write into or one file, which only the first input of the command may take. A file whose name does not end in `.zst` needs `-DestinationPath`.

With `-InputObject`, the result is written as one `byte[]` when the pipeline ends. When every frame in the first record records its content size, as the frames `Compress-Zstd` writes from a file or from a single `byte[]` do, the host makes a `byte[]` as long as the sizes add up to, libzstd expands the record straight into it, and that array is the one written; libzstd keeps no window of its own that way, so a frame of any window expands. Other bytes, and the records after the first, are fed to libzstd as they arrive. The first record is fed that way too when libzstd cannot add up its frames' sizes, such as when it ends inside a frame, or when the host cannot make an array that long, and the verbose stream then says why. When no record arrives, nothing is written, output or error. A `$null` record gets a `ZstdNoInput` error record, and the records around it are expanded as if it had not been sent. The command writes the same error when PowerShell's binder chooses the `InputObject` set with no `-InputObject` given, as it does for `-Dictionary` alone.

Every frame in the input is expanded, in order. Input that is empty, ends inside a frame, holds bytes after a frame that do not begin another, fails a frame's checksum, has a frame whose header records a content size other than what it holds, or is in a format from before zstd 1.0 is refused with `ZstdInvalidData`, and nothing is written. A frame whose header names a dictionary other than the one given, or names one when none was given, is refused with `ZstdDictionaryMismatch`, whose message names the id the frame needs and what was given.

A frame expanded a mebibyte at a time is checked before libzstd sees it: without `-MaxMemory`, a window over 128 MiB, libzstd's own limit, does not fit; with it, a frame that takes more to expand than the budget does not. A file with such a frame is written in place when it can be, and is otherwise refused with `ZstdWindowTooLarge`, as are bytes fed to libzstd as they arrive.

## Parameters

### -Path

| | |
|---|---|
| Type | `System.String[]` |
| Parameter sets | `Path`, `PathDictionary`: mandatory, position 0 |
| Pipeline input | no |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value; `ZstdInputNotFound` for a value that names nothing or whose wildcards match nothing; `ZstdPathNotResolved` for a path the session cannot resolve; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be opened or read; `ZstdNoDestination` for a file not named `.zst`, without `-DestinationPath` |

The files to expand. Wildcard characters select every file they match, and a relative path is resolved against the current PowerShell location. A value that matches nothing is a `ZstdInputNotFound` record of its own.

### -LiteralPath

| | |
|---|---|
| Type | `System.String[]` |
| Aliases | `PSPath`, `LP` |
| Parameter sets | `LiteralPath`, `LiteralPathDictionary`: mandatory, named |
| Pipeline input | by property name (`PSPath`, `LP`, `LiteralPath`) |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value; `ZstdInputNotFound` for a file that is not there; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be opened or read; `ZstdNoDestination` for a file not named `.zst`, without `-DestinationPath` |

The files to expand, each named exactly as written, wildcard characters included. A file piped from `Get-ChildItem` or `Get-Item` binds here by its `PSPath` property.

### -DestinationPath

| | |
|---|---|
| Type | `System.String` |
| Parameter sets | `Path`, `LiteralPath`, `PathDictionary`, `LiteralPathDictionary`: optional, position 1 |
| Pipeline input | no |
| Default | none: each file is written beside itself under its name without `.zst` |
| Refused with | `ZstdDestinationExists` for a file that exists, without `-Force`; `ZstdDestinationIsDirectory` where a folder stands in place of the file; `ZstdDestinationReused` for a second input to one file; `ZstdNoDestination` for a folder given for a file not named `.zst`; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdWriteFailed` when the file cannot be written |

Where to write. An existing folder receives each file under its name without `.zst`. Any other path names one file, which only the first input of the command may take; a later input is refused with `ZstdDestinationReused`.

### -InputObject

| | |
|---|---|
| Type | `System.Byte[]` |
| Parameter sets | `InputObject`, `InputObjectDictionary`: optional, named |
| Pipeline input | by value |
| Validation | an empty `byte[]` is allowed |
| Refused with | `InputObjectNotBound` from the binder for a record it cannot make a `byte[]`; `ZstdNoInput` for a `$null` record, and when the binder chooses this set with no `-InputObject` given; `ZstdInvalidData` for data that is not whole zstd frames; `AmbiguousParameterSet` from the binder beside `-DestinationPath`, `-Force` or `-PassThru` |

The zstd data to expand. PowerShell's binder converts each pipeline record to a `byte[]` first, as for [Compress-Zstd](Compress-Zstd.md#-inputobject), and a `byte[]` another command wrote binds as the array itself. Data with no bytes in it at all is refused with `ZstdInvalidData`, since it holds no frame, and a `$null` record gets `ZstdNoInput`.

### -DictionaryPath

| | |
|---|---|
| Type | `System.String` |
| Parameter sets | `Path`, `LiteralPath`, `InputObject`: optional, named |
| Pipeline input | no |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value, and `AmbiguousParameterSet` beside `-Dictionary`; `ZstdInputNotFound` for a file that is not there; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be read; `ZstdDictionaryInvalid` for a dictionary in zstd's format whose tables libzstd refuses; `ZstdOutOfMemory` when its bytes cannot be held; `ZstdDictionaryMismatch` for a frame that names another dictionary |

The dictionary the data was compressed with, as a file named exactly as written. It is loaded, and checked by libzstd when it is in zstd's format, before any input is read.

### -Dictionary

| | |
|---|---|
| Type | `System.Byte[]` |
| Parameter sets | `PathDictionary`, `LiteralPathDictionary`, `InputObjectDictionary`: mandatory, named |
| Pipeline input | no |
| Refused with | `AmbiguousParameterSet` from the binder beside `-DictionaryPath`; `ZstdDictionaryInvalid` for a dictionary in zstd's format whose tables libzstd refuses; `ZstdOutOfMemory` when its bytes cannot be held; `ZstdDictionaryMismatch` for a frame that names another dictionary |

The dictionary the data was compressed with, as a `byte[]`, taken as `-DictionaryPath` takes a file's bytes.

### -MaxMemory

| | |
|---|---|
| Type | `System.Int64` |
| Parameter sets | all |
| Pipeline input | no |
| Validation | 1048576 to 9223372036854775807 |
| Default | none: a frame expanded a mebibyte at a time may have a window of up to 128 MiB, libzstd's own limit |
| Refused with | `ParameterArgumentValidationError` from the binder below 1048576; `ZstdWindowTooLarge` for a frame whose window does not fit; `ZstdBudgetTooSmall` for a dictionary, or a result on bytes, that does not fit |

The most memory, in bytes, the command may commit. It counts what libzstd takes for a frame's window, from libzstd's own estimate of the frame's header; a dictionary, the command's copy and libzstd's, refused with `ZstdBudgetTooSmall` when it does not fit with libzstd's context before any frame; the command's own buffers and what the host takes while it runs; the page tables of a file's map, 4 KB for each 2 MiB of it; and, on bytes, the result. The result is held in memory once when the bytes are one record whose frames all record their content size, expanded straight into the `byte[]` written, which takes libzstd's context and no window; otherwise it is held twice, as it is built and as the `byte[]` written. A frame that does not fit is refused with `ZstdWindowTooLarge` before libzstd sees it, unless its file can be written in place, which takes a frame of any window: every frame records its content size, and the page tables of both files' maps fit with libzstd's context and the rest. On bytes, a result the frames' headers record that would not fit is refused with `ZstdBudgetTooSmall` before libzstd sees them, and a result that outgrows the budget stops the command with it.

### -Force

| | |
|---|---|
| Type | `System.Management.Automation.SwitchParameter` |
| Parameter sets | `Path`, `LiteralPath`, `PathDictionary`, `LiteralPathDictionary` |
| Refused with | `AmbiguousParameterSet` from the binder beside `-InputObject` |

Replaces a destination file that exists. Without it, an existing destination is refused with `ZstdDestinationExists`.

### -PassThru

| | |
|---|---|
| Type | `System.Management.Automation.SwitchParameter` |
| Parameter sets | `Path`, `LiteralPath`, `PathDictionary`, `LiteralPathDictionary` |
| Refused with | `AmbiguousParameterSet` from the binder beside `-InputObject` |

Writes the `FileInfo` of each file written, as `Get-Item` gives it.

### -WhatIf, -Confirm

`-WhatIf` names each file and its destination and writes nothing; `-Confirm` asks before each file. Both act on files only: on bytes the cmdlet does not ask, and the result is written either way.

## Inputs

- `System.Byte[]`, by value, to `-InputObject`.
- Any object with a `PSPath` property, such as the `System.IO.FileInfo` that `Get-ChildItem` writes, by property name to `-LiteralPath`.

## Outputs

- `System.IO.FileInfo`, one for each file written, with `-PassThru` on files.
- `System.Byte[]`, one for the whole pipeline, on bytes.

Some files to expand, and a few bytes compressed:

```powershell
$random = [System.Random]::new(3)
$words = -split 'GET POST PUT DELETE /api/users /api/orders /api/items /login /health 200 201 204 304 400 404 500'
New-Item -ItemType Directory -Path .\logs, .\archive | Out-Null
foreach ($name in 'app', 'logs\api', 'logs\web') {
    1..5000 | ForEach-Object {
        '12:{0:d2}:{1:d2} {2} {3} {4} {5}ms' -f ($_ % 60), ($_ * 7 % 60), $words[$random.Next(0, 4)], $words[$random.Next(4, 9)], $words[$random.Next(9, 16)], $random.Next(1, 900)
    } | Set-Content -Path ".\$name.log"
}
Compress-Zstd -Path .\app.log
Compress-Zstd -Path .\logs\*.log -DestinationPath .\archive
Remove-Item .\app.log
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('hello'))
```

The `FileInfo` is the one `Get-Item` gives, with the provider's properties such as `PSPath`:

```powershell
$written = Expand-Zstd -Path .\archive\api.log.zst -DestinationPath .\members.log -PassThru
$written.GetType().FullName
$written | Get-Member -MemberType Properties | Format-Table -Wrap
```

```text
System.IO.FileInfo

   TypeName: System.IO.FileInfo

Name                MemberType     Definition
----                ----------     ----------
Target              AliasProperty  Target = LinkTarget
LinkType            CodeProperty   System.String LinkType{get=GetLinkType;}
Mode                CodeProperty   System.String Mode{get=Mode;}
ModeWithoutHardLink CodeProperty   System.String ModeWithoutHardLink{get=ModeWithoutHardLink;}
ResolvedTarget      CodeProperty   System.String ResolvedTarget{get=ResolvedTarget;}
PSChildName         NoteProperty   string PSChildName=members.log
PSDrive             NoteProperty   PSDriveInfo PSDrive=C
PSIsContainer       NoteProperty   bool PSIsContainer=False
PSParentPath        NoteProperty   string PSParentPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs
PSPath              NoteProperty   string PSPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs\members.log
PSProvider          NoteProperty   ProviderInfo PSProvider=Microsoft.PowerShell.Core\FileSystem
Attributes          Property       System.IO.FileAttributes Attributes {get;set;}
CreationTime        Property       datetime CreationTime {get;set;}
CreationTimeUtc     Property       datetime CreationTimeUtc {get;set;}
Directory           Property       System.IO.DirectoryInfo Directory {get;}
DirectoryName       Property       string DirectoryName {get;}
Exists              Property       bool Exists {get;}
Extension           Property       string Extension {get;}
FullName            Property       string FullName {get;}
IsReadOnly          Property       bool IsReadOnly {get;set;}
LastAccessTime      Property       datetime LastAccessTime {get;set;}
LastAccessTimeUtc   Property       datetime LastAccessTimeUtc {get;set;}
LastWriteTime       Property       datetime LastWriteTime {get;set;}
LastWriteTimeUtc    Property       datetime LastWriteTimeUtc {get;set;}
Length              Property       long Length {get;}
LinkTarget          Property       string LinkTarget {get;}
Name                Property       string Name {get;}
UnixFileMode        Property       System.IO.UnixFileMode UnixFileMode {get;set;}
BaseName            ScriptProperty System.Object BaseName {get=if ($this.Extension.Length -gt 0){$this.Name.Remove($this.Name.Length - $this.Extension.Length)}else{$this.Name};}
VersionInfo         ScriptProperty System.Object VersionInfo {get=[System.Diagnostics.FileVersionInfo]::GetVersionInfo($this.FullName);}
```

In Windows PowerShell 5.1:

```text
System.IO.FileInfo

   TypeName: System.IO.FileInfo

Name              MemberType     Definition
----              ----------     ----------
LinkType          CodeProperty   System.String LinkType{get=GetLinkType;}
Mode              CodeProperty   System.String Mode{get=Mode;}
Target            CodeProperty   System.Collections.Generic.IEnumerable`1[[System.String, mscorlib, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089]] Target{get=GetTarget;}
PSChildName       NoteProperty   string PSChildName=members.log
PSDrive           NoteProperty   PSDriveInfo PSDrive=C
PSIsContainer     NoteProperty   bool PSIsContainer=False
PSParentPath      NoteProperty   string PSParentPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs
PSPath            NoteProperty   string PSPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs\members.log
PSProvider        NoteProperty   ProviderInfo PSProvider=Microsoft.PowerShell.Core\FileSystem
Attributes        Property       System.IO.FileAttributes Attributes {get;set;}
CreationTime      Property       datetime CreationTime {get;set;}
CreationTimeUtc   Property       datetime CreationTimeUtc {get;set;}
Directory         Property       System.IO.DirectoryInfo Directory {get;}
DirectoryName     Property       string DirectoryName {get;}
Exists            Property       bool Exists {get;}
Extension         Property       string Extension {get;}
FullName          Property       string FullName {get;}
IsReadOnly        Property       bool IsReadOnly {get;set;}
LastAccessTime    Property       datetime LastAccessTime {get;set;}
LastAccessTimeUtc Property       datetime LastAccessTimeUtc {get;set;}
LastWriteTime     Property       datetime LastWriteTime {get;set;}
LastWriteTimeUtc  Property       datetime LastWriteTimeUtc {get;set;}
Length            Property       long Length {get;}
Name              Property       string Name {get;}
BaseName          ScriptProperty System.Object BaseName {get=if ($this.Extension.Length -gt 0){$this.Name.Remove($this.Name.Length - $this.Extension.Length)}else{$this.Name};}
VersionInfo       ScriptProperty System.Object VersionInfo {get=[System.Diagnostics.FileVersionInfo]::GetVersionInfo($this.FullName);}
```

On bytes the output is a plain `byte[]`:

```powershell
$bytes = Expand-Zstd -InputObject $packed
$bytes.GetType().FullName
Get-Member -InputObject $bytes -MemberType Properties | Format-Table -Wrap
```

```text
System.Byte[]

   TypeName: System.Byte[]

Name           MemberType Definition
----           ---------- ----------
Count          Property   int Count {get;}
IsFixedSize    Property   bool IsFixedSize {get;}
IsReadOnly     Property   bool IsReadOnly {get;}
IsSynchronized Property   bool IsSynchronized {get;}
Length         Property   int Length {get;}
LongLength     Property   long LongLength {get;}
Rank           Property   int Rank {get;}
SyncRoot       Property   System.Object SyncRoot {get;}
```

In Windows PowerShell 5.1:

```text
System.Byte[]

   TypeName: System.Byte[]

Name           MemberType    Definition
----           ----------    ----------
Count          AliasProperty Count = Length
IsFixedSize    Property      bool IsFixedSize {get;}
IsReadOnly     Property      bool IsReadOnly {get;}
IsSynchronized Property      bool IsSynchronized {get;}
Length         Property      int Length {get;}
LongLength     Property      long LongLength {get;}
Rank           Property      int Rank {get;}
SyncRoot       Property      System.Object SyncRoot {get;}
```

## Examples

The examples from the cmdlet's help, on the files made above.

Expand one file beside itself. Without `-PassThru` nothing is written to the pipeline:

```powershell
Expand-Zstd -Path .\app.log.zst
Get-Item .\app.log | Select-Object Name, Length
```

```text
Name    Length
----    ------
app.log 173504
```

Expand every file a wildcard matches into a folder, and list what was written:

```powershell
New-Item -ItemType Directory -Path .\restored | Out-Null
Expand-Zstd -Path .\archive\*.zst -DestinationPath .\restored -PassThru | Select-Object Name, Length
```

```text
Name    Length
----    ------
api.log 173591
web.log 173278
```

Expand files piped from `Get-ChildItem` into a folder:

```powershell
New-Item -ItemType Directory -Path .\restored-again | Out-Null
Get-ChildItem .\archive -Filter *.zst | Expand-Zstd -DestinationPath .\restored-again
Get-ChildItem .\restored-again | Select-Object Name, Length
```

```text
Name    Length
----    ------
api.log 173591
web.log 173278
```

Expand bytes:

```powershell
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $packed))
```

```text
hello
```

See what would be written, and the sizes of what was:

```powershell
Expand-Zstd -Path .\archive\web.log.zst -DestinationPath .\whatif.log -WhatIf
Test-Path .\whatif.log
```

```text
What if: Performing the operation "Expand File" on target "Item: C:\Temp\zstd-docs\archive\web.log.zst Destination: C:\Temp\zstd-docs\whatif.log".
False
```

```powershell
Expand-Zstd -Path .\archive\web.log.zst -DestinationPath .\web.log -Verbose
```

```text
VERBOSE: Performing the operation "Expand File" on target "Item: C:\Temp\zstd-docs\archive\web.log.zst Destination: C:\Temp\zstd-docs\web.log".
VERBOSE: Expanded 'C:\Temp\zstd-docs\archive\web.log.zst' (36794 bytes) to 'C:\Temp\zstd-docs\web.log' (173278 bytes).
```

Expand bytes within a memory budget. The frame records its content size, so libzstd expands it straight into the `byte[]` written, and the budget counts that result once, with libzstd's context and the command's own share. A budget too small for them is refused before libzstd sees the frame, and one that holds them is not:

```powershell
$log = [System.IO.File]::ReadAllBytes("$PWD\app.log")
$frame = Compress-Zstd -InputObject $log -Level 19
try {
    Expand-Zstd -InputObject $frame -MaxMemory 4MB -ErrorAction Stop
} catch {
    $_.FullyQualifiedErrorId
    $_.Exception.Message
}
(Expand-Zstd -InputObject $frame -MaxMemory 16MB).Length
```

```text
ZstdBudgetTooSmall,Pwrs.Modules.Zstd.ExpandZstdCommand
Cannot expand the input within -MaxMemory of 4194304 bytes: it needs about 4594848 bytes, 173504 of them for its result, as its frames record, which is held in memory once, as the byte[] it is expanded into.
173504
```

A frame that records no content size, such as one `Compress-Zstd` wrote from several records, is fed to libzstd a mebibyte at a time instead, and the budget counts its window, and its result twice.

## Errors

`ZstdInputNotFound`, `ZstdInputIsDirectory`, `ZstdReadFailed`, `ZstdPathNotResolved`, `ZstdDestinationExists`, `ZstdDestinationIsDirectory`, `ZstdDestinationReused`, `ZstdNoDestination`, `ZstdWriteFailed`, `ZstdInvalidData`, `ZstdDictionaryMismatch`, `ZstdDictionaryInvalid`, `ZstdWindowTooLarge`, `ZstdBudgetTooSmall`, `ZstdInputNotBytes` and `ZstdOutOfMemory`, each described in the [Error Reference](Error-Reference.md). [How To Expand Files](How-To-Expand-Files.md) shows the ones data that is not whole gets.
