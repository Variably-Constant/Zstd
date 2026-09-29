# Changelog

What changed in each version of the Zstd module. The repository ships as
a single root commit that is rewritten on every release, so this file is
the record of what came before it; the commit log is not.

## 0.1.0

### Added

- The crate, scaffolded with `cargo pwrs new` from cargo-pwrs 0.2.1: a
  PowerShell module named `Zstd`, built from the crate `zstd-pwsh` on
  PoWerRuSt 0.3.0 and the zstd crate 0.14, which compiles libzstd 1.5.7
  into the module with its worker threads enabled, and the memmap2 crate
  0.9, which maps files into memory. The crate is in Rust's 2024 edition
  and declares `rust-version = "1.98"`, so Cargo refuses to build it with
  an older Rust.
- PoWerRuSt comes from crates.io at 0.3.0, pinned in `Cargo.lock`, and
  the module is built with cargo-pwrs 0.3.0, which writes
  `THIRD-PARTY-NOTICES.txt` into the module folder: at its root for the
  assemblies PWRS compiles, and under each `runtimes/<rid>` with the
  license and license files of each crate compiled into that native
  library.
- libzstd is built without its decoders for the formats before zstd 1.0
  (the zstd crate's `legacy` feature), keeping its worker threads and its
  dictionary trainer; `Expand-Zstd` refuses such data with
  `ZstdInvalidData`. The zstd crate's `experimental` feature gives the
  module libzstd's memory estimates and its stable buffers.
- `.github/workflows/ci.yml` builds the module from the committed lock,
  runs clippy and `cargo pwrs test` on an x64 and an Arm64 Windows runner
  (`windows-latest` and `windows-11-arm`), with cargo-pwrs 0.3.0, Pester
  6.2.0 saved for both hosts and, on x64, LLVM's `clang-cl` put on
  `PATH`, and uploads each runner's module folder.
- The package carries a native library for `win-x64` and one for
  `win-arm64`. The Arm64 one is the library the `windows-11-arm` job
  built and tested from the released commit, joined to the x64 folder
  with `cargo pwrs merge`.
- `.cargo/config.toml` makes every build of this repository the x86-64
  baseline with the C runtime linked in statically, so the native library
  imports only `KERNEL32.dll`, `ntdll.dll` and
  `api-ms-win-core-synch-l1-2-0.dll` and needs no Visual C++
  Redistributable. CI leaves `RUSTFLAGS` unset, since it would replace
  those flags. The same file names LLVM's `clang-cl`, which has to be on
  `PATH`, as the C compiler for libzstd in the x64 build, with
  `-fgnuc-version`, under which libzstd compiles its BMI2 code paths
  beside the baseline ones and picks them at run time on a CPU that has
  BMI2. The Arm64 build links the C runtime statically as well, with
  the toolchain's own C compiler.
- The module manifest names Mark Newton as the author, Variably
  Constant as the company and `(c) 2026 Mark Newton` as the copyright.
  Its description says what each cmdlet does, that the module is bound
  to Rust with PoWerRuSt, and which platforms and hosts it runs on, and
  links this repository, PWRS and PoWerRuSt on crates.io. ProjectUri is
  this repository, LicenseUri its `LICENSE`, IconUri PWRS's logo, and
  the release notes point at this file. The tags are `powershell`,
  `zstd`, `zstandard`, `compression` and `Windows`, besides the `pwrs`
  tag cargo-pwrs adds.
- `Compress-Zstd`, which compresses files (`-Path`) or the bytes piped
  to it (`-InputObject`), each into one zstd frame that carries a
  content checksum. `-Level` takes 1 to 22 and defaults to 3, libzstd's
  own default; `-Threads` takes 0 to 256 worker threads and defaults to
  0, compression on the calling thread.
- `Expand-Zstd`, which expands every frame in a file or in the bytes
  piped to it, in order. Input that is empty, ends inside a frame, holds
  bytes after a frame that do not begin another, fails a frame's
  checksum, or has a frame whose header records a content size other than
  what it holds is refused with the error id `ZstdInvalidData`. A frame whose
  header names a dictionary other than the one given, if any, is refused
  with `ZstdDictionaryMismatch`, whose message names the id the frame
  needs and what was given.
- `-DictionaryPath` (a file, named as written) and `-Dictionary` (a
  `byte[]`) on both cmdlets, for files and bytes, in six parameter sets
  so the binder refuses the two together. Bytes in zstd's dictionary
  format are checked by libzstd both ways before any input is read, and
  refused with `ZstdDictionaryInvalid` when it refuses their tables; any
  other bytes are used as raw content.
- `New-ZstdDictionary`, which trains a dictionary with libzstd's trainer
  from the sample files `-Path` or `-LiteralPath` names, or that are piped
  to it, each file one sample, and writes it to `-DestinationPath`. `-MaxSize` takes 256 to 2147483647 bytes and
  defaults to 112640, the zstd command line tool's default; `-Force`
  replaces an existing file, `-PassThru` writes the dictionary's
  `FileInfo`, and `-WhatIf` and `-Confirm` work. A sample that cannot be
  read gets its own error record and
  training goes on with the rest; training that libzstd refuses, such as
  from fewer than 7 samples, is the error id
  `ZstdDictionaryTrainingFailed`.
- On files, `-Path` takes wildcards and several paths, `-LiteralPath`
  (aliases `PSPath` and `LP`) takes names as written, and a file piped
  from `Get-ChildItem` or `Get-Item` binds to `-LiteralPath` by its
  `PSPath`. `-DestinationPath`, `-Force` and `-PassThru` belong to the
  `Path` and `LiteralPath` sets, and the binder refuses them beside
  `-InputObject`. Each file is handled on its own: one that fails gets
  its own error record and the rest go on. Each is written to a temporary
  file beside its destination that is renamed onto it only when complete.
  A file is compressed from a map of it, which libzstd reads where it
  lies, handed over a mebibyte more at a time, or read in 1 MiB steps when
  Windows will not map it. A file is expanded in 1 MiB steps; one with a
  frame whose window does not fit that way is written in place instead,
  through maps of the file and of a temporary file of its content size,
  which takes a frame of any window, when every frame records its content
  size. The destination is the file beside itself as `name.zst`, or
  as the name without `.zst` when expanding (`ZstdNoDestination` for a
  name without it), a folder given as `-DestinationPath` under that
  name, or the one file `-DestinationPath` names, which only the first
  input of a command may take (`ZstdDestinationReused`). An existing
  destination is replaced only with `-Force`; `-WhatIf`, `-Confirm` and
  `-PassThru`, which writes each file written as `Get-Item` gives it,
  work on files. A frame written from a file records the content size.
- On bytes, `-InputObject` is declared `byte[]`, so the binder converts
  each record (a number from 0 to 255 to a `byte[]` of one) or refuses it
  with its own `InputObjectNotBound` error, and takes an empty
  `byte[]`. A `byte[]` another command wrote, which arrives wrapped in a
  `PSObject`, binds as the array itself rather than element by element.
  `-InputObject` is not mandatory: a `$null` record, or the bytes set
  chosen with no `-InputObject` given, gets `ZstdNoInput`, and the records
  around a `$null` go on as if it had not been sent. A single record is
  held without being copied, through a pinned borrow, and compressed in
  one piece with the content size in
  its frame; from a second record on, every record is fed to libzstd as
  it arrives and the frame carries no content size. `Expand-Zstd`
  expands a first record whose frames all record their content size
  straight into a `byte[]` of the size they add up to, which the host
  makes and which is the one written, and takes a frame of any window
  that way; other bytes, the first record when the host cannot make the
  array, and the records after the first are fed to libzstd as they
  arrive. Either way the result is written as one `byte[]` when the
  pipeline ends, and when no record reached the cmdlet it writes nothing,
  output or error.
- `-MaxMemory` on `Compress-Zstd` and `Expand-Zstd`: the most memory, in
  bytes, the command may commit, 1MB or more. It counts what libzstd
  takes, from libzstd's own estimates, a dictionary, the command's own
  buffers and what the host commits while it runs, the page tables of a
  file's map (4 KB for each 2 MiB of the file), and on bytes the result:
  once when a single record is expanded straight into the `byte[]`
  written, and otherwise twice, as it is built and as the `byte[]`
  written. A file whose map does not fit is read a mebibyte at a time
  instead, and is not written in place. Compression uses as many of the `-Threads` worker
  threads as fit, estimated before any work starts, with a warning when
  that is fewer than asked for, and is refused with `ZstdBudgetTooSmall`
  when the calling thread, or one worker, does not fit. Expansion refuses
  a frame expanded in 1 MiB steps whose window does not fit, when its
  file cannot be written in place, with
  `ZstdWindowTooLarge`, before libzstd sees it, and on bytes refuses a
  result that would not fit, or stops one that outgrows the budget, with
  `ZstdBudgetTooSmall`. Without `-MaxMemory`, such a frame may have a
  window of up to 128 MiB, libzstd's own limit, and compression uses the
  worker threads asked for. `New-ZstdDictionary` takes no budget.
- Every failure is an error record with a `Zstd` error id and a
  category; the wiki's Error Reference lists them.
- `tests/Zstd.Tests.ps1`, a Pester suite that `cargo pwrs test` runs on
  Pester 6.2.0 in PowerShell 7 and in Windows PowerShell 5.1, and Rust
  unit tests of the stream and training functions in `src/lib.rs`, among
  them compression and expansion with a trained dictionary and with raw
  content. Three of the suite's tests stop a command with
  `PowerShell.Stop()` partway through a file and check that neither the
  destination nor the temporary file is left, and that a file already at
  the destination survives a stopped `-Force` run; a fourth stops
  `New-ZstdDictionary` during training and checks that no dictionary is
  written.
- `tests/LargeFile.Gate.ps1`, which runs a 5 GiB file built from the
  files under `C:\Windows\System32`, and its first 1 GiB, through both
  cmdlets in both hosts, compares every expansion with its source byte
  for byte, and bounds each process's peak commit on 5 GiB at less than
  one and a half times its peak on 1 GiB, recording its peak working set
  beside it.
- The documentation: a wiki under `wiki/content`, for the PowerShell
  user first and organized by the Diataxis framework, with a tutorial
  that installs the module from the PowerShell Gallery, six how-to
  guides, four explanations, a reference page for each cmdlet, one for
  the error ids and one for where the module has been run, and a section
  on building from source and running the tests. Every example in it
  ran in Windows PowerShell 5.1 and PowerShell 7 against the module
  built from this source, and the output it shows is from that run.
  `wiki/` holds the Hugo site the pages are laid out for, with the
  Hextra theme, which `.github/workflows/wiki-deploy.yml` builds with
  Hugo 0.166.0 and deploys to GitHub Pages at
  variably-constant.github.io/Zstd on every push to main. `README.md`
  gives the features, a quick start and where the module has been run,
  and links into the wiki.
