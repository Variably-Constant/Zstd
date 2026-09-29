---
title: New-ZstdDictionary
weight: 3
---

Trains a Zstandard dictionary from sample files. Source: `src/lib.rs`, `NewZstdDictionary`, `Samples` and `train`.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, in an empty folder. The output shown is PowerShell 7.6's; Windows PowerShell 5.1 printed the same apart from column widths and line wrapping, except where its output is shown too.

## Syntax

```powershell
Get-Command New-ZstdDictionary -Syntax
```

```text
New-ZstdDictionary [-Path] <string[]> [-DestinationPath] <string> [-MaxSize <int>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]

New-ZstdDictionary [-DestinationPath] <string> -LiteralPath <string[]> [-MaxSize <int>] [-Force] [-PassThru] [-WhatIf] [-Confirm] [<CommonParameters>]
```

The two lines are the parameter sets: `Path` (the default) and `LiteralPath`.

## Description

Each file `-Path` or `-LiteralPath` names is one sample, and files piped from `Get-ChildItem` or `Get-Item` bind to `-LiteralPath` by their `PSPath`. The samples are read into memory together, since libzstd's trainer takes them as one buffer, and the dictionary it trains, at most `-MaxSize` bytes, is written to `-DestinationPath` through a temporary file beside it that is renamed onto it only when complete.

`-DestinationPath` is checked, and `-WhatIf` or `-Confirm` answered, before any sample is read: a folder is refused, and so is a file that exists, unless `-Force` is given. A sample that cannot be read gets its own error record, and training goes on with the rest. libzstd's trainer needs at least 7 samples and 8 bytes of them in all, and refuses 4294967295 bytes or more in all; a refusal is `ZstdDictionaryTrainingFailed`, with libzstd's reason, and nothing is written. Training is one call into libzstd, so a stop during it takes effect when it returns, and nothing is written then either.

The dictionary is in zstd's format: it begins with zstd's dictionary magic number and an id, which `-Verbose` reports and which every frame compressed with it records. [Dictionaries](Dictionaries.md) explains the format.

## Parameters

### -Path

| | |
|---|---|
| Type | `System.String[]` |
| Parameter sets | `Path`: mandatory, position 0 |
| Pipeline input | no |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value; `ZstdInputNotFound` for a value that names nothing or whose wildcards match nothing; `ZstdPathNotResolved` for a path the session cannot resolve; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be opened or read; `ZstdOutOfMemory` for samples that cannot be held |

The sample files, each one sample. Wildcard characters select every file they match, and a relative path is resolved against the current PowerShell location. Each value that fails gets its own error record, and training goes on with the rest.

### -LiteralPath

| | |
|---|---|
| Type | `System.String[]` |
| Aliases | `PSPath`, `LP` |
| Parameter sets | `LiteralPath`: mandatory, named |
| Pipeline input | by property name (`PSPath`, `LP`, `LiteralPath`) |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value; `ZstdInputNotFound` for a file that is not there; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdInputIsDirectory` for a folder; `ZstdReadFailed` for a file that cannot be opened or read; `ZstdOutOfMemory` for samples that cannot be held |

The sample files, each one sample, named exactly as written, wildcard characters included. A file piped from `Get-ChildItem` or `Get-Item` binds here by its `PSPath` property.

### -DestinationPath

| | |
|---|---|
| Type | `System.String` |
| Parameter sets | all: mandatory, position 1 |
| Pipeline input | no |
| Validation | not null or empty |
| Refused with | `ParameterArgumentValidationError` from the binder for a null or empty value; `ZstdPathNotResolved` for a path the session cannot resolve to one path; `ZstdDestinationIsDirectory` for a folder; `ZstdDestinationExists` for a file that exists, without `-Force`; `ZstdWriteFailed` when the file cannot be written |

The file to write the dictionary to. A folder is refused with `ZstdDestinationIsDirectory`, and a file that exists with `ZstdDestinationExists` unless `-Force` is given.

### -MaxSize

| | |
|---|---|
| Type | `System.Int32` |
| Parameter sets | all |
| Pipeline input | no |
| Validation | 256 to 2147483647 |
| Default | 112640, the zstd command line tool's default |
| Refused with | `ParameterArgumentValidationError` from the binder outside 256 to 2147483647; `ZstdDictionaryTrainingFailed` when libzstd's trainer refuses the size for the samples given |

The largest dictionary to train, in bytes.

### -Force

| | |
|---|---|
| Type | `System.Management.Automation.SwitchParameter` |
| Parameter sets | all |

Replaces a destination file that exists.

### -PassThru

| | |
|---|---|
| Type | `System.Management.Automation.SwitchParameter` |
| Parameter sets | all |

Writes the dictionary's `FileInfo`, as `Get-Item` gives it. Without it the cmdlet writes nothing to the pipeline.

### -WhatIf, -Confirm

`-WhatIf` names the destination and does nothing more: no sample is read and nothing is written. `-Confirm` asks once, before any sample is read.

## Inputs

Any object with a `PSPath` property, such as the `System.IO.FileInfo` that `Get-ChildItem` writes, by property name to `-LiteralPath`.

## Outputs

`System.IO.FileInfo`, one, with `-PassThru`.

Samples to train from: 300 small JSON records, and ten logs of 500 lines:

```powershell
New-Item -ItemType Directory -Path .\samples, .\logs | Out-Null
foreach ($i in 1..300) {
    $role = ('reader', 'writer', 'admin')[$i % 3]
    $active = ($i % 2 -eq 0).ToString().ToLower()
    '{{"id":{0},"name":"user{0}","email":"user{0}@example.com","role":"{1}","active":{2},"quota":{3}}}' -f $i, $role, $active, ($i * 37 % 1000) |
        Set-Content -Path ".\samples\user$i.json"
}
$random = [System.Random]::new(5)
$words = -split 'GET POST PUT DELETE /api/users /api/orders /api/items /login /health 200 201 204 304 400 404 500'
foreach ($part in 1..10) {
    1..500 | ForEach-Object {
        '12:{0:d2}:{1:d2} {2} {3} {4} {5}ms' -f ($_ % 60), ($_ * 7 % 60), $words[$random.Next(0, 4)], $words[$random.Next(4, 9)], $words[$random.Next(9, 16)], $random.Next(1, 900)
    } | Set-Content -Path ".\logs\part$part.log"
}
```

The `FileInfo` is the one `Get-Item` gives, with the provider's properties such as `PSPath`:

```powershell
$written = New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\members.dict -PassThru
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
PSChildName         NoteProperty   string PSChildName=members.dict
PSDrive             NoteProperty   PSDriveInfo PSDrive=C
PSIsContainer       NoteProperty   bool PSIsContainer=False
PSParentPath        NoteProperty   string PSParentPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs
PSPath              NoteProperty   string PSPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs\members.dict
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
PSChildName       NoteProperty   string PSChildName=members.dict
PSDrive           NoteProperty   PSDriveInfo PSDrive=C
PSIsContainer     NoteProperty   bool PSIsContainer=False
PSParentPath      NoteProperty   string PSParentPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs
PSPath            NoteProperty   string PSPath=Microsoft.PowerShell.Core\FileSystem::C:\Temp\zstd-docs\members.dict
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

## Examples

The examples from the cmdlet's help, on the samples made above.

Train a dictionary from every sample a wildcard matches. Without `-PassThru` nothing is written to the pipeline:

```powershell
New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\json.dict
Get-Item .\json.dict | Select-Object Name, Length
```

```text
Name      Length
----      ------
json.dict  21971
```

Train from samples piped from `Get-ChildItem`, replacing that dictionary, and see what was trained:

```powershell
Get-ChildItem .\samples -Filter *.json | New-ZstdDictionary -DestinationPath .\json.dict -Force -Verbose
```

```text
VERBOSE: Performing the operation "New Dictionary" on target "Destination: C:\Temp\zstd-docs\json.dict".
VERBOSE: Training a dictionary of at most 112640 bytes from 300 samples of 30002 bytes in all.
VERBOSE: Wrote dictionary 1027205458, 21971 bytes, to 'C:\Temp\zstd-docs\json.dict'.
```

Train a dictionary of at most 64 KiB from logs:

```powershell
New-ZstdDictionary -Path .\logs\*.log -DestinationPath .\logs.dict -MaxSize 65536 -Force -PassThru | Select-Object Name, Length
```

```text
Name      Length
----      ------
logs.dict  65536
```

See what would be written:

```powershell
New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\whatif.dict -WhatIf
Test-Path .\whatif.dict
```

```text
What if: Performing the operation "New Dictionary" on target "Destination: C:\Temp\zstd-docs\whatif.dict".
False
```

A folder as the destination is refused before any sample is read:

```powershell
New-ZstdDictionary -Path .\samples\*.json -DestinationPath .\samples -ErrorVariable failed -ErrorAction SilentlyContinue
$failed.FullyQualifiedErrorId
$failed.Exception.Message
```

```text
ZstdDestinationIsDirectory,Pwrs.Modules.Zstd.NewZstdDictionaryCommand
'C:\Temp\zstd-docs\samples' is a directory, and -DestinationPath takes a file.
```

## Errors

`ZstdInputNotFound`, `ZstdInputIsDirectory`, `ZstdReadFailed`, `ZstdPathNotResolved`, `ZstdDestinationExists`, `ZstdDestinationIsDirectory`, `ZstdWriteFailed`, `ZstdDictionaryTrainingFailed` and `ZstdOutOfMemory`, each described in the [Error Reference](Error-Reference.md). [How To Use A Dictionary](How-To-Use-A-Dictionary.md) shows training that fails.
