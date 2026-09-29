---
title: Compress-Zstd
weight: 1
---

Compresses files, or bytes, with Zstandard. Source: `src/lib.rs`, `CompressZstd`.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, in an empty folder. The output shown is PowerShell 7.6's; Windows PowerShell 5.1 printed the same apart from column widths, except where its output is shown too.

## Syntax

```powershell
Get-Command Compress-Zstd -Syntax
```

```text
Compress-Zstd [-Path] <string[]> [[-DestinationPath] <string>] [-DictionaryPath <string>] [-Level <int>] [-Threads <int>] [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Compress-Zstd [-Path] <string[]> [[-DestinationPath] <string>] -Dictionary <byte[]> [-Level <int>] [-Threads <int>] [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Compress-Zstd [[-DestinationPath] <string>] -LiteralPath <string[]> [-DictionaryPath <string>] [-Level <int>] [-Threads <int>] [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Compress-Zstd [[-DestinationPath] <string>] -LiteralPath <string[]> -Dictionary <byte[]> [-Level <int>] [-Threads <int>] [-MaxMemory <long>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

Compress-Zstd [-InputObject <byte[]>] [-DictionaryPath <string>] [-Level <int>] [-Threads <int>] [-MaxMemory <long>] [-WhatIf] [-Confirm] [<CommonParameters>]

Compress-Zstd -Dictionary <byte[]> [-InputObject <byte[]>] [-Level <int>] [-Threads <int>] [-MaxMemory <long>] [-WhatIf] [-Confirm] [<CommonParameters>]
```

The six lines are the parameter sets, in order: `Path` (the default), `PathDictionary`, `LiteralPath`, `LiteralPathDictionary`, `InputObject` and `InputObjectDictionary`. A set whose name ends in `Dictionary` is its twin with `-Dictionary` mandatory and `-DictionaryPath` absent, so PowerShell's binder refuses the two dictionary parameters together, and refuses `-InputObject` beside `-DestinationPath`, `-Force` or `-PassThru`.

## Description

With `-Path` or `-LiteralPath`, each file named is compressed on its own into one zstd frame. A file that fails gets its own error record and the rest go on. A file is mapped into memory and handed to libzstd a mebibyte more at a time: on the calling thread libzstd reads the mapping in place, and with worker threads it copies each job's input into buffers of its own. When Windows will not map the file, or, within `-MaxMemory`, the page tables of its map do not fit, it is read a mebibyte at a time instead. The output is written to a temporary file beside its destination that is renamed onto the destination only when complete, so a failed or stopped run leaves no partial file and a file already at the destination survives. The destination is the file's name with `.zst` added, beside it, unless `-DestinationPath` names an existing folder to write into or one file, which only the first input of the command may take.

With `-InputObject`, a single `byte[]` is compressed in one piece without being copied, and the records of a longer pipeline are fed to libzstd as they arrive. The result is written as one `byte[]` when the pipeline ends. When no record arrives, nothing is written, output or error. A `$null` record gets a `ZstdNoInput` error record, and the records around it are compressed as if it had not been sent. The command writes the same error when PowerShell's binder chooses the `InputObject` set with no `-InputObject` given, as it does for `-Dictionary` alone.

Every result is one zstd frame (RFC 8878) with a content checksum. The frame's header records the content size for a file, and for a single `byte[]`; a frame made from several records does not, since it begins before its length is known. With a dictionary in zstd's format, the header records the dictionary's id.

With `-MaxMemory`, libzstd's estimates decide before any work starts: the command uses as many of the `-Threads` worker threads as fit, writes a warning when that is fewer than asked for, and is refused with `ZstdBudgetTooSmall` when the calling thread, or one worker, does not fit.

## Parameters

### -Path

| | |
|---|---|
| Type | `System.String[]` |
| Parameter sets | `Path`, `PathDictionary`: mandatory, position 0 |
| Pipeline input | no |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value; `ZstdInputNotFound` for a value that names nothing or whose wildcards match nothing; `ZstdPathNotResolved` for a path the session cannot resolve; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be opened or read |

The files to compress, one or several. Wildcard characters select every file they match, and a relative path is resolved against the current PowerShell location. A value that matches nothing is a `ZstdInputNotFound` record of its own.

### -LiteralPath

| | |
|---|---|
| Type | `System.String[]` |
| Aliases | `PSPath`, `LP` |
| Parameter sets | `LiteralPath`, `LiteralPathDictionary`: mandatory, named |
| Pipeline input | by property name (`PSPath`, `LP`, `LiteralPath`) |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value; `ZstdInputNotFound` for a file that is not there; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be opened or read |

The files to compress, each named exactly as written, wildcard characters included. A file piped from `Get-ChildItem` or `Get-Item` binds here by its `PSPath` property.

### -DestinationPath

| | |
|---|---|
| Type | `System.String` |
| Parameter sets | `Path`, `LiteralPath`, `PathDictionary`, `LiteralPathDictionary`: optional, position 1 |
| Pipeline input | no |
| Default | none: each file is written beside itself as `name.zst` |
| Refused with | `ZstdDestinationExists` for a file that exists, without `-Force`; `ZstdDestinationIsDirectory` where a folder stands in place of the file; `ZstdDestinationReused` for a second input to one file; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdWriteFailed` when the file cannot be written |

Where to write. An existing folder receives each file as `name.zst`. Any other path names one file, which only the first input of the command may take; a later input is refused with `ZstdDestinationReused`.

### -InputObject

| | |
|---|---|
| Type | `System.Byte[]` |
| Parameter sets | `InputObject`, `InputObjectDictionary`: optional, named |
| Pipeline input | by value |
| Validation | an empty `byte[]` is allowed |
| Refused with | `InputObjectNotBound` from the binder for a record it cannot make a `byte[]`; `ZstdNoInput` for a `$null` record, and when the binder chooses this set with no `-InputObject` given; `AmbiguousParameterSet` from the binder beside `-DestinationPath`, `-Force` or `-PassThru` |

The bytes to compress. PowerShell's binder converts each pipeline record to a `byte[]` first: a single byte piped from an enumerated array arrives as a `byte[]` of one, a number from 0 to 255 becomes one, and a record it cannot convert gets the binder's `InputObjectNotBound` error and never reaches the cmdlet. A `byte[]` another command wrote, which reaches this one wrapped in a `PSObject`, binds as the array itself. An empty `byte[]` compresses to a frame of nothing, and a `$null` record gets `ZstdNoInput`.

### -DictionaryPath

| | |
|---|---|
| Type | `System.String` |
| Parameter sets | `Path`, `LiteralPath`, `InputObject`: optional, named |
| Pipeline input | no |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value, and `AmbiguousParameterSet` beside `-Dictionary`; `ZstdInputNotFound` for a file that is not there; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be read; `ZstdDictionaryInvalid` for a dictionary in zstd's format whose tables libzstd refuses; `ZstdOutOfMemory` when its bytes cannot be held |

A dictionary file, named exactly as written: one in zstd's format, whose id each frame's header then records, or any other file, whose bytes are used as raw content. It is loaded, and checked by libzstd when it is in zstd's format, before any input is read. Expanding the output needs the same dictionary.

### -Dictionary

| | |
|---|---|
| Type | `System.Byte[]` |
| Parameter sets | `PathDictionary`, `LiteralPathDictionary`, `InputObjectDictionary`: mandatory, named |
| Pipeline input | no |
| Refused with | `AmbiguousParameterSet` from the binder beside `-DictionaryPath`; `ZstdDictionaryInvalid` for a dictionary in zstd's format whose tables libzstd refuses; `ZstdOutOfMemory` when its bytes cannot be held |

A dictionary as a `byte[]`, taken as `-DictionaryPath` takes a file's bytes.

### -Level

| | |
|---|---|
| Type | `System.Int32` |
| Parameter sets | all |
| Pipeline input | no |
| Validation | 1 to 22 |
| Default | 3, libzstd's own default |
| Refused with | `ParameterArgumentValidationError` from the binder outside 1 to 22 |

The compression level. Higher levels trade speed and memory for a smaller output; [How To Choose A Level And Threads](How-To-Choose-A-Level-And-Threads.md) shows sizes.

### -Threads

| | |
|---|---|
| Type | `System.Int32` |
| Parameter sets | all |
| Pipeline input | no |
| Validation | 0 to 256 |
| Default | 0: compress on the calling thread |
| Refused with | `ParameterArgumentValidationError` from the binder outside 0 to 256 |

Worker threads libzstd compresses with. The number of workers from 1 up does not change the output; compressing on the calling thread can. An input of 512 KiB or less is compressed on the calling thread whatever the number asked for, as libzstd does.

### -MaxMemory

| | |
|---|---|
| Type | `System.Int64` |
| Parameter sets | all |
| Pipeline input | no |
| Validation | 1048576 to 9223372036854775807 |
| Default | none: no limit, and the worker threads asked for |
| Refused with | `ParameterArgumentValidationError` from the binder below 1048576; `ZstdBudgetTooSmall` when the calling thread, or one worker, does not fit, or a result on bytes outgrows what is left |

The most memory, in bytes, the command may commit. It counts what libzstd takes, estimated from libzstd's own sizes before any work starts; a dictionary, the command's copy and libzstd's; the command's own buffers and what the host takes while it runs; the page tables of a file's map, 4 KB for each 2 MiB of the file, so that a file whose map does not fit is read a mebibyte at a time instead; and, on bytes, the result, which is held in memory twice, as it is built and as the `byte[]` written: a single `byte[]` counts its largest possible frame up front, and a longer pipeline's frame stops the command with `ZstdBudgetTooSmall` when it outgrows what is left. The verbose stream reports what the command was fitted to. [Streaming And Memory](Streaming-And-Memory.md) explains the estimates and compares them with what was measured.

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

`-WhatIf` names each file and its destination and writes nothing; `-Confirm` asks before each file. Both act on files only: on bytes neither cmdlet asks, and the frame is written either way.

## Inputs

- `System.Byte[]`, by value, to `-InputObject`.
- Any object with a `PSPath` property, such as the `System.IO.FileInfo` that `Get-ChildItem` writes, by property name to `-LiteralPath`.

## Outputs

- `System.IO.FileInfo`, one for each file written, with `-PassThru` on files.
- `System.Byte[]`, one for the whole pipeline, on bytes.

The `FileInfo` is the one `Get-Item` gives, with the provider's properties such as `PSPath`, so it pipes on to other commands:

```powershell
$random = [System.Random]::new(3)
$words = -split 'GET POST PUT DELETE /api/users /api/orders /api/items /login /health 200 201 204 304 400 404 500'
New-Item -ItemType Directory -Path .\logs, .\archive | Out-Null
foreach ($name in 'app', 'logs\api', 'logs\web') {
    1..5000 | ForEach-Object {
        '12:{0:d2}:{1:d2} {2} {3} {4} {5}ms' -f ($_ % 60), ($_ * 7 % 60), $words[$random.Next(0, 4)], $words[$random.Next(4, 9)], $words[$random.Next(9, 16)], $random.Next(1, 900)
    } | Set-Content -Path ".\$name.log"
}
```

```powershell
$written = Compress-Zstd -Path .\logs\api.log -DestinationPath .\members.zst -PassThru
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
PSChildName         NoteProperty   string PSChildName=members.zst
PSDrive             NoteProperty   PSDriveInfo PSDrive=C
PSIsContainer       NoteProperty   bool PSIsContainer=False
PSParentPath        NoteProperty   string PSParentPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs
PSPath              NoteProperty   string PSPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs\members.zst
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
PSChildName       NoteProperty   string PSChildName=members.zst
PSDrive           NoteProperty   PSDriveInfo PSDrive=C
PSIsContainer     NoteProperty   bool PSIsContainer=False
PSParentPath      NoteProperty   string PSParentPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs
PSPath            NoteProperty   string PSPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs\members.zst
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

The properties most scripts read:

```powershell
$written | Format-List Name, Length, FullName, PSPath
```

```text
Name     : members.zst
Length   : 36801
FullName : C:\Temp\zstd-docs\members.zst
PSPath   : Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs\members.zst
```

On bytes the output is a plain `byte[]`, the frame:

```powershell
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('hello'))
$packed.GetType().FullName
Get-Member -InputObject $packed -MemberType Properties | Format-Table -Wrap
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

The examples from the cmdlet's help, on the logs made above.

Compress one file beside itself. Without `-PassThru` nothing is written to the pipeline:

```powershell
Compress-Zstd -Path .\app.log
Get-Item .\app.log.zst | Select-Object Name, Length
```

```text
Name        Length
----        ------
app.log.zst  36729
```

Compress every file a wildcard matches into a folder, and list what was written:

```powershell
Compress-Zstd -Path .\logs\*.log -DestinationPath .\archive -PassThru | Select-Object Name, Length
```

```text
Name        Length
----        ------
api.log.zst  36801
web.log.zst  36794
```

Compress files piped from `Get-ChildItem`, each beside itself, at level 19:

```powershell
Get-ChildItem .\logs -Filter *.log | Compress-Zstd -Level 19
Get-ChildItem .\logs -Filter *.zst | Select-Object Name, Length
```

```text
Name        Length
----        ------
api.log.zst  25096
web.log.zst  25044
```

Replace a file at level 19 with four worker threads:

```powershell
Compress-Zstd -Path .\app.log -DestinationPath .\app.log.zst -Level 19 -Threads 4 -Force
Get-Item .\app.log.zst | Select-Object Name, Length
```

```text
Name        Length
----        ------
app.log.zst  25087
```

Compress bytes:

```powershell
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes('hello'))
$packed.Length
```

```text
18
```

Compress within a memory budget. A 16 MiB file at level 3, with eight worker threads asked for, gets as many as fit 96 MB, and a warning says how many:

```powershell
$noise = [byte[]]::new(16MB)
[System.Random]::new(5).NextBytes($noise)
[System.IO.File]::WriteAllBytes("$PWD\noise.bin", $noise)
Compress-Zstd -Path .\noise.bin -Threads 8 -MaxMemory 96MB -Verbose
```

```text
VERBOSE: Performing the operation "Compress File" on target "Item: C:\Temp\zstd-docs\noise.bin Destination: C:\Temp\zstd-docs\noise.bin.zst".
WARNING: Compressing 'C:\Temp\zstd-docs\noise.bin' with 5 of the 8 worker threads asked for, the most that fit -MaxMemory of 100663296 bytes: estimated at 96032816 bytes.
VERBOSE: Within -MaxMemory of 100663296 bytes: level 3 with 5 worker threads, estimated at 96032816 bytes.
VERBOSE: Compressed 'C:\Temp\zstd-docs\noise.bin' (16777216 bytes) to 'C:\Temp\zstd-docs\noise.bin.zst' (16777614 bytes) at level 3 with 5 worker threads.
```

Random bytes do not compress: the frame is a little larger than the file.

A budget too small for even the calling thread is refused before anything is written:

```powershell
try {
    Compress-Zstd -Path .\noise.bin -DestinationPath .\small.zst -Level 19 -MaxMemory 8MB -ErrorAction Stop
} catch {
    $_.FullyQualifiedErrorId
    $_.Exception.Message
}
Test-Path .\small.zst
```

```text
ZstdBudgetTooSmall,Pwrs.Modules.Zstd.CompressZstdCommand
Cannot compress 'C:\Temp\zstd-docs\noise.bin' within -MaxMemory of 8388608 bytes: at level 19 on the calling thread it needs about 89559703 bytes.
False
```

Values the parameter binder refuses never reach the command, and its records carry the binder's own ids:

```powershell
$refused = @(
    { Compress-Zstd -Path '' }
    { Compress-Zstd -Path $null }
    { Compress-Zstd -Path .\app.log -Level 23 }
    { Compress-Zstd -Path .\app.log -MaxMemory 1000 }
    { Compress-Zstd -InputObject ([byte[]] (1, 2, 3)) -Force }
    { Compress-Zstd -Path .\app.log -DictionaryPath .\a.dict -Dictionary ([byte[]] (1, 2, 3)) }
)
foreach ($command in $refused) {
    try { & $command } catch { $_.FullyQualifiedErrorId }
}
```

```text
ParameterArgumentValidationError,Pwrs.Modules.Zstd.CompressZstdCommand
ParameterArgumentValidationError,Pwrs.Modules.Zstd.CompressZstdCommand
ParameterArgumentValidationError,Pwrs.Modules.Zstd.CompressZstdCommand
ParameterArgumentValidationError,Pwrs.Modules.Zstd.CompressZstdCommand
AmbiguousParameterSet,Pwrs.Modules.Zstd.CompressZstdCommand
AmbiguousParameterSet,Pwrs.Modules.Zstd.CompressZstdCommand
```

## Errors

`ZstdInputNotFound`, `ZstdInputIsDirectory`, `ZstdReadFailed`, `ZstdPathNotResolved`, `ZstdDestinationExists`, `ZstdDestinationIsDirectory`, `ZstdDestinationReused`, `ZstdWriteFailed`, `ZstdDictionaryInvalid`, `ZstdCompressFailed`, `ZstdBudgetTooSmall`, `ZstdNoInput`, `ZstdInputNotBytes`, `ZstdOutOfMemory` and `ZstdThreadsInvalid`, each described in the [Error Reference](Error-Reference.md). [How To Handle Errors](How-To-Handle-Errors.md) shows how to collect and act on them.
