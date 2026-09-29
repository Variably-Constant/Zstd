---
title: How To Run The Tests
weight: 2
---

The lint, the unit tests, the Pester suite in both hosts, the large-file check, and what CI runs. Source: `src/lib.rs` (`mod tests`), `tests/Zstd.Tests.ps1`, `tests/LargeFile.Gate.ps1`, `.github/workflows/ci.yml`, and cargo-pwrs's `test` command.

The output quoted on this page is from one run of each command on Windows 11 Pro 10.0.26200 on an AMD Ryzen 9 7900X, of the code this page describes.

## Lint

```text
cargo clippy --release --all-targets -- -D warnings
```

It exits 0 with no warnings.

## Unit tests and the Pester suite

The suite runs on Pester 6.2.0. Windows ships Pester 3.4.0, which it does not run on, so save Pester 6.2.0 where the test runner looks for it, once:

```text
Save-Module -Name Pester -RequiredVersion 6.2.0 -Path .\target\pester
cargo pwrs test --release
```

`cargo pwrs` here is cargo-pwrs 0.3.0, installed as [Building From Source](Building-From-Source.md) sets it up. It runs the Rust unit tests in `src/lib.rs`, builds the module, and runs `tests/Zstd.Tests.ps1` in PowerShell 7 and then in Windows PowerShell 5.1, with `PWRS_MODULE` naming the module folder. Each host prints a line such as `pwrs pester host=7.6.6 pester=6.2.0 passed=<n> failed=<n>`, and a failing host ends the run. The runner loads Pester from `PWRS_PESTER_PATH` when it is set, else from `target\pester\Pester` when that folder exists, else from the host's module path. The result lines from the run:

```text
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.96s
Tests Passed: 87, Failed: 0, Skipped: 0, Inconclusive: 0, NotRun: 0
pwrs pester host=7.6.6 pester=6.2.0 passed=87 failed=0
Tests Passed: 87, Failed: 0, Skipped: 0, Inconclusive: 0, NotRun: 0
pwrs pester host=5.1.26100.9444 pester=6.2.0 passed=87 failed=0
pwrs: surface check: every cmdlet ran, every pipeline parameter took a record, every ShouldProcess asked
```

Four of the Pester tests stop a command partway: `Compress-Zstd` and `Expand-Zstd` on a file, once with `-Force` over an existing file, and `New-ZstdDictionary` while it trains. Each checks that no partial output is left, and that a file already at the destination is unchanged.

## The large-file check

```text
.\tests\LargeFile.Gate.ps1 -Module .\target\pwrs\Zstd -Work <folder>
```

It concatenates the files under `C:\Windows\System32` into a 5 GiB source and takes its first 1 GiB as a second one. It then runs `Compress-Zstd` and `Expand-Zstd` on each, at level 3 on the calling thread and at level 19 with 8 worker threads, in Windows PowerShell 5.1 and in PowerShell 7, each command in a process of its own. Every expansion is compared with its source byte for byte. A command is bounded when its peak commit, the memory Windows backs with RAM or the page file, is less on 5 GiB than one and a half times its peak on 1 GiB. Each command's peak working set is recorded beside its commit and not bounded, since the pages of a mapped file count toward the working set while they are in use. It needs up to about 14 GB of free space under `-Work`, removes what it wrote when it finishes, and exits 1 when an expansion differs or a command is not bounded. The case lines and the last line from the run:

```text
CASE host=powershell level=3 threads=0 identical=True bounded=True input_growth=5.00 compress_commit_growth=1.117 expand_commit_growth=0.985
CASE host=powershell level=19 threads=8 identical=True bounded=True input_growth=5.00 compress_commit_growth=1.006 expand_commit_growth=1.002
CASE host=pwsh level=3 threads=0 identical=True bounded=True input_growth=5.00 compress_commit_growth=1.142 expand_commit_growth=0.926
CASE host=pwsh level=19 threads=8 identical=True bounded=True input_growth=5.00 compress_commit_growth=1.006 expand_commit_growth=1.053
GATE cases=4 failures=0
```

The peaks themselves are in [Streaming And Memory](Streaming-And-Memory.md).

## In CI

`.github/workflows/ci.yml` runs the same steps on a `windows-latest` and a `windows-11-arm` runner, at every push to main and every pull request: it installs Rust's stable toolchain with clippy, saves Pester 6.2.0 for both hosts and points `PWRS_PESTER_PATH` at it, puts LLVM's `clang-cl` on `PATH` on the x64 runner, installs cargo-pwrs 0.3.0, checks that the committed `Cargo.lock` resolves every dependency, runs clippy with warnings denied, then `cargo pwrs test --release`, checks that the build left `Cargo.lock` as committed, and uploads the module folder it built and tested. The package's `win-arm64` library is the one the `windows-11-arm` job built and tested from the released commit.
