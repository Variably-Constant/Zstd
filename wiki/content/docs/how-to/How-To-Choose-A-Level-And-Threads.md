---
title: How To Choose A Level And Threads
weight: 6
---

What `-Level` and `-Threads` do to the output of `Compress-Zstd`, and how `-MaxMemory` fits the threads. Source: `src/lib.rs`, `Settings`, `compress`, `compress_stable`, `fit` and `report_fit`.

`-Level` takes 1 to 22 and defaults to 3, libzstd's own default. Higher levels search harder for repeated data, which takes longer and more memory. Its memory, and the speed of its file paths at levels 3 and 19, were measured on a 5 GiB file, in [Streaming And Memory](Streaming-And-Memory.md).

`-Threads` takes 0 to 256. The default, 0, compresses on the calling thread. From 1 up, libzstd compresses on that many worker threads of its own.

The examples ran in order, in PowerShell 7.6.6 and Windows PowerShell 5.1, and printed the same in both.

## Sizes by level

A log of 20000 lines:

```powershell
$random = [System.Random]::new(42)
$words = -split 'GET POST PUT DELETE /api/users /api/orders /api/items /login /health 200 201 204 304 400 404 500'
1..20000 | ForEach-Object {
    '12:{0:d2}:{1:d2} {2} {3} {4} {5}ms' -f ($_ % 60), ($_ * 7 % 60), $words[$random.Next(0, 4)], $words[$random.Next(4, 9)], $words[$random.Next(9, 16)], $random.Next(1, 900)
} | Set-Content -Path .\app.log
[byte[]] $log = [System.IO.File]::ReadAllBytes("$PWD\app.log")
$log.Length
```

```text
693509
```

```powershell
foreach ($level in 1, 3, 6, 9, 12, 15, 19, 22) {
    [pscustomobject]@{ Level = $level; Bytes = (Compress-Zstd -InputObject $log -Level $level).Length }
}
```

```text
Level  Bytes
-----  -----
    1 147471
    3 140990
    6 130753
    9 122105
   12 115608
   15 101429
   19  92214
   22  92214
```

For this file, level 19 finds everything level 22 does. What each level buys depends on the data, so try yours: the sizes are exact and cost nothing to check.

## Worker threads

On this file, the calling thread and 1, 2 and 8 workers write the same bytes. The first 16 hex digits of each frame's SHA-256 show it:

```powershell
function Get-Sha256([byte[]] $Bytes) {
    (Get-FileHash -InputStream ([System.IO.MemoryStream]::new($Bytes)) -Algorithm SHA256).Hash.Substring(0, 16)
}
foreach ($threads in 0, 1, 2, 8) {
    $frame = Compress-Zstd -InputObject $log -Level 19 -Threads $threads
    [pscustomobject]@{ Threads = $threads; Bytes = $frame.Length; Sha256 = Get-Sha256 $frame }
}
```

```text
Threads Bytes Sha256
------- ----- ------
      0 92214 D886A68EF95CEDC3
      1 92214 D886A68EF95CEDC3
      2 92214 D886A68EF95CEDC3
      8 92214 D886A68EF95CEDC3
```

With workers, libzstd cuts larger input into jobs and compresses them side by side, and each job reaches back only part of the way into the one before it. The same log written twenty times over is 14 MB, several jobs at level 3:

```powershell
$buffer = [System.IO.MemoryStream]::new()
foreach ($copy in 1..20) { $buffer.Write($log, 0, $log.Length) }
[byte[]] $big = $buffer.ToArray()
$big.Length
foreach ($threads in 0, 1, 2, 8) {
    $frame = Compress-Zstd -InputObject $big -Threads $threads
    [pscustomobject]@{ Threads = $threads; Bytes = $frame.Length; Sha256 = Get-Sha256 $frame }
}
```

```text
13870180

Threads  Bytes Sha256
-------  ----- ------
      0 142266 7692D2043EFE1DB7
      1 229005 BDE792736B7CEB73
      2 229005 BDE792736B7CEB73
      8 229005 BDE792736B7CEB73
```

Two things show here. The number of workers did not change the output: 1, 2 and 8 wrote identical frames. A run on two 128 MiB inputs found the same for 1, 2, 8 and 24 workers at levels 3 and 19, and on their first 32 MiB at level 22. The calling thread and the workers can write different frames, though, and on data that repeats every 693509 bytes the calling thread, which keeps each repeat in reach, found far more. Any of these frames expands to the same bytes.

Workers also hold more memory. `-Threads 8` at level 19 committed about 1.4 GB above what the host held on its own, on a 1 GiB file, where level 3 on the calling thread committed 3 to 4 MB ([Streaming And Memory](Streaming-And-Memory.md)).

## Worker threads within a memory budget

With `-MaxMemory`, the command takes as many of the worker threads asked for as fit the budget, from libzstd's own estimates, before any work starts. libzstd allocates its workers' input buffers whole at the start, sized by the level's window and the number of workers rather than by the input, so workers at a high level take a lot of memory even for a small input. The verbose stream reports what the command was fitted to:

```powershell
Compress-Zstd -InputObject $big -Level 19 -Threads 8 -MaxMemory 1TB -Verbose 4>&1 |
    Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object Message
```

```text
Within -MaxMemory of 1099511627776 bytes: level 19 with 8 worker threads, estimated at 500564118 bytes, and its result of up to 13924360 bytes, held in memory twice.
Compressed 13870180 bytes to 93325 bytes at level 19 with 8 worker threads.
```

Eight workers at level 19 are estimated at about 500 MB for an input of 14 MB, most of it libzstd's input buffers.

When fewer workers fit than were asked for, a warning names how many, the estimate and the budget. `-WarningVariable` collects it even under `-WarningAction SilentlyContinue`:

```powershell
$frame = Compress-Zstd -InputObject $big -Level 19 -Threads 8 -MaxMemory 400MB -WarningVariable warned -WarningAction SilentlyContinue
$warned.Message
$frame.Length
```

```text
Compressing the input with 4 of the 8 worker threads asked for, the most that fit -MaxMemory of 419430400 bytes: estimated at 362152086 bytes, and its result of up to 13924360 bytes, held in memory twice.
93325
```

The frame from four workers is 93325 bytes, as from eight.

A budget that even one worker, or the calling thread, does not fit is refused with `ZstdBudgetTooSmall`; the [Compress-Zstd reference](Compress-Zstd.md) shows one.

## Out of range

A count beyond 256 is refused by the parameter's range before the cmdlet runs:

```powershell
try {
    Compress-Zstd -InputObject $log -Threads 257
} catch {
    $_.FullyQualifiedErrorId
}
```

```text
ParameterArgumentValidationError,Pwrs.Modules.Zstd.CompressZstdCommand
```
