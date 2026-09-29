---
title: Streaming And Memory
weight: 2
---

How files, bytes and samples move through the cmdlets, what that costs in memory, and how `-MaxMemory` bounds it. Source: `src/lib.rs`, `compress_stable`, `expand_mapped`, `expand_whole`, `expand_direct`, `direct_fits`, `go_on_from`, `compress`, `expand`, `map_input`, `write_through_temporary`, `write_mapped_through_temporary`, `CompressZstd::compress_file`, `ExpandZstd::expand_file`, `fitting`, `calling_thread_estimate`, `workers_estimate`, `page_tables`, `in_place_needs`, `Compression`, `Expansion` and `Samples`; `tests/LargeFile.Gate.ps1` for the measurements.

## Files are mapped into memory

`Compress-Zstd` maps a file into memory and hands the mapping to libzstd as its input, a mebibyte more at a time. On the calling thread libzstd reads the file where it lies and keeps no input buffer of its own; with worker threads it copies each job's input into buffers of its own, as it does for any input. The compressed output is written a mebibyte at a time.

`Expand-Zstd` reads a file a mebibyte at a time, and libzstd keeps a window of recent output as large as each frame's header asks for. A frame whose window does not fit that way, over 128 MiB or beyond `-MaxMemory`, sends the file to be written in place instead, when every frame in it records its content size: the length of the output is then known before any frame is expanded, the temporary file is made that long and mapped, and libzstd writes each frame straight into it through a map of the input. Everything libzstd has written is then in reach in the map, so it keeps no window of its own, and a frame of any window it expands takes no memory for it.

When Windows will not map a file, `Compress-Zstd` reads it a mebibyte at a time instead, and says so on the verbose stream; a file that needed to be written in place and cannot be is refused, as the next sections say.

Before each mebibyte of input, each cmdlet checks whether the pipeline is stopping, so Ctrl+C, or a `PowerShell.Stop()` from a script, is noticed at the next mebibyte.

The pages of a mapped file count toward the process's working set while they are in use, but not toward its commit, the memory Windows must back with RAM or the page file: the file itself backs them, and Windows can drop them, or write them back to the file, when it needs the memory. What a map does add to the commit is its page tables, which Windows charges to the process as the map is touched: 4 KB for each 2 MiB of the file, 1/512 of it. So compressing a large file, or writing one in place, grows the working set with the file, and the commit by the page tables, beside what the level, the workers and the window take.

## Output goes through a temporary file

Each output file is written first to a temporary file beside its destination, in the same folder so that the last step is a rename on one volume. Only when the output is complete, and for `Expand-Zstd` only when every frame has expanded and passed its checksum, is the temporary file renamed onto the destination; one written in place is flushed to disk first, and one written a mebibyte at a time is left to Windows to write. A failure or a stop removes the temporary file instead. So a destination either holds a complete result or is left as it was: with `-Force`, the file already there survives a failed or stopped run unchanged. The Pester suite stops both cmdlets partway through a file and checks exactly that.

## What it costs, measured

`tests/LargeFile.Gate.ps1` ran a 5 GiB file of real data, the files under `C:\Windows\System32` concatenated, and its first 1 GiB through both cmdlets in both hosts, each command in a process of its own. It ran on Windows 11 Pro 10.0.26200 on an AMD Ryzen 9 7900X, beside other work, and every expansion matched its source byte for byte. Each process's peak commit above what the host held after importing the module, with its peak working set in parentheses, in MB (10^6 bytes):

| Command | Windows PowerShell 5.1, 1 GiB | 5.1, 5 GiB | PowerShell 7.6, 1 GiB | 7.6, 5 GiB |
|---|---|---|---|---|
| `Compress-Zstd`, level 3, on the calling thread | 3.0 (1164) | 11.3 (5459) | 3.9 (1173) | 12.6 (5460) |
| `Expand-Zstd` of that output | 2.9 (93) | 2.9 (92) | 4.2 (102) | 3.9 (97) |
| `Compress-Zstd -Level 19 -Threads 8` | 1392.1 (2369) | 1400.7 (6783) | 1393.0 (2374) | 1401.5 (6758) |
| `Expand-Zstd` of that output | 9.3 (99) | 9.4 (99) | 10.2 (104) | 10.3 (107) |

At level 3 on the calling thread, compressing committed 3 to 4 MB above the host on 1 GiB and 11 to 13 MB on 5 GiB: what grows with the file is the map's page tables, 2 MB for each GiB mapped. Compressing at level 19 with 8 worker threads committed about 1.4 GB on either size: libzstd's worker threads copy their input into buffers of their own, sized by the level's window and the number of workers. Expanding, a mebibyte at a time, committed 3 to 4 MB for the level 3 output and 9 to 10 MB for the level 19 output, whose window is 8 MiB, on either size. The working set grew with the file only when compressing, to about 5.5 GB at level 3 and 6.8 GB at level 19 for 5 GiB, since the pages of a map count in it while they are in use; expanding kept it at 107 MB or less.

The level 3 output was 415617774 bytes from 1 GiB and 1989226526 from 5 GiB; the level 19 output was 343565437 and 1613033180.

### Mapped against a mebibyte at a time, timed

`Compress-Zstd` and `Expand-Zstd` ran file to file on a 5 GiB file built as above, in this module, which compresses from a map of the file and expands it a mebibyte at a time, and in a module that read and wrote every file a mebibyte at a time: at level 3 on the calling thread and at level 19 with 8 worker threads, in both hosts, 7 rounds, one process for each module in each round, in an order rotated by round. Both modules compiled libzstd with MSVC's cl.exe. It ran on Windows 11 Pro 10.0.26200 on an AMD Ryzen 9 7900X, whatever else the machine was doing, with the load outside the run sampled every 5 seconds: the median of the rounds' medians was 4.7 cores busy in Windows PowerShell 5.1 and 6.2 to 7.5 in PowerShell 7.6, with single readings up to 23. Medians in MB (10^6 bytes) of the file per second, and the median of each round's ratio, this module over the other, with the least and the most of the 7:

| Command | 5.1, a mebibyte at a time | 5.1, this module | Ratio | 7.6, a mebibyte at a time | 7.6, this module | Ratio |
|---|---|---|---|---|---|---|
| `Compress-Zstd`, level 3, on the calling thread | 315.3 | 346.9 | 1.12 [0.91-1.65] | 320.8 | 354.9 | 1.08 [0.82-1.18] |
| `Expand-Zstd` of that output | 678.0 | 897.9 | 1.00 [0.52-2.50] | 903.8 | 902.0 | 0.98 [0.78-2.37] |
| `Compress-Zstd -Level 19 -Threads 8` | 25.7 | 24.3 | 0.97 [0.80-1.31] | 25.3 | 23.9 | 0.97 [0.86-1.08] |
| `Expand-Zstd` of that output | 571.2 | 501.5 | 1.07 [0.36-1.40] | 700.3 | 681.9 | 1.07 [0.79-1.83] |

Compressing from the map read 8 to 12% faster by the median on the calling thread, with rounds going either way at this load, and the same with worker threads, which copy their input either way. Expanding read the same in both modules within the spread of the rounds. Measured the same way, a module that wrote every file in place expanded it 29 to 43% slower than a mebibyte at a time, 0.8 to 1.6 s of the difference being the flush to disk before the rename, so a file is written in place only when a frame's window needs it.

## The memory budget

`-MaxMemory` is the most memory, in bytes, a command may commit, from 1 MB up. It counts:

- libzstd's share, from libzstd's own estimates: to compress, the context on the calling thread, or each worker's context with the buffers libzstd shares among them, at the level and for the size of the input; to expand a mebibyte at a time, what a frame's window takes, from the frame's header; to expand in place, into a file or a `byte[]`, its context and a block of input, and no window.
- a dictionary: the command's copy of its bytes, and libzstd's own digest of it.
- the page tables of a mapped file: 4 KB for each 2 MiB of it, and one more, since a map need not start on a 2 MiB boundary. A file whose map does not fit is not mapped: `Compress-Zstd` reads it a mebibyte at a time, as when Windows will not map it, and says so on the verbose stream, and `Expand-Zstd` does not write it in place.
- the command's own share: its buffers, at most a mebibyte of input and one of output, and 2 MiB for what the host commits while the command runs; and with worker threads, 1 MiB for each, for its stack and its job's own state beyond libzstd's sizes.
- on bytes, the result: counted once when the bytes are one record whose frames all record their content size, since libzstd expands them straight into the `byte[]` written; otherwise counted twice, since it is held in memory as it is built and again as the `byte[]` written.

`Compress-Zstd` fits the work to the budget before any work starts. It takes as many of the `-Threads` worker threads as fit, writes a warning when that is fewer than asked for, and is refused with `ZstdBudgetTooSmall` when the calling thread, or one worker, does not fit. On bytes, a single `byte[]` counts its frame at its largest, libzstd's bound for its length, and a longer pipeline's frame, whose length is not known ahead, stops the command with `ZstdBudgetTooSmall` when it outgrows what the rest leaves. A file is mapped when the calling thread, or at least one worker, fits the budget with the map's page tables counted, and is otherwise read a mebibyte at a time and fitted again without them; when neither fits, `ZstdBudgetTooSmall` names the smaller need of the two.

`Expand-Zstd` checks each frame it expands a mebibyte at a time before libzstd sees it. A file with a frame whose window does not fit is written in place, when every frame in it records its content size and what that takes fits: the page tables of both files' maps, libzstd's context and a block of input, the dictionary and the command's own share. It then takes no window, so its frames are not held to the budget. Otherwise the frame is refused with `ZstdWindowTooLarge`. On bytes, a first record whose frames all record their content size is checked once, before libzstd sees it: its result, counted once, with libzstd's context, a block of input, the dictionary and the command's own share, is refused with `ZstdBudgetTooSmall` when it does not fit, and a frame of any window in it is expanded, since libzstd keeps no window for it. Bytes fed to libzstd as they arrive are checked frame by frame: a frame whose window does not fit is refused the same way as in a file, a frame whose header records a result that will not fit is refused with `ZstdBudgetTooSmall`, and a result that outgrows the budget stops the command with it. A dictionary is counted whichever way a file or bytes are expanded.

### How close the estimates come

Each case below compressed the start of a 5 GiB file built as the one above, in a fresh process of each host, with `-MaxMemory` at its largest, so that the verbose stream reported the estimate and nothing was held back. The process recorded its commit before the command and the peak commit Windows keeps for it, and ran nothing else while the command did. The module compiled libzstd with clang-cl. It ran on Windows 11 Pro 10.0.26200 on an AMD Ryzen 9 7900X, beside other work that kept the processors 41 to 97% busy as each case began. In MB (10^6 bytes):

| Level | Worker threads | Input | Estimate | Peak above the host, 5.1 | Peak above the host, 7.6 |
|---|---|---|---|---|---|
| 3 | 0 | 64 MiB | 5.8 | 2.7 | 1.1 |
| 3 | 1 | 64 MiB | 73.9 | 44.7 | 51.7 |
| 3 | 8 | 64 MiB | 182.8 | 161.2 | 140.8 |
| 3 | 24 | 192 MiB | 489.6 | 259.0 | 461.3 |
| 19 | 0 | 64 MiB | 89.7 | 86.7 | 86.0 |
| 19 | 1 | 64 MiB | 292.2 | 254.9 | 256.4 |
| 19 | 8 | 256 MiB | 1333.3 | 1325.1 | 1323.6 |
| 19 | 24 | 768 MiB | 3790.1 | 3770.6 | 3769.4 |
| 22 | 0 | 128 MiB | 744.2 | 742.5 | 740.9 |
| 22 | 1 | 512 MiB | 3700.8 | 3636.6 | 3635.0 |
| 22 | 8 | 4 GiB | 17832.8 | 17317.5 | 17314.9 |
| 22 | 24 | 5 GiB | 29401.4 | 28757.9 | 28756.1 |

Every peak stayed within its estimate: at levels 19 and 22 at 0.87 to 0.998 of it, and at level 3 at 0.19 to 0.94. Each case ran once in each host, and at level 3 the two hosts' peaks for one case differ by as much as 202.4 MB. The estimates count the most libzstd can hold at once, such as an output buffer for every job its table of jobs holds, and the command's own share of 4 MiB, which weighs most in the smallest.

## Windows when expanding

A frame expanded a mebibyte at a time needs a window as large as its header asks for. Without `-MaxMemory`, libzstd's own limit applies: a window of up to 128 MiB, which covers every level this module offers, the largest window of which, at level 22, is 128 MiB. A file with a larger window is written in place when every frame in it records its content size, and needs no window then; otherwise the frame is refused with `ZstdWindowTooLarge` before libzstd sees it, and the message names the `-MaxMemory` that lets it. Bytes whose frames record their content size are expanded straight into their `byte[]` and need no window either. With `-MaxMemory`, the budget decides, up to 2 GiB, the largest window libzstd expands at all.

## Bytes are held in memory

On bytes, the input arrives as `byte[]` objects the pipeline already holds, and the output is built in memory and written as one `byte[]` when the pipeline ends. A single record is compressed in place, without a copy. Records from a longer pipeline are fed to libzstd as they arrive, so the input is never gathered into one array, but the output, compressed or expanded, is held whole until the end, and is held twice at the end, when it is written as a `byte[]`: `-MaxMemory` counts it that way.

`Expand-Zstd` takes a first record whose frames all record their content size whole: the host makes a `byte[]` as long as the sizes add up to, libzstd expands the record straight into it, and that array is the one written, so the result is held once. When the host cannot make an array that long, or libzstd cannot add up the sizes, such as for a record that ends inside a frame, the record is fed to libzstd as it arrives instead, and the verbose stream says why. The records after the first are fed as they arrive, after a copy of what the first expanded to, and the result is held twice from then on.

## Samples are held in memory

`New-ZstdDictionary` reads every sample into one buffer, since libzstd's trainer takes the samples that way, and holds the dictionary it trains, at most `-MaxSize` bytes. libzstd's trainer refuses 4294967295 bytes or more of samples in all. It takes no budget.
