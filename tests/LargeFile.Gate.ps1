# Runs files of 1 GiB and of several GiB through Compress-Zstd and
# Expand-Zstd in Windows PowerShell 5.1 and in PowerShell 7, each command
# in a process of its own, and reports each process's peak commit (its
# private bytes) and peak working set beside the file's size. Every
# expansion is compared with its source byte for byte.
#
#   .\tests\LargeFile.Gate.ps1 -Module <folder holding Zstd.psd1> -Work <folder>
#
# The large source is real data: the files under C:\Windows\System32 in
# ordinal order of their paths, concatenated until -Size bytes (5 GiB by
# default, which crosses 2^32 inside the file); a file or folder that
# cannot be listed or opened, and a folder that is a reparse point, is
# skipped and counted, and a file whose read fails ends there and is
# counted too. The small source is its first 1 GiB. Two cases run in each host: level 3 on the calling thread, and
# level 19 with 8 worker threads, whose 8 MiB window is the largest any
# level up to 19 uses. A command's memory is bounded when its peak commit
# on the large source is less than one and a half times its peak commit
# on the small one, while the input is five times larger: memory that grew
# with the input would grow with it about fivefold. The working set is
# recorded beside it and not bounded, since the pages of a file mapped
# into memory count in it however little is committed. The gate exits 0
# when every expansion matched its source and every command was bounded,
# and 1 otherwise. It removes the files it wrote under -Work unless -Keep
# is given.
#
# -Step, -From, -To, -Level and -Threads are how the gate runs one
# command in a child process; the child prints one RESULT line.
param(
    [Parameter(Mandatory = $true)][string] $Module,
    [Parameter(Mandatory = $true)][string] $Work,
    [long] $Size = 5GB,
    [switch] $Keep,
    [ValidateSet('', 'Compress', 'Expand')][string] $Step = '',
    [string] $From = '',
    [string] $To = '',
    [int] $Level = 3,
    [int] $Threads = 0
)
$ErrorActionPreference = 'Stop'

if ($Step) {
    Import-Module (Join-Path $Module 'Zstd.psd1')
    $self = [System.Diagnostics.Process]::GetCurrentProcess()
    $baseline = $self.PeakWorkingSet64
    $baselineCommit = $self.PeakPagedMemorySize64
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    if ($Step -eq 'Compress') {
        Compress-Zstd -Path $From -DestinationPath $To -Level $Level -Threads $Threads -Force
    } else {
        Expand-Zstd -Path $From -DestinationPath $To -Force
    }
    $clock.Stop()
    $self.Refresh()
    'RESULT step={0} host={1}/{2} level={3} threads={4} ms={5} in={6} out={7} baseline_ws={8} peak_ws={9} baseline_commit={10} peak_commit={11}' -f $Step,
        $PSVersionTable.PSEdition, $PSVersionTable.PSVersion, $Level, $Threads, $clock.ElapsedMilliseconds,
        (Get-Item -LiteralPath $From).Length, (Get-Item -LiteralPath $To).Length,
        $baseline, $self.PeakWorkingSet64, $baselineCommit, $self.PeakPagedMemorySize64
    exit 0
}

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;

namespace ZstdGate
{
    public static class Files
    {
        // Writes the files under root, in ordinal order of their full
        // paths, one after another into destination until it holds size
        // bytes; the last file is cut short. A file or folder that cannot
        // be listed or opened is skipped, and so is a folder that is a
        // reparse point; a file whose read fails ends there, with the bytes
        // read before the failure kept. Each of these counts as skipped.
        // Returns { bytes written, files opened, skipped }.
        public static long[] Concatenate(string root, string destination, long size)
        {
            List<string> paths = new List<string>();
            long skipped = 0;
            Collect(root, paths, ref skipped);
            paths.Sort(StringComparer.Ordinal);
            byte[] buffer = new byte[1 << 20];
            long written = 0;
            long used = 0;
            using (FileStream output = new FileStream(destination, FileMode.Create, FileAccess.Write, FileShare.None, 1 << 20))
            {
                foreach (string path in paths)
                {
                    if (written >= size) break;
                    FileStream input;
                    try { input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete, 1 << 16); }
                    catch (IOException) { skipped++; continue; }
                    catch (UnauthorizedAccessException) { skipped++; continue; }
                    using (input)
                    {
                        used++;
                        while (written < size)
                        {
                            int want = (int)Math.Min(buffer.Length, size - written);
                            int n;
                            try { n = input.Read(buffer, 0, want); }
                            catch (IOException) { skipped++; break; }
                            if (n == 0) break;
                            output.Write(buffer, 0, n);
                            written += n;
                        }
                    }
                }
            }
            return new long[] { written, used, skipped };
        }

        static void Collect(string folder, List<string> paths, ref long skipped)
        {
            string[] files;
            string[] folders;
            try
            {
                files = Directory.GetFiles(folder);
                folders = Directory.GetDirectories(folder);
            }
            catch (IOException) { skipped++; return; }
            catch (UnauthorizedAccessException) { skipped++; return; }
            paths.AddRange(files);
            foreach (string child in folders)
            {
                FileAttributes attributes;
                try { attributes = File.GetAttributes(child); }
                catch (IOException) { skipped++; continue; }
                catch (UnauthorizedAccessException) { skipped++; continue; }
                if ((attributes & FileAttributes.ReparsePoint) != 0) { skipped++; continue; }
                Collect(child, paths, ref skipped);
            }
        }

        // Writes the first size bytes of source to destination; returns
        // the bytes written, fewer only when source is shorter.
        public static long CopyPrefix(string source, string destination, long size)
        {
            byte[] buffer = new byte[1 << 20];
            long written = 0;
            using (FileStream input = new FileStream(source, FileMode.Open, FileAccess.Read, FileShare.Read, 1 << 16))
            using (FileStream output = new FileStream(destination, FileMode.Create, FileAccess.Write, FileShare.None, 1 << 20))
            {
                while (written < size)
                {
                    int n = input.Read(buffer, 0, (int)Math.Min(buffer.Length, size - written));
                    if (n == 0) break;
                    output.Write(buffer, 0, n);
                    written += n;
                }
            }
            return written;
        }

        // The offset of the first byte at which the files differ, the
        // shorter one's length when one is a prefix of the other, or -1
        // when they are identical.
        public static long FirstDifference(string a, string b)
        {
            byte[] x = new byte[1 << 20];
            byte[] y = new byte[1 << 20];
            using (FileStream fa = new FileStream(a, FileMode.Open, FileAccess.Read, FileShare.Read, 1 << 16, FileOptions.SequentialScan))
            using (FileStream fb = new FileStream(b, FileMode.Open, FileAccess.Read, FileShare.Read, 1 << 16, FileOptions.SequentialScan))
            {
                long offset = 0;
                while (true)
                {
                    int na = Fill(fa, x);
                    int nb = Fill(fb, y);
                    int n = Math.Min(na, nb);
                    for (int i = 0; i < n; i++)
                    {
                        if (x[i] != y[i]) return offset + i;
                    }
                    if (na != nb) return offset + n;
                    if (na == 0) return -1;
                    offset += n;
                }
            }
        }

        static int Fill(Stream s, byte[] buffer)
        {
            int total = 0;
            while (total < buffer.Length)
            {
                int n = s.Read(buffer, total, buffer.Length - total);
                if (n == 0) break;
                total += n;
            }
            return total;
        }
    }
}
'@

# Runs one command in a fresh process of the given host and returns its
# RESULT line, then that line's fields; anything else the child wrote is
# in the error when it fails. The child's standard error is collected as
# text: under 'Stop', Windows PowerShell would turn its first line into a
# terminating error before the exit code could be read.
function Invoke-Step([string] $Exe, [string] $Name, [string] $Source, [string] $Destination, [int] $StepLevel, [int] $StepThreads) {
    $ErrorActionPreference = 'Continue'
    $output = @(& $Exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $PSCommandPath -Module $Module -Work $Work -Step $Name -From $Source -To $Destination -Level $StepLevel -Threads $StepThreads 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $line = @($output | Where-Object { $_ -like 'RESULT *' })
    if ($code -ne 0 -or $line.Count -ne 1) {
        throw ("{0} {1} exited {2}: {3}" -f $Exe, $Name, $code, ($output -join ' | '))
    }
    $fields = @{}
    foreach ($pair in ($line[0].Substring(7) -split ' ')) {
        $at = $pair.IndexOf('=')
        $fields[$pair.Substring(0, $at)] = $pair.Substring($at + 1)
    }
    $line[0]
    $fields
}

# Compresses and expands one source in one host, compares the round trip,
# removes both outputs and returns one object describing the pair.
function Test-RoundTrip([hashtable] $Shell, [hashtable] $Case, [string] $Source) {
    $packed = Join-Path $Work ('round.L{0}.T{1}.zst' -f $Case.Level, $Case.Threads)
    $restored = Join-Path $Work 'round.restored.bin'
    $compressed = Invoke-Step $Shell.Exe 'Compress' $Source $packed $Case.Level $Case.Threads
    (Get-Date -Format 'HH:mm:ss') + ' ' + $compressed[0] | Out-Host
    $expanded = Invoke-Step $Shell.Exe 'Expand' $packed $restored 0 0
    (Get-Date -Format 'HH:mm:ss') + ' ' + $expanded[0] | Out-Host
    $difference = [ZstdGate.Files]::FirstDifference($Source, $restored)
    Remove-Item -LiteralPath $packed, $restored
    [pscustomobject]@{
        Size               = (Get-Item -LiteralPath $Source).Length
        Packed             = [long]$compressed[1]['out']
        CompressPeak       = [long]$compressed[1]['peak_commit']
        ExpandPeak         = [long]$expanded[1]['peak_commit']
        CompressBase       = [long]$compressed[1]['baseline_commit']
        ExpandBase         = [long]$expanded[1]['baseline_commit']
        CompressPeakWs     = [long]$compressed[1]['peak_ws']
        ExpandPeakWs       = [long]$expanded[1]['peak_ws']
        CompressMs         = [long]$compressed[1]['ms']
        ExpandMs           = [long]$expanded[1]['ms']
        Difference         = $difference
    }
}

$null = New-Item -ItemType Directory -Force -Path $Work
$native = Get-ChildItem -LiteralPath (Join-Path $Module 'runtimes\win-x64\native') -Filter '*.dll' | Select-Object -First 1
'MODULE {0} native {1} bytes {2} sha256 {3}' -f $Module, $native.Name, $native.Length, (Get-FileHash -LiteralPath $native.FullName -Algorithm SHA256).Hash

$system32 = Join-Path $env:SystemRoot 'System32'
$large = Join-Path $Work 'large.bin'
$small = Join-Path $Work 'small.bin'
$clock = [System.Diagnostics.Stopwatch]::StartNew()
$made = [ZstdGate.Files]::Concatenate($system32, $large, $Size)
'SOURCE {0} bytes {1} from {2} files under {3}, {4} entries skipped, {5} ms' -f $large, $made[0], $made[1], $system32, $made[2], $clock.ElapsedMilliseconds
if ($made[0] -ne $Size) { throw "The large source holds $($made[0]) bytes, short of the $Size asked for." }
$smallSize = [long]1GB
if ([ZstdGate.Files]::CopyPrefix($large, $small, $smallSize) -ne $smallSize) { throw "The small source is short of $smallSize bytes." }
'SOURCE {0} bytes {1}, the first bytes of {2}' -f $small, $smallSize, $large

$pwsh = Get-Command pwsh -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $pwsh) { throw 'pwsh is not on PATH, so PowerShell 7 cannot be run.' }
$shells = @(
    @{ Name = 'powershell'; Exe = (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') },
    @{ Name = 'pwsh'; Exe = $pwsh.Source }
)
$cases = @(@{ Level = 3; Threads = 0 }, @{ Level = 19; Threads = 8 })
$failures = 0
foreach ($shell in $shells) {
    foreach ($case in $cases) {
        $one = Test-RoundTrip $shell $case $small
        $many = Test-RoundTrip $shell $case $large
        $identical = ($one.Difference -eq -1) -and ($many.Difference -eq -1)
        $compressGrowth = $many.CompressPeak / $one.CompressPeak
        $expandGrowth = $many.ExpandPeak / $one.ExpandPeak
        $bounded = ($compressGrowth -lt 1.5) -and ($expandGrowth -lt 1.5)
        'CASE host={0} level={1} threads={2} identical={3} bounded={4} input_growth={5:F2} compress_commit_growth={6:F3} expand_commit_growth={7:F3}' -f $shell.Name,
            $case.Level, $case.Threads, $identical, $bounded, ($many.Size / $one.Size), $compressGrowth, $expandGrowth
        foreach ($run in $one, $many) {
            '  size={0} packed={1} compress_ms={2} expand_ms={3} compress_peak_commit={4} expand_peak_commit={5} compress_baseline_commit={6} expand_baseline_commit={7} compress_peak_ws={8} expand_peak_ws={9} first_difference={10}' -f $run.Size,
                $run.Packed, $run.CompressMs, $run.ExpandMs, $run.CompressPeak, $run.ExpandPeak, $run.CompressBase, $run.ExpandBase,
                $run.CompressPeakWs, $run.ExpandPeakWs, $run.Difference
        }
        if (-not ($identical -and $bounded)) { $failures++ }
    }
}
if (-not $Keep) { Remove-Item -LiteralPath $large, $small }
'GATE cases={0} failures={1}' -f ($shells.Count * $cases.Count), $failures
if ($failures -gt 0) { exit 1 }
exit 0
