---
title: Error Reference
weight: 4
---

Every error id the module raises. Source: `src/lib.rs`, the functions that build error records: `failed`, `budget_too_small`, `unestimated`, `empty_input`, `open_input`, `input_error`, `output_error`, `provider_path`, `matched_files`, `named_default`, `destination`, `check_destination`, `write_through_temporary`, `write_mapped_through_temporary`, `dictionary_refused`, `dictionary_from_file`, `dictionary_from_bytes`, `training_failed`, `Samples::append`, `no_input`, `not_bytes` and `Settings::from_parameters`.

The id is the first part of the error record's `FullyQualifiedErrorId`; the second part names the cmdlet's class, as in `ZstdInvalidData,Pwrs.Modules.Zstd.ExpandZstdCommand`. A record about a file carries the file's path as its target; a record about bytes on the pipeline carries none. [How To Handle Errors](How-To-Handle-Errors.md) shows how to collect and act on them.

Every error is non-terminating, reported for one file or for the pipeline's bytes while the command goes on, except `ZstdInputNotBytes`, which ends the command. A dictionary that cannot be loaded is reported once, before any input is read, and the command then does nothing more.

## The ids

| Id | Category | Cmdlets | Raised when |
|---|---|---|---|
| `ZstdInputNotFound` | ObjectNotFound | all three | A `-Path` value names nothing or its wildcards match nothing, or a `-LiteralPath` or `-DictionaryPath` value names nothing. |
| `ZstdInputIsDirectory` | InvalidArgument | all three | `-Path`, `-LiteralPath` or `-DictionaryPath` names a directory. |
| `ZstdReadFailed` | ReadError, or the category of the I/O error, such as PermissionDenied | all three | An input cannot be opened or read, or a file ends before the length it had when it was opened. |
| `ZstdPathNotResolved` | InvalidArgument | all three | A path does not resolve in the session, such as one on a drive the session lacks, or a `-LiteralPath`, `-DestinationPath` or `-DictionaryPath` value does not resolve to exactly one path. |
| `ZstdDestinationExists` | ResourceExists | all three | The destination file exists and `-Force` was not given. |
| `ZstdDestinationIsDirectory` | InvalidArgument | all three | The destination is a directory: a folder already named `name.zst`, or `name` when expanding, where a file goes, or a folder given to `New-ZstdDictionary` as `-DestinationPath`. |
| `ZstdDestinationReused` | InvalidOperation | `Compress-Zstd`, `Expand-Zstd` | A second input of one command would go to the one file `-DestinationPath` names. |
| `ZstdNoDestination` | InvalidArgument | `Expand-Zstd` | A file whose name does not end in `.zst` has no default name, and `-DestinationPath` is missing or names a folder. |
| `ZstdWriteFailed` | WriteError, or the category of the I/O error | all three | The output cannot be created, written or renamed into place. |
| `ZstdInvalidData` | InvalidData | `Expand-Zstd` | The input is empty, ends inside a frame, holds bytes after a frame that do not begin another, fails a frame's checksum, has a frame whose header records a content size other than what it holds, is otherwise not zstd data, or is in a format from before zstd 1.0, whose decoders the module leaves out. |
| `ZstdDictionaryMismatch` | InvalidArgument | `Expand-Zstd` | A frame's header names a dictionary other than the one given, or names one when none was given. |
| `ZstdDictionaryInvalid` | InvalidData | `Compress-Zstd`, `Expand-Zstd` | The dictionary given begins with zstd's dictionary magic number, and libzstd refuses its tables. |
| `ZstdCompressFailed` | InvalidOperation | `Compress-Zstd` | libzstd refuses a setting or fails while compressing, or its memory cannot be estimated. |
| `ZstdWindowTooLarge` | LimitsExceeded | `Expand-Zstd` | A frame expanded a chunk at a time asks for a window that does not fit: one over 128 MiB, libzstd's own limit, without `-MaxMemory`, or one that takes more to expand than `-MaxMemory`. It is refused before libzstd sees it, unless its file can be written in place, which takes a frame of any window: every frame in the file records its content size, and with `-MaxMemory` the page tables of the two files' maps fit. Bytes are not checked this way when every frame in their first record records its content size: that record is expanded straight into the `byte[]` written, which takes a frame of any window. |
| `ZstdBudgetTooSmall` | LimitsExceeded | `Compress-Zstd`, `Expand-Zstd` | `-MaxMemory` is too small: compressing on the calling thread, or with one worker thread, does not fit it; with a dictionary, expanding does not fit it before any frame; or, on bytes, the result does not fit what the rest leaves: counted once, as its frames record it, when every frame in the first record records its content size and the record is expanded straight into the `byte[]` written, and otherwise counted twice, as a frame's header records its size or as the result grows. |
| `ZstdDictionaryTrainingFailed` | InvalidOperation | `New-ZstdDictionary` | libzstd's trainer refuses the samples or the size, such as fewer than 7 samples. |
| `ZstdNoInput` | InvalidArgument | `Compress-Zstd`, `Expand-Zstd` | No bytes arrived where they were expected: a `$null` record, or the `InputObject` set chosen with no `-InputObject` given, as for `-Dictionary` alone. The records around a `$null` go on as if it had not been sent. |
| `ZstdInputNotBytes` | InvalidType | `Compress-Zstd`, `Expand-Zstd` | A record's bytes cannot be read as a `byte[]`. PowerShell's binder makes every record a `byte[]` or refuses it before the cmdlet sees it, so this is not expected. |
| `ZstdOutOfMemory` | ResourceUnavailable | all three | Memory for the bytes, the output, a sample or a dictionary cannot be allocated. |
| `ZstdThreadsInvalid` | InvalidArgument | `Compress-Zstd` | `-Threads` is not a thread count. The parameter's range, 0 to 256, refuses such values first. |

## The messages

Each message names the path involved, or "the input" and "the output" for bytes on the pipeline. Text in angle brackets is filled in; libzstd's reasons are its own words, such as `Restored data doesn't match checksum`.

| Id | Message |
|---|---|
| `ZstdInputNotFound` | `Cannot find '<path>'.` |
| `ZstdInputIsDirectory` | `'<path>' is a directory, and <parameter> takes a file.`, where the parameter is the one that named it: `-Path`, `-LiteralPath` or `-DictionaryPath`. |
| `ZstdReadFailed` | `Cannot read '<path>': <reason>` |
| `ZstdPathNotResolved` | `Cannot resolve '<path>': <reason>`, or `'<path>' resolves to <n> paths, and one file is needed.` |
| `ZstdDestinationExists` | `'<path>' already exists. Use -Force to replace it.` |
| `ZstdDestinationIsDirectory` | `'<path>' is a directory, and -DestinationPath takes a file.` |
| `ZstdDestinationReused` | `-DestinationPath '<path>' is one file, and an earlier input of this command took it, so '<input>' was not written.` |
| `ZstdNoDestination` | `'<path>' does not end in .zst, so it has no default destination. Name one with -DestinationPath.` |
| `ZstdWriteFailed` | `Cannot write '<path>': <reason>`, or `'<path>' does not name a file.` |
| `ZstdInvalidData` | `Cannot expand '<path>': it is not valid zstd data (<libzstd's reason>).`, or `Cannot expand '<path>': it is empty, and zstd data holds at least one frame.` A frame whose header records more than it holds gives the reason `a frame records a content size of <n> bytes and holds <n>` when it is expanded into its `byte[]` or written in place; a chunk at a time it gives libzstd's `Data corruption detected`, or that reason where libzstd does not check the size, at a frame that ends with an empty block. A frame that holds more than it records gives libzstd's `Destination buffer is too small` when it is expanded into its `byte[]` or written in place, and a chunk at a time `Destination buffer is too small` or `Data corruption detected`. |
| `ZstdDictionaryMismatch` | `Cannot expand '<path>': a frame in it needs dictionary <id>, and <what was given>.`, where what was given is `no dictionary was given`, `dictionary <id> was given`, or `the dictionary given is raw content, which carries no id`. When the frame's header cannot be read back for its id, `needs another dictionary` stands in for `needs dictionary <id>`. |
| `ZstdDictionaryInvalid` | `Cannot use '<path>' as a dictionary: it begins with zstd's dictionary magic number, and libzstd refused its tables (<libzstd's reason>).`, or the same beginning `Cannot use the bytes given to -Dictionary as a dictionary:` |
| `ZstdCompressFailed` | `Cannot compress '<path>': libzstd reported <reason>`, or `Cannot estimate the memory compressing '<path>' takes: <reason>` |
| `ZstdWindowTooLarge` | `Cannot expand '<path>': a frame in it has a window of <n> bytes, more than the 134217728 bytes libzstd expands without -MaxMemory; -MaxMemory of <n> bytes or more lets it.`, on bytes whose frame records no content size ending `, with room besides for its result, which is held in memory twice.`; or `Cannot expand '<path>': a frame in it has a window of <n> bytes, which takes <n> bytes to expand, more than -MaxMemory of <n> bytes.`; or `Cannot expand '<path>': a frame in it has a window of <n> bytes, more than libzstd expands at all.` With a dictionary, `, <n> of them for the dictionary` follows the bytes it takes. |
| `ZstdBudgetTooSmall` | `Cannot compress '<path>' within -MaxMemory of <n> bytes: at level <n> on the calling thread it needs about <n> bytes.`, or `with one worker thread`, followed with a dictionary by `, <n> of them for the dictionary`, and on bytes by `, <n> of them for its result of up to <n> bytes, which is held in memory twice, as it is built and as the byte[] written`; `Cannot expand '<path>' within -MaxMemory of <n> bytes: with its dictionary, expanding needs about <n> bytes before any frame, <n> of them for the dictionary.`; `Cannot expand the input within -MaxMemory of <n> bytes: it needs about <n> bytes, <n> of them for its result, as its frames record, which is held in memory once, as the byte[] it is expanded into.`, with a dictionary `<n> of them for the dictionary and <n> for its result`; `Cannot expand the input within -MaxMemory of <n> bytes: its result of <n> bytes, as its frames record, would be held in memory twice, as it is built and as the byte[] written, and <n> bytes of result is the most that fits.`; or `Cannot <compress or expand> the input within -MaxMemory of <n> bytes: its result is held in memory twice, as it is built and as the byte[] written, and it outgrew <n> bytes, the most that fits.` |
| `ZstdDictionaryTrainingFailed` | `Cannot train a dictionary from <n> samples of <n> bytes in all: libzstd reported '<reason>'. Its trainer needs at least 7 samples and 8 bytes of them in all, and refuses 4294967295 bytes or more in all.`, or `-MaxSize <n> is not a size in bytes: <reason>`, which the parameter's range refuses first |
| `ZstdNoInput` | `No bytes were given: -InputObject takes a byte[], from the pipeline or by name.` |
| `ZstdInputNotBytes` | `-InputObject takes a byte[], and this record reached it as a <type>.`, or `Cannot read this byte[]: <reason>` |
| `ZstdOutOfMemory` | `Cannot hold <what> in memory: <reason>`, or `Cannot load <what> as a dictionary: <reason>` |
| `ZstdThreadsInvalid` | `-Threads <n> is not a thread count: <reason>` |

When the temporary file of a failed write cannot be removed either, the record's message goes on to name it: ` The partial output '<path>' could not be removed: <reason>`.

## Errors that come from PowerShell

Some arguments never reach the cmdlet. PowerShell's parameter binder checks them first, and its records carry its own ids:

| Id | When |
|---|---|
| `AmbiguousParameterSet` | Parameters from different parameter sets together, such as `-InputObject` with `-DestinationPath`, or `-Dictionary` with `-DictionaryPath`. The command does not run. |
| `InputObjectNotBound` | A pipeline record that cannot be made a `byte[]`, such as text or a number over 255. The command goes on with the next record. |
| `MissingMandatoryParameter` | No input at all where the default `Path` set is chosen, such as the cmdlet run with no parameters, in a host that cannot ask for `-Path`. The command does not run. |
| `ParameterArgumentValidationError` | A value outside a parameter's range, such as `-Level 23`, `-Threads 257` or `-MaxMemory` below 1MB. The command does not run. |
