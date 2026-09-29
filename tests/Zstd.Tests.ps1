# Pester suite for the Zstd module, run on Pester 6.2.0 in pwsh and in
# Windows PowerShell by `cargo pwrs test`. PWRS_MODULE points at the built
# module folder.
BeforeAll {
    Import-Module (Join-Path $env:PWRS_MODULE 'Zstd.psd1') -Force -ErrorAction Stop

    $script:compressId = 'Pwrs.Modules.Zstd.CompressZstdCommand'
    $script:expandId = 'Pwrs.Modules.Zstd.ExpandZstdCommand'

    # The SHA-256 of a byte array as hex, for comparing an output with
    # the input it came from.
    function Get-Digest([byte[]] $Bytes) {
        $sha = [System.Security.Cryptography.SHA256]::Create()
        try { [System.BitConverter]::ToString($sha.ComputeHash($Bytes)) } finally { $sha.Dispose() }
    }

    # Log-shaped text that compresses well, as UTF-8 bytes. The leading
    # comma on each return keeps the array whole; a bare array would be
    # unrolled into the pipeline one byte at a time.
    function New-Text([int] $Lines) {
        $text = [System.Text.StringBuilder]::new()
        for ($i = 0; $i -lt $Lines; $i++) {
            [void] $text.AppendFormat('01:{0:D2}:{1:D2}Z INFO worker-{2} handled request {3} in {4} ms', ($i % 60), (($i * 7) % 60), ($i % 8), (($i * 7919) % 100003), ($i % 997)).Append("`n")
        }
        return , [System.Text.Encoding]::UTF8.GetBytes($text.ToString())
    }

    # Pseudo-random bytes, which zstd cannot compress, the same for a seed.
    function New-Noise([int] $Count, [int] $Seed) {
        $bytes = [byte[]]::new($Count)
        [System.Random]::new($Seed).NextBytes($bytes)
        return , $bytes
    }

    # The byte arrays given, joined into one.
    function Join-Bytes {
        $joined = [System.IO.MemoryStream]::new()
        foreach ($part in $args) { $joined.Write($part, 0, $part.Length) }
        return , $joined.ToArray()
    }

    # A frame built by hand from RFC 8878 whose header names dictionary
    # $Id: the magic number; a single-segment header with a four-byte
    # dictionary id and a one-byte content size of 5; the id; one last raw
    # block of 5 bytes; the bytes of 'hello'.
    function New-DictionaryFrame([uint32] $Id) {
        return , (Join-Bytes ([byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x23)) ([System.BitConverter]::GetBytes($Id)) ([byte[]] (0x05, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F)))
    }
}

Describe 'Compress-Zstd and Expand-Zstd on files' {
    BeforeAll {
        # About 3.3 MB, so each stream runs through several 1 MiB chunks:
        # a megabyte of noise, then text.
        $script:data = Join-Bytes (New-Noise 1048576 7) (New-Text 40000)
        $script:source = Join-Path $TestDrive 'source.bin'
        [System.IO.File]::WriteAllBytes($script:source, $script:data)
        $script:sourceHash = (Get-FileHash -LiteralPath $script:source -Algorithm SHA256).Hash
    }

    It 'round-trips a file of several chunks byte for byte, on the calling thread and on four workers' {
        foreach ($threads in 0, 4) {
            $packed = Join-Path $TestDrive "source.$threads.zst"
            $restored = Join-Path $TestDrive "restored.$threads.bin"
            Compress-Zstd -Path $script:source -DestinationPath $packed -Threads $threads
            Expand-Zstd -Path $packed -DestinationPath $restored
            (Get-Item -LiteralPath $packed).Length | Should -BeLessThan $script:data.Length -Because "$threads threads"
            (Get-FileHash -LiteralPath $restored -Algorithm SHA256).Hash | Should -Be $script:sourceHash -Because "$threads threads"
        }
    }

    It 'writes one zstd frame with a checksum, whose header records the content size' {
        $packed = Join-Path $TestDrive 'header.zst'
        Compress-Zstd -Path $script:source -DestinationPath $packed
        $head = [System.IO.File]::ReadAllBytes($packed)
        [System.BitConverter]::ToString($head, 0, 4) | Should -Be '28-B5-2F-FD'
        $descriptor = $head[4]
        ($descriptor -band 0x04) | Should -Be 0x04
        ((($descriptor -shr 6) -ne 0) -or (($descriptor -band 0x20) -ne 0)) | Should -BeTrue
    }

    It 'round-trips an empty file' {
        $empty = Join-Path $TestDrive 'empty.txt'
        [System.IO.File]::WriteAllBytes($empty, [byte[]]::new(0))
        $packed = Join-Path $TestDrive 'empty.zst'
        $restored = Join-Path $TestDrive 'empty.out'
        Compress-Zstd -Path $empty -DestinationPath $packed
        (Get-Item -LiteralPath $packed).Length | Should -BeGreaterThan 0
        Expand-Zstd -Path $packed -DestinationPath $restored
        (Get-Item -LiteralPath $restored).Length | Should -Be 0
    }

    It 'resolves relative paths against the current PowerShell location, and binds them by position' {
        Push-Location -LiteralPath $TestDrive
        try {
            Compress-Zstd -Path 'source.bin' -DestinationPath 'relative.zst'
            Expand-Zstd 'relative.zst' 'relative.bin'
        } finally {
            Pop-Location
        }
        (Get-FileHash -LiteralPath (Join-Path $TestDrive 'relative.bin') -Algorithm SHA256).Hash | Should -Be $script:sourceHash
    }

    It 'expands wildcard characters in -Path, and takes them as written when escaped' {
        $folder = Join-Path $TestDrive 'wildcards'
        $null = New-Item -ItemType Directory -Path $folder
        foreach ($name in 'log[1].txt', 'log1.txt', 'log2.txt', 'other.dat') {
            [System.IO.File]::WriteAllBytes((Join-Path $folder $name), (New-Text 100))
        }
        # [1] is a character class, so this pattern names log1.txt only.
        Compress-Zstd -Path (Join-Path $folder 'log[1].txt')
        [System.IO.File]::Exists((Join-Path $folder 'log1.txt.zst')) | Should -BeTrue
        [System.IO.File]::Exists((Join-Path $folder 'log[1].txt.zst')) | Should -BeFalse
        Compress-Zstd -Path (Join-Path $folder 'log`[1`].txt')
        [System.IO.File]::Exists((Join-Path $folder 'log[1].txt.zst')) | Should -BeTrue
        Compress-Zstd -Path (Join-Path $folder 'log2*'), (Join-Path $folder '*.dat')
        [System.IO.File]::Exists((Join-Path $folder 'log2.txt.zst')) | Should -BeTrue
        [System.IO.File]::Exists((Join-Path $folder 'other.dat.zst')) | Should -BeTrue
    }

    It 'writes beside each file under its default name, and expands back beside it' {
        $folder = Join-Path $TestDrive 'defaults'
        $null = New-Item -ItemType Directory -Path $folder
        $source = Join-Path $folder 'app.log'
        [System.IO.File]::Copy($script:source, $source)
        Compress-Zstd -Path $source
        $packed = Join-Path $folder 'app.log.zst'
        [System.IO.File]::Exists($packed) | Should -BeTrue
        $elsewhere = Join-Path $TestDrive 'defaults-out'
        $null = New-Item -ItemType Directory -Path $elsewhere
        [System.IO.File]::Copy($packed, (Join-Path $elsewhere 'APP.LOG.ZST'))
        Expand-Zstd -Path (Join-Path $elsewhere 'APP.LOG.ZST')
        (Get-FileHash -LiteralPath (Join-Path $elsewhere 'APP.LOG') -Algorithm SHA256).Hash | Should -Be $script:sourceHash
    }

    It 'refuses to expand a file not named .zst without -DestinationPath, and expands it with one' {
        $packed = Join-Path $TestDrive 'packed.bin'
        Compress-Zstd -Path $script:source -DestinationPath $packed
        Expand-Zstd -Path $packed -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdNoDestination,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
        $restored = Join-Path $TestDrive 'packed.restored'
        Expand-Zstd -Path $packed -DestinationPath $restored
        (Get-FileHash -LiteralPath $restored -Algorithm SHA256).Hash | Should -Be $script:sourceHash
    }

    It 'writes several files into a folder given as -DestinationPath, under their default names' {
        $inputs = Join-Path $TestDrive 'many'
        $packedFolder = Join-Path $TestDrive 'many-packed'
        $restoredFolder = Join-Path $TestDrive 'many-restored'
        $null = New-Item -ItemType Directory -Path $inputs, $packedFolder, $restoredFolder
        $digests = @{}
        foreach ($i in 1..3) {
            $bytes = New-Noise (1000 * $i) $i
            [System.IO.File]::WriteAllBytes((Join-Path $inputs "part$i.bin"), $bytes)
            $digests["part$i.bin"] = Get-Digest $bytes
        }
        Compress-Zstd -Path (Join-Path $inputs '*.bin') -DestinationPath $packedFolder
        @(Get-ChildItem -LiteralPath $packedFolder -Name | Sort-Object) | Should -Be @('part1.bin.zst', 'part2.bin.zst', 'part3.bin.zst')
        Expand-Zstd -Path (Join-Path $packedFolder '*.zst') -DestinationPath $restoredFolder
        foreach ($name in $digests.Keys) {
            Get-Digest ([System.IO.File]::ReadAllBytes((Join-Path $restoredFolder $name))) | Should -Be $digests[$name] -Because $name
        }
    }

    It 'gives a -DestinationPath that names one file to the first input only' {
        $inputs = Join-Path $TestDrive 'reuse'
        $null = New-Item -ItemType Directory -Path $inputs
        [System.IO.File]::WriteAllBytes((Join-Path $inputs 'a.txt'), (New-Text 50))
        [System.IO.File]::WriteAllBytes((Join-Path $inputs 'b.txt'), (New-Text 60))
        $single = Join-Path $TestDrive 'reuse.zst'
        Compress-Zstd -Path (Join-Path $inputs 'a.txt'), (Join-Path $inputs 'b.txt') -DestinationPath $single -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDestinationReused,$script:compressId"
        $failures[0].TargetObject | Should -Be (Join-Path $inputs 'b.txt')
        Get-Digest (Expand-Zstd -InputObject ([System.IO.File]::ReadAllBytes($single))) | Should -Be (Get-Digest (New-Text 50))
    }

    It 'reports a failed file and goes on with the rest' {
        $inputs = Join-Path $TestDrive 'goes-on'
        $null = New-Item -ItemType Directory -Path $inputs
        $good = Join-Path $inputs 'good.txt'
        [System.IO.File]::WriteAllBytes($good, (New-Text 80))
        $missing = Join-Path $inputs 'missing.txt'
        Compress-Zstd -Path $missing, $good, (Join-Path $inputs 'nothing*.txt') -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 2
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("ZstdInputNotFound,$script:compressId", "ZstdInputNotFound,$script:compressId")
        [System.IO.File]::Exists("$good.zst") | Should -BeTrue
    }

    It 'writes the FileInfo of each file written under -PassThru, and nothing without it or under -WhatIf' {
        $folder = Join-Path $TestDrive 'passthru'
        $null = New-Item -ItemType Directory -Path $folder
        $source = Join-Path $folder 'data.bin'
        [System.IO.File]::Copy($script:source, $source)
        Compress-Zstd -Path $source -WhatIf -PassThru | Should -BeNullOrEmpty
        $written = @(Compress-Zstd -Path $source -PassThru)
        $written.Count | Should -Be 1
        $written[0] | Should -BeOfType System.IO.FileInfo
        $written[0].FullName | Should -Be "$source.zst"
        $written[0].PSPath | Should -Not -BeNullOrEmpty
        Compress-Zstd -Path $source -Force | Should -BeNullOrEmpty
        $expanded = @(Expand-Zstd -Path "$source.zst" -DestinationPath (Join-Path $folder 'again.bin') -PassThru)
        $expanded.Count | Should -Be 1
        $expanded[0].FullName | Should -Be (Join-Path $folder 'again.bin')
        (Get-FileHash -LiteralPath $expanded[0].FullName -Algorithm SHA256).Hash | Should -Be $script:sourceHash
    }

    It 'writes nothing under -WhatIf' {
        $packed = Join-Path $TestDrive 'whatif.zst'
        Compress-Zstd -Path $script:source -DestinationPath $packed -WhatIf
        Test-Path -LiteralPath $packed | Should -BeFalse
        $real = Join-Path $TestDrive 'whatif.real.zst'
        Compress-Zstd -Path $script:source -DestinationPath $real
        $restored = Join-Path $TestDrive 'whatif.bin'
        Expand-Zstd -Path $real -DestinationPath $restored -WhatIf
        Test-Path -LiteralPath $restored | Should -BeFalse
    }

    It 'refuses an existing destination without -Force, and replaces it with -Force' {
        $packed = Join-Path $TestDrive 'exists.zst'
        [System.IO.File]::WriteAllText($packed, 'keep me')
        { Compress-Zstd -Path $script:source -DestinationPath $packed -ErrorAction Stop } |
            Should -Throw -ErrorId "ZstdDestinationExists,$script:compressId"
        [System.IO.File]::ReadAllText($packed) | Should -Be 'keep me'
        Compress-Zstd -Path $script:source -DestinationPath $packed -Force
        $restored = Join-Path $TestDrive 'exists.bin'
        [System.IO.File]::WriteAllText($restored, 'keep me too')
        { Expand-Zstd -Path $packed -DestinationPath $restored -ErrorAction Stop } |
            Should -Throw -ErrorId "ZstdDestinationExists,$script:expandId"
        [System.IO.File]::ReadAllText($restored) | Should -Be 'keep me too'
        Expand-Zstd -Path $packed -DestinationPath $restored -Force
        (Get-FileHash -LiteralPath $restored -Algorithm SHA256).Hash | Should -Be $script:sourceHash
    }

    It 'refuses a directory as input, naming the parameter that took it, and a destination that is a directory, even with -Force' {
        { Compress-Zstd -Path $TestDrive -DestinationPath (Join-Path $TestDrive 'dir.zst') -ErrorAction Stop } |
            Should -Throw -ErrorId "ZstdInputIsDirectory,$script:compressId"
        $cases = @(
            @{ Command = 'Compress-Zstd'; Id = $script:compressId; Parameter = 'Path'; Parameters = @{ Path = $TestDrive; DestinationPath = (Join-Path $TestDrive 'dir.zst') } }
            @{ Command = 'Compress-Zstd'; Id = $script:compressId; Parameter = 'LiteralPath'; Parameters = @{ LiteralPath = $TestDrive; DestinationPath = (Join-Path $TestDrive 'dir.zst') } }
            @{ Command = 'Compress-Zstd'; Id = $script:compressId; Parameter = 'DictionaryPath'; Parameters = @{ Path = $script:source; DestinationPath = (Join-Path $TestDrive 'dir.zst'); DictionaryPath = $TestDrive } }
            @{ Command = 'Expand-Zstd'; Id = $script:expandId; Parameter = 'Path'; Parameters = @{ Path = $TestDrive; DestinationPath = (Join-Path $TestDrive 'dir.bin') } }
            @{ Command = 'Expand-Zstd'; Id = $script:expandId; Parameter = 'LiteralPath'; Parameters = @{ LiteralPath = $TestDrive; DestinationPath = (Join-Path $TestDrive 'dir.bin') } }
            @{ Command = 'Expand-Zstd'; Id = $script:expandId; Parameter = 'DictionaryPath'; Parameters = @{ InputObject = (Compress-Zstd -InputObject (New-Text 10)); DictionaryPath = $TestDrive } }
        )
        foreach ($case in $cases) {
            $parameters = $case.Parameters
            $output = & $case.Command @parameters -ErrorVariable failures -ErrorAction SilentlyContinue
            $output | Should -BeNullOrEmpty -Because "$($case.Command) -$($case.Parameter)"
            $failures.Count | Should -Be 1 -Because "$($case.Command) -$($case.Parameter)"
            $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInputIsDirectory,$($case.Id)"
            "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
            $failures[0].Exception.Message | Should -Be "'$TestDrive' is a directory, and -$($case.Parameter) takes a file."
        }
        Test-Path -LiteralPath (Join-Path $TestDrive 'dir.zst') | Should -BeFalse
        Test-Path -LiteralPath (Join-Path $TestDrive 'dir.bin') | Should -BeFalse
        # The folder given receives source.bin.zst, which is itself a folder.
        $into = Join-Path $TestDrive 'into'
        $null = New-Item -ItemType Directory -Path (Join-Path $into 'source.bin.zst') -Force
        { Compress-Zstd -Path $script:source -DestinationPath $into -Force -ErrorAction Stop } |
            Should -Throw -ErrorId "ZstdDestinationIsDirectory,$script:compressId"
    }

    It 'refuses a missing input with an error record that names it, and writes nothing' {
        $missing = Join-Path $TestDrive 'missing.bin'
        $packed = Join-Path $TestDrive 'missing.zst'
        Compress-Zstd -Path $missing -DestinationPath $packed -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInputNotFound,$script:compressId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'ObjectNotFound'
        $failures[0].TargetObject | Should -Be $missing
        Test-Path -LiteralPath $packed | Should -BeFalse
        { Expand-Zstd -Path $missing -DestinationPath $packed -ErrorAction Stop } |
            Should -Throw -ErrorId "ZstdInputNotFound,$script:expandId"
    }

    It 'refuses input that is not zstd, is truncated, or is empty, and leaves no file behind' {
        $good = Join-Path $TestDrive 'good.zst'
        Compress-Zstd -Path $script:source -DestinationPath $good
        $bytes = [System.IO.File]::ReadAllBytes($good)
        $truncated = [byte[]]::new([int] [math]::Floor($bytes.Length / 2))
        [System.Array]::Copy($bytes, $truncated, $truncated.Length)
        $cases = [ordered]@{
            'not-zstd.zst'  = (New-Noise 4096 11)
            'truncated.zst' = $truncated
            'empty.zst'     = [byte[]]::new(0)
        }
        foreach ($name in $cases.Keys) {
            $bad = Join-Path $TestDrive $name
            [System.IO.File]::WriteAllBytes($bad, $cases[$name])
            $restored = Join-Path $TestDrive "$name.out"
            $before = @(Get-ChildItem -LiteralPath $TestDrive -Force).Count
            Expand-Zstd -Path $bad -DestinationPath $restored -ErrorVariable failures -ErrorAction SilentlyContinue
            $failures.Count | Should -Be 1 -Because $name
            $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInvalidData,$script:expandId" -Because $name
            "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidData' -Because $name
            Test-Path -LiteralPath $restored | Should -BeFalse -Because $name
            @(Get-ChildItem -LiteralPath $TestDrive -Force).Count | Should -Be $before -Because $name
        }
    }

    It 'takes -LiteralPath as written, wildcard characters included, under the aliases PSPath and LP too' {
        $folder = Join-Path $TestDrive 'literal'
        $null = New-Item -ItemType Directory -Path $folder
        $odd = Join-Path $folder 'data[1].bin'
        [System.IO.File]::WriteAllBytes($odd, (New-Noise 5000 31))
        $oddHash = (Get-FileHash -LiteralPath $odd -Algorithm SHA256).Hash
        Compress-Zstd -LiteralPath $odd
        Test-Path -LiteralPath "$odd.zst" | Should -BeTrue
        $back = Join-Path $folder 'back.bin'
        Expand-Zstd -LP "$odd.zst" -DestinationPath $back
        (Get-FileHash -LiteralPath $back -Algorithm SHA256).Hash | Should -Be $oddHash
        Compress-Zstd -PSPath $odd -DestinationPath (Join-Path $folder 'pspath.zst')
        Test-Path -LiteralPath (Join-Path $folder 'pspath.zst') | Should -BeTrue
        # Given to -Path, [1] is a wildcard for a name with 1 there, and no file has one.
        Compress-Zstd -Path $odd -DestinationPath (Join-Path $folder 'wild.zst') -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInputNotFound,$script:compressId"
    }

    It 'takes files piped from Get-ChildItem by their PSPath, with -DestinationPath, -Force and -PassThru' {
        $folder = Join-Path $TestDrive 'piped'
        $out = Join-Path $folder 'out'
        $null = New-Item -ItemType Directory -Path $folder, $out
        $hashes = @{}
        foreach ($i in 1..3) {
            $file = Join-Path $folder "f$i.bin"
            [System.IO.File]::WriteAllBytes($file, (New-Noise 2000 $i))
            $hashes["f$i.bin"] = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
        }
        $written = @(Get-ChildItem -LiteralPath $folder -Filter '*.bin' | Compress-Zstd -PassThru)
        $written.Count | Should -Be 3
        @($written | ForEach-Object Name | Sort-Object) | Should -Be @('f1.bin.zst', 'f2.bin.zst', 'f3.bin.zst')
        $expanded = @(Get-ChildItem -LiteralPath $folder -Filter '*.zst' | Expand-Zstd -DestinationPath $out -Force -PassThru)
        $expanded.Count | Should -Be 3
        foreach ($name in $hashes.Keys) {
            (Get-FileHash -LiteralPath (Join-Path $out $name) -Algorithm SHA256).Hash | Should -Be $hashes[$name] -Because $name
        }
        Get-Item -LiteralPath (Join-Path $folder 'f1.bin') | Compress-Zstd -DestinationPath (Join-Path $out 'one.zst')
        Test-Path -LiteralPath (Join-Path $out 'one.zst') | Should -BeTrue
    }

    It 'leaves -DestinationPath, -Force and -PassThru to the file sets, as the binder enforces' {
        $bytes = [byte[]] (1, 2, 3)
        { Compress-Zstd -InputObject $bytes -DestinationPath (Join-Path $TestDrive 'never.zst') } | Should -Throw -ErrorId "AmbiguousParameterSet,$script:compressId"
        { Expand-Zstd -InputObject $bytes -Force } | Should -Throw -ErrorId "AmbiguousParameterSet,$script:expandId"
        { Compress-Zstd -Path $script:source -LiteralPath $script:source } | Should -Throw -ErrorId "AmbiguousParameterSet,$script:compressId"
        Test-Path -LiteralPath (Join-Path $TestDrive 'never.zst') | Should -BeFalse
    }

    It 'refuses a frame that needs a dictionary, naming the id it needs, and leaves no file behind' {
        $needs = Join-Path $TestDrive 'needs.bin.zst'
        [System.IO.File]::WriteAllBytes($needs, (New-DictionaryFrame 0x12345678))
        Expand-Zstd -Path $needs -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDictionaryMismatch,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
        $failures[0].TargetObject | Should -Be $needs
        $failures[0].Exception.Message | Should -Be "Cannot expand '$needs': a frame in it needs dictionary 305419896, and no dictionary was given."
        Test-Path -LiteralPath (Join-Path $TestDrive 'needs.bin') | Should -BeFalse
    }

    It 'refuses a file it cannot open for reading' {
        $locked = Join-Path $TestDrive 'locked.txt'
        [System.IO.File]::WriteAllText($locked, 'locked')
        $handle = [System.IO.File]::Open($locked, 'Open', 'ReadWrite', 'None')
        try {
            { Compress-Zstd -Path $locked -DestinationPath (Join-Path $TestDrive 'locked.zst') -ErrorAction Stop } |
                Should -Throw -ErrorId "ZstdReadFailed,$script:compressId"
        } finally {
            $handle.Dispose()
        }
    }

    It 'refuses a destination folder that does not exist' {
        $packed = Join-Path $TestDrive 'no\such\folder\out.zst'
        { Compress-Zstd -Path $script:source -DestinationPath $packed -ErrorAction Stop } |
            Should -Throw -ErrorId "ZstdWriteFailed,$script:compressId"
    }

    It 'refuses a path on a drive this session does not have' {
        { Compress-Zstd -Path 'NoSuchDrive:\x.bin' -DestinationPath (Join-Path $TestDrive 'x.zst') -ErrorAction Stop } |
            Should -Throw -ErrorId "ZstdPathNotResolved,$script:compressId"
    }

    It 'reports both sizes on the verbose stream' {
        # The engine writes its own ShouldProcess message there too.
        $packed = Join-Path $TestDrive 'verbose.zst'
        $records = @(Compress-Zstd -Path $script:source -DestinationPath $packed -Verbose 4>&1 |
                Where-Object { $_ -is [System.Management.Automation.VerboseRecord] -and $_.Message -like 'Compressed *' })
        $records.Count | Should -Be 1
        $records[0].Message | Should -Match "\($($script:data.Length) bytes\) to '.+' \(\d+ bytes\) at level 3"
    }
}

Describe 'Compress-Zstd and Expand-Zstd on bytes' {
    BeforeAll {
        $script:text = New-Text 20000
        $script:noise = New-Noise 300000 3
    }

    It 'round-trips a byte[] given to -InputObject' {
        foreach ($data in $script:text, $script:noise) {
            $packed = Compress-Zstd -InputObject $data
            $packed.GetType().FullName | Should -Be 'System.Byte[]'
            $restored = Expand-Zstd -InputObject $packed
            $restored.GetType().FullName | Should -Be 'System.Byte[]'
            Get-Digest $restored | Should -Be (Get-Digest $data)
        }
        (Compress-Zstd -InputObject $script:text).Length | Should -BeLessThan $script:text.Length
    }

    It 'round-trips bytes piped one at a time and a byte[] piped whole' {
        $small = New-Noise 2000 5
        $packed = $small | Compress-Zstd
        Get-Digest (, $packed | Expand-Zstd) | Should -Be (Get-Digest $small)
        Get-Digest ($packed | Expand-Zstd) | Should -Be (Get-Digest $small)
        Get-Digest ($small | Compress-Zstd | Expand-Zstd) | Should -Be (Get-Digest $small)
    }

    It 'round-trips empty input' {
        $empty = [byte[]]::new(0)
        $packed = Compress-Zstd -InputObject $empty
        $packed.Length | Should -BeGreaterThan 0
        $restored = Expand-Zstd -InputObject $packed
        $restored.GetType().FullName | Should -Be 'System.Byte[]'
        $restored.Length | Should -Be 0
        (Expand-Zstd -InputObject (, $empty | Compress-Zstd)).Length | Should -Be 0
    }

    It 'writes nothing, output or error, for a pipeline that delivers no records' {
        $packed = & { } | Compress-Zstd -ErrorVariable failures
        $packed | Should -BeNullOrEmpty
        $failures.Count | Should -Be 0
        $expanded = & { } | Expand-Zstd -ErrorVariable failures
        $expanded | Should -BeNullOrEmpty
        $failures.Count | Should -Be 0
    }

    It 'refuses a $null record with ZstdNoInput and goes on with the records around it' {
        $first = New-Text 300
        $second = New-Noise 400 11
        $message = 'No bytes were given: -InputObject takes a byte[], from the pipeline or by name.'
        $packed = $first, $null, $second | Compress-Zstd -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdNoInput,$script:compressId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
        $failures[0].Exception.Message | Should -Be $message
        Get-Digest (Expand-Zstd -InputObject $packed) | Should -Be (Get-Digest (Join-Bytes $first $second))
        $frames = (Compress-Zstd -InputObject $first), (Compress-Zstd -InputObject $second)
        $restored = $frames[0], $null, $frames[1] | Expand-Zstd -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdNoInput,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
        $failures[0].Exception.Message | Should -Be $message
        Get-Digest $restored | Should -Be (Get-Digest (Join-Bytes $first $second))
    }

    It 'refuses -InputObject $null with ZstdNoInput and writes nothing' {
        $output = Compress-Zstd -InputObject $null -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("ZstdNoInput,$script:compressId")
        $output = Expand-Zstd -InputObject $null -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("ZstdNoInput,$script:expandId")
    }

    It 'refuses the bytes set chosen with no -InputObject with ZstdNoInput and writes nothing' {
        # -Dictionary alone leaves the InputObjectDictionary set as the only
        # one whose mandatory parameters are all bound.
        $dictionary = [System.Text.Encoding]::ASCII.GetBytes('raw dictionary content ' * 20)
        $output = Compress-Zstd -Dictionary $dictionary -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("ZstdNoInput,$script:compressId")
        $output = Expand-Zstd -Dictionary $dictionary -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("ZstdNoInput,$script:expandId")
    }

    It 'binds a byte[] another command wrote, wrapped in a PSObject, as the array itself' {
        # Converting 32 MiB element by element takes seconds in either host;
        # the array itself binds at once.
        $big = New-Noise (32MB) 23
        $packed = Compress-Zstd -InputObject $big -Level 1
        $wrapped = Write-Output -NoEnumerate $packed
        $wrapped -is [psobject] | Should -BeTrue
        $clock = [System.Diagnostics.Stopwatch]::StartNew()
        $back = Expand-Zstd -InputObject $wrapped
        $clock.Stop()
        Get-Digest $back | Should -Be (Get-Digest $big)
        $clock.Elapsed.TotalSeconds | Should -BeLessThan 3
    }

    It 'writes one zstd frame with a checksum, whose header records the content size, for a single byte[]' {
        foreach ($packed in (Compress-Zstd -InputObject $script:noise), (, $script:noise | Compress-Zstd)) {
            [System.BitConverter]::ToString($packed, 0, 4) | Should -Be '28-B5-2F-FD'
            ($packed[4] -band 0x04) | Should -Be 0x04
            ((($packed[4] -shr 6) -ne 0) -or (($packed[4] -band 0x20) -ne 0)) | Should -BeTrue
        }
    }

    It 'streams several records into one frame with a checksum and no content size' {
        # One byte per record, as the pipeline enumerates an array, and
        # two byte[] records.
        $small = New-Noise 5000 17
        $second = New-Text 200
        foreach ($case in @(
                @{ Packed = ($small | Compress-Zstd); Expected = $small },
                @{ Packed = ($small, $second | Compress-Zstd -Level 1); Expected = (Join-Bytes $small $second) })) {
            $packed = $case.Packed
            [System.BitConverter]::ToString($packed, 0, 4) | Should -Be '28-B5-2F-FD'
            ($packed[4] -band 0x04) | Should -Be 0x04
            ($packed[4] -shr 6) | Should -Be 0
            ($packed[4] -band 0x20) | Should -Be 0
            Get-Digest (Expand-Zstd -InputObject $packed) | Should -Be (Get-Digest $case.Expected)
        }
    }

    It 'expands records that split frames anywhere' {
        $first = Compress-Zstd -InputObject (New-Text 3000)
        $second = Compress-Zstd -InputObject $script:noise
        $joined = Join-Bytes $first $second
        $cut = [int] ($first.Length / 2)
        $head = [byte[]]::new($cut)
        $tail = [byte[]]::new($joined.Length - $cut)
        [System.Array]::Copy($joined, 0, $head, 0, $cut)
        [System.Array]::Copy($joined, $cut, $tail, 0, $tail.Length)
        $restored = $head, $tail | Expand-Zstd
        Get-Digest $restored | Should -Be (Get-Digest (Join-Bytes (New-Text 3000) $script:noise))
    }

    It 'expands a first record whose frames record their size whole, and goes on with the records after it' {
        $text = New-Text 3000
        $first = Compress-Zstd -InputObject $text
        $second = $script:noise, $text | Compress-Zstd
        $records = @(Expand-Zstd -InputObject $first -Verbose 4>&1)
        $output = @($records | Where-Object { $_ -is [byte[]] })
        $output.Count | Should -Be 1
        Get-Digest $output[0] | Should -Be (Get-Digest $text)
        @($records | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message }) | Should -Be @("Expanded $($first.Length) bytes to $($text.Length) bytes.")
        $records = @($first, $second | Expand-Zstd -Verbose 4>&1)
        $output = @($records | Where-Object { $_ -is [byte[]] })
        $output.Count | Should -Be 1
        Get-Digest $output[0] | Should -Be (Get-Digest (Join-Bytes $text $script:noise $text))
        @($records | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message }) | Should -Be @("Expanded $($first.Length + $second.Length) bytes to $(2 * $text.Length + $script:noise.Length) bytes.")
        Get-Digest ($first, [byte[]]::new(0) | Expand-Zstd) | Should -Be (Get-Digest $text)
    }

    It 'refuses bytes whose frame records a content size other than it holds' {
        # The magic number; a single-segment header with a one-byte content
        # size of 10, then of 3; one last raw block of the 5 bytes of hello.
        $cases = @(
            @{ Frame = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x0A, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F); Reason = 'a frame records a content size of 10 bytes and holds 5' }
            @{ Frame = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x03, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F); Reason = 'Destination buffer is too small' }
        )
        foreach ($case in $cases) {
            $output = Expand-Zstd -InputObject $case.Frame -ErrorVariable failures -ErrorAction SilentlyContinue
            $output | Should -BeNullOrEmpty -Because $case.Reason
            $failures.Count | Should -Be 1 -Because $case.Reason
            $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInvalidData,$script:expandId"
            "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidData'
            $failures[0].Exception.Message | Should -Be "Cannot expand the input: it is not valid zstd data ($($case.Reason))."
        }
    }

    It 'refuses a frame that ends with an empty block and records more than it holds, whole, piped and from a file' {
        # The magic number; a header that asks for a window of 1 MiB and
        # records a content size of 6 in four bytes; a raw block of the 5
        # bytes of hello; and an empty last block, at which libzstd does not
        # check the size.
        $frame = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x80, 0x50, 0x06, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x01, 0x00, 0x00)
        $reason = 'it is not valid zstd data (a frame records a content size of 6 bytes and holds 5).'
        $output = Expand-Zstd -InputObject $frame -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInvalidData,$script:expandId"
        $failures[0].Exception.Message | Should -Be "Cannot expand the input: $reason"
        # Piped from a variable, the frame arrives a byte at a time and is
        # fed to libzstd a chunk at a time.
        $output = $frame | Expand-Zstd -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].Exception.Message | Should -Be "Cannot expand the input: $reason"
        $packed = Join-Path $TestDrive 'empty-block.zst'
        [System.IO.File]::WriteAllBytes($packed, $frame)
        $restored = Join-Path $TestDrive 'empty-block.bin'
        Expand-Zstd -Path $packed -DestinationPath $restored -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInvalidData,$script:expandId"
        $failures[0].Exception.Message | Should -Be "Cannot expand '$packed': $reason"
        Test-Path -LiteralPath $restored | Should -BeFalse
    }

    It 'expands a chunk at a time a record whose frames record more than a byte[] holds' {
        # The magic number; a header that asks for a window of 1 MiB and
        # records a content size of 3000000000 bytes in four bytes; one last
        # raw block of 5 bytes, which is all the frame holds.
        $frame = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x80, 0x50, 0x00, 0x5E, 0xD0, 0xB2, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F)
        $records = @(Expand-Zstd -InputObject $frame -Verbose -ErrorVariable failures -ErrorAction SilentlyContinue 4>&1)
        @($records | Where-Object { $_ -is [byte[]] }).Count | Should -Be 0
        $verbose = @($records | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message })
        @($verbose | Where-Object { $_ -like 'Expanding the input a chunk at a time: a byte`[`] of 3000000000 bytes could not be made (*).' }).Count | Should -Be 1 -Because ($verbose -join ' | ')
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInvalidData,$script:expandId"
    }

    It 'refuses levels 0 and 23' {
        foreach ($level in 0, 23) {
            { Compress-Zstd -InputObject $script:noise -Level $level } |
                Should -Throw -ErrorId "ParameterArgumentValidationError,$script:compressId"
        }
    }

    It 'defaults to level 3, and round-trips at every level from 1 to 22' {
        $sample = New-Text 300
        Get-Digest (Compress-Zstd -InputObject $sample) | Should -Be (Get-Digest (Compress-Zstd -InputObject $sample -Level 3))
        foreach ($level in 1..22) {
            Get-Digest (Expand-Zstd -InputObject (Compress-Zstd -InputObject $sample -Level $level)) |
                Should -Be (Get-Digest $sample) -Because "level $level"
        }
    }

    It 'compresses text smaller at level 19 than at level 1' {
        (Compress-Zstd -InputObject $script:text -Level 19).Length |
            Should -BeLessThan (Compress-Zstd -InputObject $script:text -Level 1).Length
    }

    It 'round-trips on worker threads, and refuses -Threads outside 0 to 256' {
        $data = Join-Bytes $script:text $script:noise $script:text
        Get-Digest (Expand-Zstd -InputObject (Compress-Zstd -InputObject $data -Threads 4 -Level 1)) |
            Should -Be (Get-Digest $data)
        foreach ($threads in -1, 257) {
            { Compress-Zstd -InputObject $script:noise -Threads $threads } |
                Should -Throw -ErrorId "ParameterArgumentValidationError,$script:compressId"
        }
    }

    It 'expands a frame built by hand from RFC 8878' {
        # The magic number; a single-segment header with a one-byte content
        # size of 5; one last raw block of 5 bytes; the bytes.
        $frame = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x05, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F)
        [System.Text.Encoding]::ASCII.GetString((Expand-Zstd -InputObject $frame)) | Should -Be 'hello'
    }

    It 'expands concatenated frames in order' {
        $first = Compress-Zstd -InputObject ([System.Text.Encoding]::ASCII.GetBytes('hel'))
        $second = Compress-Zstd -InputObject ([System.Text.Encoding]::ASCII.GetBytes('lo'))
        [System.Text.Encoding]::ASCII.GetString((Expand-Zstd -InputObject (Join-Bytes $first $second))) | Should -Be 'hello'
    }

    It 'refuses corrupt, truncated and empty input with an error record, and writes nothing' {
        $good = Compress-Zstd -InputObject $script:text
        $truncated = [byte[]]::new($good.Length - 10)
        [System.Array]::Copy($good, $truncated, $truncated.Length)
        $flipped = [byte[]] $good.Clone()
        $flipped[-1] = $flipped[-1] -bxor 0xFF
        $cases = [ordered]@{
            'not zstd'       = (New-Noise 1000 9)
            'truncated'      = $truncated
            'checksum'       = $flipped
            'trailing bytes' = (Join-Bytes $good (New-Noise 64 5))
            'empty'          = [byte[]]::new(0)
        }
        foreach ($name in $cases.Keys) {
            $output = Expand-Zstd -InputObject $cases[$name] -ErrorVariable failures -ErrorAction SilentlyContinue
            $output | Should -BeNullOrEmpty -Because $name
            $failures.Count | Should -Be 1 -Because $name
            $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInvalidData,$script:expandId" -Because $name
            "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidData' -Because $name
        }
    }

    It 'refuses a frame that needs a dictionary with an error record naming the id it needs' {
        $message = 'Cannot expand the input: a frame in it needs dictionary 305419896, and no dictionary was given.'
        $output = Expand-Zstd -InputObject (New-DictionaryFrame 0x12345678) -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDictionaryMismatch,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
        $failures[0].Exception.Message | Should -Be $message
        # Piped from a variable, the frame arrives a byte at a time and its
        # header across ten records.
        $frame = New-DictionaryFrame 0x12345678
        $piped = $frame | Expand-Zstd -ErrorVariable failures -ErrorAction SilentlyContinue
        $piped | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].Exception.Message | Should -Be $message
    }

    It 'takes the records the binder makes a byte[] of, and adds nothing to the binder''s refusals' {
        # -InputObject is declared byte[], so a number from 0 to 255 binds
        # as a byte[] of one, and the binder refuses text or a larger
        # number before the cmdlet sees it, as input it cannot bind.
        $packed = 1, 2, 3 | Compress-Zstd
        Get-Digest (Expand-Zstd -InputObject $packed) | Should -Be (Get-Digest ([byte[]] (1, 2, 3)))
        $output = 'text' | Expand-Zstd -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("InputObjectNotBound,$script:expandId")
        $output = 256 | Compress-Zstd -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("InputObjectNotBound,$script:compressId")
        $output = 'text', ([byte[]] (1, 2, 3)) | Compress-Zstd -ErrorVariable failures -ErrorAction SilentlyContinue
        @($failures | ForEach-Object { $_.FullyQualifiedErrorId }) | Should -Be @("InputObjectNotBound,$script:compressId")
        Get-Digest (Expand-Zstd -InputObject $output) | Should -Be (Get-Digest ([byte[]] (1, 2, 3)))
    }

    It 'reports both sizes on the verbose stream' {
        $records = @(Compress-Zstd -InputObject $script:noise -Verbose 4>&1)
        $verbose = @($records | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] -and $_.Message -like 'Compressed *' })
        $verbose.Count | Should -Be 1
        $verbose[0].Message | Should -Match "^Compressed $($script:noise.Length) bytes to \d+ bytes at level 3"
    }
}

Describe 'New-ZstdDictionary' {
    BeforeAll {
        $script:newId = 'Pwrs.Modules.Zstd.NewZstdDictionaryCommand'
        # Forty samples shaped like small documents, alike in their keys
        # and different in their values, as a dictionary's samples are
        # meant to be.
        $script:samples = Join-Path $TestDrive 'samples'
        $null = New-Item -ItemType Directory -Path $script:samples
        for ($i = 0; $i -lt 40; $i++) {
            $n = $i * 7919 + 104729
            $record = '{{"id":{0},"user":"user-{1}","region":"{2}","status":"{3}","items":[{4},{5},{6}],"note":"order {0} left warehouse {7} for region {8}"}}' -f $n, ($n % 1000), ('eu-west', 'us-east', 'ap-south')[$n % 3], ('open', 'paid', 'shipped', 'closed')[$n % 4], ($n % 97), ($n % 89), ($n % 83), ($n % 17), ($n % 5)
            [System.IO.File]::WriteAllText((Join-Path $script:samples "sample$i.json"), ($record + "`n") * 4)
        }
        $script:all = Join-Path $script:samples '*.json'
        # zstd's dictionary magic number, 0xEC30A437, as the first four
        # bytes of a dictionary hold it.
        $script:magic = '37-A4-30-EC'
    }

    It 'trains a dictionary in zstd''s format from sample files, and writes nothing to the pipeline' {
        $dictionary = Join-Path $TestDrive 'plain.dict'
        New-ZstdDictionary -Path $script:all -DestinationPath $dictionary | Should -BeNullOrEmpty
        $bytes = [System.IO.File]::ReadAllBytes($dictionary)
        [System.BitConverter]::ToString($bytes, 0, 4) | Should -Be $script:magic
        [System.BitConverter]::ToUInt32($bytes, 4) | Should -Not -Be 0
        $bytes.Length | Should -BeLessOrEqual 112640
    }

    It 'binds the samples and the destination by position, and keeps to -MaxSize' {
        Push-Location -LiteralPath $TestDrive
        try {
            New-ZstdDictionary 'samples\*.json' 'small.dict' -MaxSize 1024
        } finally {
            Pop-Location
        }
        $bytes = [System.IO.File]::ReadAllBytes((Join-Path $TestDrive 'small.dict'))
        [System.BitConverter]::ToString($bytes, 0, 4) | Should -Be $script:magic
        $bytes.Length | Should -BeLessOrEqual 1024
    }

    It 'refuses -MaxSize below 256' {
        foreach ($size in 255, 0, -1) {
            { New-ZstdDictionary -Path $script:all -DestinationPath (Join-Path $TestDrive 'size.dict') -MaxSize $size } |
                Should -Throw -ErrorId "ParameterArgumentValidationError,$script:newId" -Because "-MaxSize $size"
        }
        Test-Path -LiteralPath (Join-Path $TestDrive 'size.dict') | Should -BeFalse
    }

    It 'writes the FileInfo of the dictionary under -PassThru' {
        $dictionary = Join-Path $TestDrive 'passed.dict'
        $written = @(New-ZstdDictionary -Path $script:all -DestinationPath $dictionary -PassThru)
        $written.Count | Should -Be 1
        $written[0] | Should -BeOfType System.IO.FileInfo
        $written[0].FullName | Should -Be $dictionary
        $written[0].PSPath | Should -Not -BeNullOrEmpty
    }

    It 'writes nothing under -WhatIf' {
        $dictionary = Join-Path $TestDrive 'whatif.dict'
        New-ZstdDictionary -Path $script:all -DestinationPath $dictionary -WhatIf -PassThru | Should -BeNullOrEmpty
        Test-Path -LiteralPath $dictionary | Should -BeFalse
    }

    It 'refuses an existing destination without -Force, and replaces it with -Force' {
        $dictionary = Join-Path $TestDrive 'exists.dict'
        [System.IO.File]::WriteAllText($dictionary, 'keep me')
        New-ZstdDictionary -Path $script:all -DestinationPath $dictionary -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDestinationExists,$script:newId"
        [System.IO.File]::ReadAllText($dictionary) | Should -Be 'keep me'
        New-ZstdDictionary -Path $script:all -DestinationPath $dictionary -Force
        [System.BitConverter]::ToString([System.IO.File]::ReadAllBytes($dictionary), 0, 4) | Should -Be $script:magic
    }

    It 'refuses a folder as the destination, even with -Force' {
        New-ZstdDictionary -Path $script:all -DestinationPath $script:samples -Force -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDestinationIsDirectory,$script:newId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
    }

    It 'reports a sample it cannot read, and trains on the rest' {
        $dictionary = Join-Path $TestDrive 'rest.dict'
        $missing = Join-Path $script:samples 'missing.json'
        New-ZstdDictionary -Path $script:all, $missing -DestinationPath $dictionary -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInputNotFound,$script:newId"
        $failures[0].TargetObject | Should -Be $missing
        [System.BitConverter]::ToString([System.IO.File]::ReadAllBytes($dictionary), 0, 4) | Should -Be $script:magic
    }

    It 'reports a folder given as a sample, naming the parameter that took it, and trains on the rest' {
        $files = @(Get-ChildItem -Path $script:all | ForEach-Object { $_.FullName })
        foreach ($parameter in 'Path', 'LiteralPath') {
            $dictionary = Join-Path $TestDrive "with-folder-$parameter.dict"
            $samples = @{ $parameter = ($files + $script:samples) }
            New-ZstdDictionary @samples -DestinationPath $dictionary -ErrorVariable failures -ErrorAction SilentlyContinue
            $failures.Count | Should -Be 1 -Because "-$parameter"
            $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInputIsDirectory,$script:newId"
            $failures[0].Exception.Message | Should -Be "'$($script:samples)' is a directory, and -$parameter takes a file."
            [System.BitConverter]::ToString([System.IO.File]::ReadAllBytes($dictionary), 0, 4) | Should -Be $script:magic
        }
    }

    It 'refuses to train from fewer than 7 samples, and leaves no file behind' {
        $dictionary = Join-Path $TestDrive 'few.dict'
        $before = @(Get-ChildItem -LiteralPath $TestDrive -Force).Count
        New-ZstdDictionary -Path (Join-Path $script:samples 'sample[0-5].json') -DestinationPath $dictionary -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDictionaryTrainingFailed,$script:newId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidOperation'
        $failures[0].Exception.Message | Should -BeLike "Cannot train a dictionary from 6 samples of * bytes in all: libzstd reported 'Src size is incorrect'. Its trainer needs at least 7 samples*"
        @(Get-ChildItem -LiteralPath $TestDrive -Force).Count | Should -Be $before
    }

    It 'reports the samples and the dictionary''s id on the verbose stream' {
        $dictionary = Join-Path $TestDrive 'verbose.dict'
        $records = @(New-ZstdDictionary -Path $script:all -DestinationPath $dictionary -Verbose 4>&1 |
                Where-Object { $_ -is [System.Management.Automation.VerboseRecord] -and $_.Message -notlike 'Performing the operation*' })
        $bytes = [System.IO.File]::ReadAllBytes($dictionary)
        $id = [System.BitConverter]::ToUInt32($bytes, 4)
        @($records | ForEach-Object Message) | Should -Be @(
            "Training a dictionary of at most 112640 bytes from 40 samples of $((Get-ChildItem -Path $script:all | Measure-Object -Property Length -Sum).Sum) bytes in all."
            "Wrote dictionary $id, $($bytes.Length) bytes, to '$dictionary'."
        )
    }

    It 'names the dictionary it trained when Expand-Zstd meets a frame that needs it' {
        $dictionary = Join-Path $TestDrive 'named.dict'
        New-ZstdDictionary -Path $script:all -DestinationPath $dictionary
        $id = [System.BitConverter]::ToUInt32([System.IO.File]::ReadAllBytes($dictionary), 4)
        Expand-Zstd -InputObject (New-DictionaryFrame $id) -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDictionaryMismatch,$script:expandId"
        $failures[0].Exception.Message | Should -Be "Cannot expand the input: a frame in it needs dictionary $id, and no dictionary was given."
    }
}

Describe 'Compress-Zstd and Expand-Zstd with a dictionary' {
    BeforeAll {
        # Documents alike in their keys and different in their values: forty
        # samples to train from, forty others for a second dictionary, and
        # one more document to compress.
        function New-Document([int] $N) {
            '{{"id":{0},"user":"user-{1}","region":"{2}","status":"{3}","items":[{4},{5},{6}],"note":"order {0} left warehouse {7} for region {8}"}}' -f $N, ($N % 1000), ('eu-west', 'us-east', 'ap-south')[$N % 3], ('open', 'paid', 'shipped', 'closed')[$N % 4], ($N % 97), ($N % 89), ($N % 83), ($N % 17), ($N % 5)
        }
        foreach ($set in 'first', 'second') {
            $folder = Join-Path $TestDrive "samples-$set"
            $null = New-Item -ItemType Directory -Path $folder
            $seed = if ($set -eq 'first') { 104729 } else { 7 }
            for ($i = 0; $i -lt 40; $i++) {
                $n = $i * 7919 + $seed
                $line = if ($set -eq 'first') { New-Document $n } else { "entry $n with value $($n % 991) and label L$($n % 13)" }
                [System.IO.File]::WriteAllText((Join-Path $folder "sample$i.json"), ($line + "`n") * 4)
            }
        }
        $script:dictFile = Join-Path $TestDrive 'first.dict'
        $script:otherFile = Join-Path $TestDrive 'second.dict'
        New-ZstdDictionary -Path (Join-Path $TestDrive 'samples-first\*.json') -DestinationPath $script:dictFile
        New-ZstdDictionary -Path (Join-Path $TestDrive 'samples-second\*.json') -DestinationPath $script:otherFile
        $script:dictBytes = [System.IO.File]::ReadAllBytes($script:dictFile)
        $script:dictId = [System.BitConverter]::ToUInt32($script:dictBytes, 4)
        $script:otherId = [System.BitConverter]::ToUInt32([System.IO.File]::ReadAllBytes($script:otherFile), 4)
        $script:doc = [System.Text.Encoding]::UTF8.GetBytes(((New-Document 424242) + "`n") * 4)
        $script:docFile = Join-Path $TestDrive 'doc.json'
        [System.IO.File]::WriteAllBytes($script:docFile, $script:doc)
        $script:raw = [System.Text.Encoding]::UTF8.GetBytes(((New-Document 424242) + "`n") * 8)

        # The dictionary id a frame's header records, or 0 for none
        # (RFC 8878: Dictionary_ID_flag in bits 1-0 of the frame header
        # descriptor, after the window descriptor unless single-segment).
        function Get-FrameDictionaryId([byte[]] $Frame) {
            $descriptor = $Frame[4]
            $size = @(0, 1, 2, 4)[$descriptor -band 3]
            $at = if ($descriptor -band 0x20) { 5 } else { 6 }
            $id = [uint32] 0
            for ($i = $size - 1; $i -ge 0; $i--) { $id = $id * 256 + $Frame[$at + $i] }
            $id
        }
    }

    It 'round-trips a file with -DictionaryPath, whose frame names the dictionary' {
        $packed = Join-Path $TestDrive 'doc.json.zst'
        $plain = Join-Path $TestDrive 'doc.plain.zst'
        Compress-Zstd -Path $script:docFile -DictionaryPath $script:dictFile
        Compress-Zstd -Path $script:docFile -DestinationPath $plain
        $frame = [System.IO.File]::ReadAllBytes($packed)
        Get-FrameDictionaryId $frame | Should -Be $script:dictId
        $frame.Length | Should -BeLessThan (Get-Item -LiteralPath $plain).Length
        $back = Join-Path $TestDrive 'doc.back.json'
        Expand-Zstd -Path $packed -DestinationPath $back -DictionaryPath $script:dictFile
        (Get-FileHash -LiteralPath $back -Algorithm SHA256).Hash | Should -Be (Get-FileHash -LiteralPath $script:docFile -Algorithm SHA256).Hash
    }

    It 'round-trips bytes with -Dictionary, given by name and piped' {
        $packed = Compress-Zstd -InputObject $script:doc -Dictionary $script:dictBytes
        Get-FrameDictionaryId $packed | Should -Be $script:dictId
        Get-Digest (Expand-Zstd -InputObject $packed -Dictionary $script:dictBytes) | Should -Be (Get-Digest $script:doc)
        $piped = , $script:doc | Compress-Zstd -Dictionary $script:dictBytes
        Get-Digest (, $piped | Expand-Zstd -Dictionary $script:dictBytes) | Should -Be (Get-Digest $script:doc)
    }

    It 'refuses a frame without its dictionary or with another, naming both' {
        $packed = Compress-Zstd -InputObject $script:doc -Dictionary $script:dictBytes
        $cases = @(
            @{ Given = @{}; Told = 'no dictionary was given' }
            @{ Given = @{ DictionaryPath = $script:otherFile }; Told = "dictionary $($script:otherId) was given" }
            @{ Given = @{ Dictionary = $script:raw }; Told = 'the dictionary given is raw content, which carries no id' }
        )
        foreach ($case in $cases) {
            $given = $case.Given
            $output = Expand-Zstd -InputObject $packed @given -ErrorVariable failures -ErrorAction SilentlyContinue
            $output | Should -BeNullOrEmpty -Because $case.Told
            $failures.Count | Should -Be 1 -Because $case.Told
            $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDictionaryMismatch,$script:expandId"
            "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidArgument'
            $failures[0].Exception.Message | Should -Be "Cannot expand the input: a frame in it needs dictionary $($script:dictId), and $($case.Told)."
        }
    }

    It 'uses bytes not in zstd''s dictionary format as raw content' {
        $packed = Compress-Zstd -InputObject $script:doc -Dictionary $script:raw
        Get-FrameDictionaryId $packed | Should -Be 0
        Get-Digest (Expand-Zstd -InputObject $packed -Dictionary $script:raw) | Should -Be (Get-Digest $script:doc)
        Expand-Zstd -InputObject $packed -ErrorVariable failures -ErrorAction SilentlyContinue | Should -BeNullOrEmpty
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInvalidData,$script:expandId"
    }

    It 'takes -Dictionary or -DictionaryPath, not both, as the binder enforces' {
        { Compress-Zstd -Path $script:docFile -Dictionary $script:dictBytes -DictionaryPath $script:dictFile } | Should -Throw -ErrorId "AmbiguousParameterSet,$script:compressId"
        { Expand-Zstd -InputObject $script:doc -Dictionary $script:dictBytes -DictionaryPath $script:dictFile } | Should -Throw -ErrorId "AmbiguousParameterSet,$script:expandId"
    }

    It 'refuses a dictionary whose tables libzstd refuses, before any input is read' {
        $broken = [byte[]] $script:dictBytes.Clone()
        for ($i = 8; $i -lt 200; $i++) { $broken[$i] = 0xFF }
        $brokenFile = Join-Path $TestDrive 'broken.dict'
        [System.IO.File]::WriteAllBytes($brokenFile, $broken)
        $out = Join-Path $TestDrive 'broken-out.zst'
        Compress-Zstd -Path $script:docFile -DestinationPath $out -DictionaryPath $brokenFile -PassThru -ErrorVariable failures -ErrorAction SilentlyContinue | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDictionaryInvalid,$script:compressId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'InvalidData'
        $failures[0].Exception.Message | Should -Be "Cannot use '$brokenFile' as a dictionary: it begins with zstd's dictionary magic number, and libzstd refused its tables (Dictionary is corrupted)."
        Test-Path -LiteralPath $out | Should -BeFalse
        Expand-Zstd -InputObject $script:doc -Dictionary $broken -ErrorVariable failures -ErrorAction SilentlyContinue | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdDictionaryInvalid,$script:expandId"
        $failures[0].Exception.Message | Should -BeLike 'Cannot use the bytes given to -Dictionary as a dictionary:*'
    }

    It 'reports a dictionary file that is not there as a missing input' {
        $missing = Join-Path $TestDrive 'no.dict'
        Compress-Zstd -Path $script:docFile -DestinationPath (Join-Path $TestDrive 'never.zst') -DictionaryPath $missing -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdInputNotFound,$script:compressId"
        $failures[0].TargetObject | Should -Be $missing
    }

    It 'trains from sample files piped to New-ZstdDictionary' {
        $piped = Join-Path $TestDrive 'piped.dict'
        $written = @(Get-ChildItem -LiteralPath (Join-Path $TestDrive 'samples-first') -Filter '*.json' | New-ZstdDictionary -DestinationPath $piped -PassThru)
        $written.Count | Should -Be 1
        $written[0].FullName | Should -Be $piped
        [System.BitConverter]::ToString([System.IO.File]::ReadAllBytes($piped), 0, 4) | Should -Be '37-A4-30-EC'
    }
}

Describe 'The memory budget, -MaxMemory' {
    BeforeAll {
        # A frame built by hand from RFC 8878 whose header asks for a
        # window of 2^$WindowLog bytes, with one last raw block holding
        # $Data, of 256 bytes to 64 KiB. With -Sized the header records the
        # content size, in two bytes; without it, none.
        function New-WideFrame([int] $WindowLog, [byte[]] $Data, [switch] $Sized) {
            $block = ($Data.Length -shl 3) -bor 1
            $blockHeader = [byte[]] (($block -band 0xFF), (($block -shr 8) -band 0xFF), (($block -shr 16) -band 0xFF))
            $window = [byte] (($WindowLog - 10) -shl 3)
            if ($Sized) {
                $size = $Data.Length - 256
                $header = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x40, $window, ($size -band 0xFF), (($size -shr 8) -band 0xFF))
            } else {
                $header = [byte[]] (0x28, 0xB5, 0x2F, 0xFD, 0x00, $window)
            }
            return , (Join-Bytes $header $blockHeader $Data)
        }

        # The verbose messages of a compression fitted to -MaxMemory, from
        # the records a command wrote with -Verbose 4>&1, as an array even
        # when there is one.
        function Get-Fitted([object[]] $Records) {
            return , @($Records | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] -and $_.Message -like 'Within -MaxMemory*' } | ForEach-Object { $_.Message })
        }

        $script:small = New-Text 500
        $script:wide = New-WideFrame 28 $script:small
        $script:wideSized = New-WideFrame 28 $script:small -Sized
        $script:text = New-Text 20000
        $script:source = Join-Path $TestDrive 'budget-source.bin'
        [System.IO.File]::WriteAllBytes($script:source, (Join-Bytes (New-Noise 1048576 11) $script:text))
        $script:sourceHash = (Get-FileHash -LiteralPath $script:source -Algorithm SHA256).Hash
    }

    It 'takes -MaxMemory of 1MB or more on Compress-Zstd and Expand-Zstd, as the binder enforces, and not on New-ZstdDictionary' {
        { Compress-Zstd -InputObject $script:small -MaxMemory 1048575 } | Should -Throw -ErrorId "ParameterArgumentValidationError,$script:compressId"
        { Expand-Zstd -InputObject $script:wide -MaxMemory 1048575 } | Should -Throw -ErrorId "ParameterArgumentValidationError,$script:expandId"
        (Get-Command New-ZstdDictionary).Parameters.ContainsKey('MaxMemory') | Should -BeFalse
    }

    It 'refuses bytes whose frame asks for a window over 128 MiB without -MaxMemory, and expands them within a budget that holds it' {
        $output = Expand-Zstd -InputObject $script:wide -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdWindowTooLarge,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'LimitsExceeded'
        $pattern = '^Cannot expand the input: a frame in it has a window of 268435456 bytes, more than the 134217728 bytes libzstd expands without -MaxMemory; -MaxMemory of (\d+) bytes or more lets it, with room besides for its result, which is held in memory twice\.$'
        ($failures[0].Exception.Message -match $pattern) | Should -BeTrue -Because $failures[0].Exception.Message
        $needed = [long] $Matches[1]
        Get-Digest (Expand-Zstd -InputObject $script:wide -MaxMemory ($needed + 2 * $script:small.Length)) | Should -Be (Get-Digest $script:small)
        $output = Expand-Zstd -InputObject $script:wide -MaxMemory 64MB -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdWindowTooLarge,$script:expandId"
        $failures[0].Exception.Message | Should -Match '^Cannot expand the input: a frame in it has a window of 268435456 bytes, which takes \d+ bytes to expand, more than -MaxMemory of 67108864 bytes\.$'
    }

    It 'expands bytes whose frames record their size straight into the byte[] written, whatever the window, counting that result once' {
        Get-Digest (Expand-Zstd -InputObject $script:wideSized) | Should -Be (Get-Digest $script:small)
        $output = Expand-Zstd -InputObject $script:wideSized -MaxMemory 1MB -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'LimitsExceeded'
        $pattern = "^Cannot expand the input within -MaxMemory of 1048576 bytes: it needs about (\d+) bytes, $($script:small.Length) of them for its result, as its frames record, which is held in memory once, as the byte\[\] it is expanded into\.$"
        ($failures[0].Exception.Message -match $pattern) | Should -BeTrue -Because $failures[0].Exception.Message
        $needed = [long] $Matches[1]
        Get-Digest (Expand-Zstd -InputObject $script:wideSized -MaxMemory $needed) | Should -Be (Get-Digest $script:small)
        $output = Expand-Zstd -InputObject $script:wideSized -MaxMemory ($needed - 1) -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].Exception.Message | Should -Be "Cannot expand the input within -MaxMemory of $($needed - 1) bytes: it needs about $needed bytes, $($script:small.Length) of them for its result, as its frames record, which is held in memory once, as the byte[] it is expanded into."
    }

    It 'counts the result a frame records as held twice when its bytes arrive in pieces, and refuses it before expanding when it would not fit' {
        $sized = Compress-Zstd -InputObject $script:text
        $head = [byte[]]::new(20)
        $tail = [byte[]]::new($sized.Length - 20)
        [System.Array]::Copy($sized, 0, $head, 0, 20)
        [System.Array]::Copy($sized, 20, $tail, 0, $tail.Length)
        $head, $tail | Expand-Zstd -MaxMemory 1MB -ErrorVariable failures -ErrorAction SilentlyContinue | Should -BeNullOrEmpty
        ($failures[0].Exception.Message -match 'which takes (\d+) bytes to expand, more than -MaxMemory of 1048576 bytes\.$') | Should -BeTrue -Because $failures[0].Exception.Message
        $needed = [long] $Matches[1] + 2 * $script:text.Length
        Get-Digest ($head, $tail | Expand-Zstd -MaxMemory $needed) | Should -Be (Get-Digest $script:text)
        $output = $head, $tail | Expand-Zstd -MaxMemory ($needed - 2) -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'LimitsExceeded'
        $failures[0].Exception.Message | Should -Be ("Cannot expand the input within -MaxMemory of $($needed - 2) bytes: its result of $($script:text.Length) bytes, as its frames record, would be held in memory twice, as it is built and as the byte[] written, and $($script:text.Length - 1) bytes of result is the most that fits.")
    }

    It 'stops expanding bytes whose result outgrows the budget' {
        $streamed = $script:text, $script:text | Compress-Zstd
        Expand-Zstd -InputObject $streamed -MaxMemory 1MB -ErrorVariable failures -ErrorAction SilentlyContinue | Should -BeNullOrEmpty
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdWindowTooLarge,$script:expandId"
        ($failures[0].Exception.Message -match 'which takes (\d+) bytes to expand, more than -MaxMemory of 1048576 bytes\.$') | Should -BeTrue -Because $failures[0].Exception.Message
        $needed = [long] $Matches[1]
        $output = Expand-Zstd -InputObject $streamed -MaxMemory ($needed + 200000) -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:expandId"
        $failures[0].Exception.Message | Should -Be "Cannot expand the input within -MaxMemory of $($needed + 200000) bytes: its result is held in memory twice, as it is built and as the byte[] written, and it outgrew 100000 bytes, the most that fits."
        $whole = 2 * $script:text.Length
        Get-Digest (Expand-Zstd -InputObject $streamed -MaxMemory ($needed + 2 * $whole)) | Should -Be (Get-Digest (Join-Bytes $script:text $script:text))
    }

    It 'writes a file in place when a frame''s window does not fit a chunk at a time and every frame records its content size, when the budget holds what that takes' {
        $packed = Join-Path $TestDrive 'wide-sized.zst'
        [System.IO.File]::WriteAllBytes($packed, $script:wideSized)
        $restored = Join-Path $TestDrive 'wide-sized.bin'
        $inPlace = "Expanding '$packed' in place: a frame in it has a window of 268435456 bytes, which does not fit a chunk at a time."
        $verbose = @(Expand-Zstd -Path $packed -DestinationPath $restored -Verbose 4>&1 | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message })
        @($verbose | Where-Object { $_ -eq $inPlace }).Count | Should -Be 1 -Because ($verbose -join ' | ')
        Get-Digest ([System.IO.File]::ReadAllBytes($restored)) | Should -Be (Get-Digest $script:small)
        Expand-Zstd -Path $packed -DestinationPath $restored -MaxMemory 16MB -Force
        Get-Digest ([System.IO.File]::ReadAllBytes($restored)) | Should -Be (Get-Digest $script:small)
        # 1MB holds neither the map nor the frame's window of 256 MiB.
        $verbose = @(Expand-Zstd -Path $packed -DestinationPath $restored -MaxMemory 1MB -Force -Verbose -ErrorVariable failures -ErrorAction SilentlyContinue 4>&1 | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message })
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdWindowTooLarge,$script:expandId"
        @($verbose | Where-Object { $_ -like "Cannot expand '$packed' in place: mapping it takes * bytes of page tables, and with them it needs about * bytes, more than -MaxMemory of 1048576 bytes." }).Count | Should -Be 1 -Because ($verbose -join ' | ')
        Get-Digest ([System.IO.File]::ReadAllBytes($restored)) | Should -Be (Get-Digest $script:small) -Because 'the file already there survives'
    }

    It 'refuses a frame whose window does not fit a chunk at a time when writing in place does not fit the budget either' {
        $packed = Join-Path $TestDrive 'wide-sized-tight.zst'
        [System.IO.File]::WriteAllBytes($packed, $script:wideSized)
        $restored = Join-Path $TestDrive 'wide-sized-tight.bin'
        # The map of this small file and the command's own share fit, and
        # the page tables of both maps with libzstd's context do not.
        $budget = 4194304 + 8192 + 16384
        $verbose = @(Expand-Zstd -Path $packed -DestinationPath $restored -MaxMemory $budget -Verbose -ErrorVariable failures -ErrorAction SilentlyContinue 4>&1 | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message })
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdWindowTooLarge,$script:expandId"
        @($verbose | Where-Object { $_ -like "Cannot expand '$packed' in place: with the page tables of both files, it needs about * bytes, more than -MaxMemory of $budget bytes." }).Count | Should -Be 1 -Because ($verbose -join ' | ')
        Test-Path -LiteralPath $restored | Should -BeFalse
    }

    It 'reads a file a chunk at a time when the page tables of its map do not fit the budget, and expands a file a chunk at a time when its windows fit' {
        # 512 MiB of zeros, whose map takes 1 MiB of page tables, more than
        # the input buffer libzstd keeps at level 1 for a file read a chunk
        # at a time.
        $zeros = Join-Path $TestDrive 'zeros.bin'
        $stream = [System.IO.File]::Create($zeros)
        $stream.SetLength(512MB)
        $stream.Dispose()
        $packed = Join-Path $TestDrive 'zeros.bin.zst'
        $fitted = Get-Fitted @(Compress-Zstd -Path $zeros -DestinationPath $packed -Level 1 -MaxMemory 1GB -Verbose 4>&1)
        ($fitted[0] -match 'estimated at (\d+) bytes\.$') | Should -BeTrue -Because $fitted[0]
        $mapped = [long] $Matches[1]
        $records = @(Compress-Zstd -Path $zeros -DestinationPath $packed -Level 1 -MaxMemory ($mapped - 1) -Force -Verbose 4>&1)
        $verbose = @($records | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message })
        $reading = "Reading '$zeros' a chunk at a time: mapping it takes * bytes of page tables, and with them compressing it needs about $mapped bytes, more than -MaxMemory of $($mapped - 1) bytes."
        @($verbose | Where-Object { $_ -like $reading }).Count | Should -Be 1 -Because ($verbose -join ' | ')
        (Get-Fitted $records)[0] -match 'estimated at (\d+) bytes\.$' | Should -BeTrue
        [long] $Matches[1] | Should -BeLessThan ($mapped - 1)
        $restored = Join-Path $TestDrive 'zeros.out'
        Expand-Zstd -Path $packed -DestinationPath $restored -MaxMemory 1MB -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdWindowTooLarge,$script:expandId"
        ($failures[0].Exception.Message -match 'which takes (\d+) bytes to expand') | Should -BeTrue -Because $failures[0].Exception.Message
        $needed = [long] $Matches[1]
        $verbose = @(Expand-Zstd -Path $packed -DestinationPath $restored -MaxMemory $needed -Verbose 4>&1 | Where-Object { $_ -is [System.Management.Automation.VerboseRecord] } | ForEach-Object { $_.Message })
        @($verbose | Where-Object { $_ -like '*in place*' }).Count | Should -Be 0 -Because ($verbose -join ' | ')
        (Get-FileHash -LiteralPath $restored -Algorithm SHA256).Hash | Should -Be (Get-FileHash -LiteralPath $zeros -Algorithm SHA256).Hash
    }

    It 'expands a file whose frame records no content size a chunk at a time, its window counted, and leaves no file behind when it does not fit' {
        $packed = Join-Path $TestDrive 'wide.zst'
        [System.IO.File]::WriteAllBytes($packed, $script:wide)
        $restored = Join-Path $TestDrive 'wide.bin'
        $before = @(Get-ChildItem -LiteralPath $TestDrive -Force).Count
        Expand-Zstd -Path $packed -DestinationPath $restored -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdWindowTooLarge,$script:expandId"
        $failures[0].TargetObject | Should -Be $packed
        $pattern = "^Cannot expand '$([regex]::Escape($packed))': a frame in it has a window of 268435456 bytes, more than the 134217728 bytes libzstd expands without -MaxMemory; -MaxMemory of (\d+) bytes or more lets it\.$"
        ($failures[0].Exception.Message -match $pattern) | Should -BeTrue -Because $failures[0].Exception.Message
        $needed = [long] $Matches[1]
        Test-Path -LiteralPath $restored | Should -BeFalse
        @(Get-ChildItem -LiteralPath $TestDrive -Force).Count | Should -Be $before
        Expand-Zstd -Path $packed -DestinationPath $restored -MaxMemory $needed
        Get-Digest ([System.IO.File]::ReadAllBytes($restored)) | Should -Be (Get-Digest $script:small)
    }

    It 'refuses a budget too small for the calling thread, naming what the compression needs, and writes nothing' {
        $output = Compress-Zstd -InputObject $script:text -Level 19 -MaxMemory 4MB -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:compressId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'LimitsExceeded'
        $failures[0].Exception.Message | Should -Match '^Cannot compress the input within -MaxMemory of 4194304 bytes: at level 19 on the calling thread it needs about \d+ bytes, \d+ of them for its result of up to \d+ bytes, which is held in memory twice, as it is built and as the byte\[\] written\.$'
        $packed = Join-Path $TestDrive 'budget-19.zst'
        Compress-Zstd -Path $script:source -DestinationPath $packed -Level 19 -MaxMemory 4MB -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:compressId"
        $failures[0].TargetObject | Should -Be $script:source
        $failures[0].Exception.Message | Should -Match "^Cannot compress '$([regex]::Escape($script:source))' within -MaxMemory of 4194304 bytes: at level 19 on the calling thread it needs about \d+ bytes\.$"
        Test-Path -LiteralPath $packed | Should -BeFalse
    }

    It 'compresses a file with as many of the worker threads asked for as fit the budget, and warns that fewer fit' {
        $packed = Join-Path $TestDrive 'budget-workers.zst'
        $fitted = Get-Fitted @(Compress-Zstd -Path $script:source -DestinationPath $packed -Threads 8 -MaxMemory 64MB -Verbose -WarningVariable warnings -WarningAction SilentlyContinue 4>&1)
        $fitted.Count | Should -Be 1
        ($fitted[0] -match '^Within -MaxMemory of 67108864 bytes: level 3 with (\d+) worker threads, estimated at (\d+) bytes\.$') | Should -BeTrue -Because $fitted[0]
        $workers = [int] $Matches[1]
        $estimate = [long] $Matches[2]
        $workers | Should -BeGreaterOrEqual 1
        $workers | Should -BeLessThan 8
        $estimate | Should -BeLessOrEqual 67108864
        $warnings.Count | Should -Be 1
        $warnings[0].Message | Should -Be "Compressing '$script:source' with $workers of the 8 worker threads asked for, the most that fit -MaxMemory of 67108864 bytes: estimated at $estimate bytes."
        $restored = Join-Path $TestDrive 'budget-workers.bin'
        Expand-Zstd -Path $packed -DestinationPath $restored
        (Get-FileHash -LiteralPath $restored -Algorithm SHA256).Hash | Should -Be $script:sourceHash
        Compress-Zstd -Path $script:source -DestinationPath $packed -Threads 8 -MaxMemory 1GB -Force -WarningVariable none -WarningAction SilentlyContinue
        $none.Count | Should -Be 0 -Because 'all 8 fit a gibibyte'
    }

    It 'counts a dictionary, and refuses one that does not fit before any frame' {
        # Raw content serves as the dictionary, at about 1.1 MB.
        $dictionary = $script:text
        $data = New-Text 3000
        $without = Get-Fitted @(Compress-Zstd -InputObject $data -MaxMemory 1GB -Verbose 4>&1)
        $with = Get-Fitted @(Compress-Zstd -InputObject $data -Dictionary $dictionary -MaxMemory 1GB -Verbose 4>&1)
        ($without[0] -match 'estimated at (\d+) bytes') | Should -BeTrue
        $bare = [long] $Matches[1]
        ($with[0] -match 'estimated at (\d+) bytes') | Should -BeTrue
        ([long] $Matches[1] - $bare) | Should -BeGreaterThan (2 * $dictionary.Length) -Because 'the command''s copy, libzstd''s copy and its tables'
        $packed = Compress-Zstd -InputObject $data -Dictionary $dictionary
        $output = Expand-Zstd -InputObject $packed -Dictionary $dictionary -MaxMemory 2MB -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:expandId"
        "$($failures[0].CategoryInfo.Category)" | Should -Be 'LimitsExceeded'
        $failures[0].Exception.Message | Should -Match '^Cannot expand the input within -MaxMemory of 2097152 bytes: with its dictionary, expanding needs about \d+ bytes before any frame, \d+ of them for the dictionary\.$'
        $file = Join-Path $TestDrive 'budget-dictionary.zst'
        [System.IO.File]::WriteAllBytes($file, $packed)
        Expand-Zstd -Path $file -DestinationPath (Join-Path $TestDrive 'budget-dictionary.bin') -Dictionary $dictionary -MaxMemory 2MB -ErrorVariable failures -ErrorAction SilentlyContinue
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:expandId"
        $failures[0].TargetObject | Should -Be $file
        Get-Digest (Expand-Zstd -InputObject $packed -Dictionary $dictionary -MaxMemory 64MB) | Should -Be (Get-Digest $data)
    }

    It 'counts the frame of a single byte[] at its largest, twice' {
        $fitted = Get-Fitted @(Compress-Zstd -InputObject $script:text -MaxMemory 1GB -Verbose 4>&1)
        ($fitted[0] -match '^Within -MaxMemory of 1073741824 bytes: level 3 with 0 worker threads, estimated at (\d+) bytes, and its result of up to (\d+) bytes, held in memory twice\.$') | Should -BeTrue -Because $fitted[0]
        $working = [long] $Matches[1]
        $bound = [long] $Matches[2]
        $bound | Should -BeGreaterThan $script:text.Length
        $packed = Compress-Zstd -InputObject $script:text -MaxMemory ($working + 2 * $bound)
        Get-Digest (Expand-Zstd -InputObject $packed) | Should -Be (Get-Digest $script:text)
        $output = Compress-Zstd -InputObject $script:text -MaxMemory ($working + 2 * $bound - 1) -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:compressId"
        $failures[0].Exception.Message | Should -Be ("Cannot compress the input within -MaxMemory of $($working + 2 * $bound - 1) bytes: at level 3 on the calling thread it needs about $($working + 2 * $bound) bytes, $(2 * $bound) of them for its result of up to $bound bytes, which is held in memory twice, as it is built and as the byte[] written.")
    }

    It 'stops a stream of records whose frame outgrows the budget' {
        $first = New-Noise 1048576 21
        $second = New-Noise 1048576 22
        $fitted = Get-Fitted @($first, $second | Compress-Zstd -MaxMemory 1GB -Verbose 4>&1)
        ($fitted[0] -match 'estimated at (\d+) bytes, and its result of up to \d+ bytes, held in memory twice\.$') | Should -BeTrue -Because $fitted[0]
        $budget = [long] $Matches[1] + 1MB
        $output = $first, $second | Compress-Zstd -MaxMemory $budget -ErrorVariable failures -ErrorAction SilentlyContinue
        $output | Should -BeNullOrEmpty
        $failures.Count | Should -Be 1
        $failures[0].FullyQualifiedErrorId | Should -Be "ZstdBudgetTooSmall,$script:compressId"
        $failures[0].Exception.Message | Should -Be "Cannot compress the input within -MaxMemory of $budget bytes: its result is held in memory twice, as it is built and as the byte[] written, and it outgrew 524288 bytes, the most that fits."
    }
}

Describe 'Stopping the cmdlets while they work' {
    BeforeAll {
        $script:manifest = Join-Path $env:PWRS_MODULE 'Zstd.psd1'

        # Runs one command in a runspace of its own, which imports the
        # module when it opens, and calls Stop() on it as soon as its
        # temporary output appears in $Folder, the destination's folder,
        # which holds nothing else; or, with $VerboseLike, as soon as it
        # writes a verbose message like that, -Verbose being passed to it.
        # The invocation is one command rather than a batch of statements,
        # and it is ended with EndInvoke before the runspace is disposed, so
        # no work of it can outlive the runspace. Returns whether what it
        # waited for was seen, the state the command ended in and why, the
        # exception EndInvoke raised, and every name left in $Folder.
        function Stop-Midstream([string] $Command, [hashtable] $Parameters, [string] $Folder, [string] $VerboseLike) {
            $state = [System.Management.Automation.Runspaces.InitialSessionState]::CreateDefault()
            $state.ImportPSModule([string[]] @($script:manifest))
            $runspace = [runspacefactory]::CreateRunspace($state)
            $runspace.Open()
            $shell = [powershell]::Create()
            try {
                $shell.Runspace = $runspace
                $null = $shell.AddCommand($Command)
                foreach ($name in $Parameters.Keys) { $null = $shell.AddParameter($name, $Parameters[$name]) }
                if ($VerboseLike) { $null = $shell.AddParameter('Verbose', $true) }
                $running = $shell.BeginInvoke()
                $clock = [System.Diagnostics.Stopwatch]::StartNew()
                $seen = $false
                $read = 0
                while (-not $running.IsCompleted -and $clock.Elapsed.TotalSeconds -lt 120) {
                    if ($VerboseLike) {
                        while ($read -lt $shell.Streams.Verbose.Count) {
                            if ($shell.Streams.Verbose[$read].Message -like $VerboseLike) { $seen = $true }
                            $read++
                        }
                    } elseif ([System.IO.Directory]::GetFiles($Folder, '*.zstd-partial').Count -gt 0) {
                        $seen = $true
                    }
                    if ($seen) { break }
                }
                $shell.Stop()
                $ended = 'EndInvoke returned'
                try {
                    $null = $shell.EndInvoke($running)
                } catch {
                    $cause = $_.Exception
                    if ($cause.InnerException) { $cause = $cause.InnerException }
                    $ended = $cause.GetType().Name
                }
                [pscustomobject]@{
                    Seen       = $seen
                    State      = "$($shell.InvocationStateInfo.State)"
                    Reason     = "$($shell.InvocationStateInfo.Reason)"
                    Ended      = $ended
                    Left       = @([System.IO.Directory]::GetFileSystemEntries($Folder))
                }
            } finally {
                $shell.Dispose()
                $runspace.Dispose()
            }
        }
    }

    It 'stops Compress-Zstd mid-stream and leaves neither the destination nor its temporary file' {
        # 32 MiB of noise at level 19 takes seconds, so the stop lands
        # while the file is being read.
        $source = Join-Path $TestDrive 'stop-source.bin'
        [System.IO.File]::WriteAllBytes($source, (New-Noise 33554432 21))
        $folder = Join-Path $TestDrive 'stop-compress'
        $null = New-Item -ItemType Directory -Path $folder
        $stopped = Stop-Midstream 'Compress-Zstd' @{ Path = $source; DestinationPath = (Join-Path $folder 'out.zst'); Level = 19 } $folder
        $stopped.Seen | Should -BeTrue -Because $stopped.Reason
        $stopped.State | Should -Be 'Stopped' -Because $stopped.Reason
        $stopped.Ended | Should -Be 'PipelineStoppedException'
        $stopped.Left | Should -BeNullOrEmpty
    }

    It 'stops Expand-Zstd mid-stream and leaves neither the destination nor its temporary file' {
        # 400 copies of one frame, which expand to about 0.9 GB.
        $frame = Compress-Zstd -InputObject (New-Text 40000)
        $packed = Join-Path $TestDrive 'stop-frames.zst'
        $out = [System.IO.File]::Create($packed)
        try {
            for ($i = 0; $i -lt 400; $i++) { $out.Write($frame, 0, $frame.Length) }
        } finally {
            $out.Dispose()
        }
        $folder = Join-Path $TestDrive 'stop-expand'
        $null = New-Item -ItemType Directory -Path $folder
        $stopped = Stop-Midstream 'Expand-Zstd' @{ Path = $packed; DestinationPath = (Join-Path $folder 'out.bin') } $folder
        $stopped.Seen | Should -BeTrue -Because $stopped.Reason
        $stopped.State | Should -Be 'Stopped' -Because $stopped.Reason
        $stopped.Ended | Should -Be 'PipelineStoppedException'
        $stopped.Left | Should -BeNullOrEmpty
    }

    It 'leaves an existing destination as it was when stopped under -Force' {
        $source = Join-Path $TestDrive 'stop-force-source.bin'
        [System.IO.File]::WriteAllBytes($source, (New-Noise 33554432 22))
        $folder = Join-Path $TestDrive 'stop-force'
        $null = New-Item -ItemType Directory -Path $folder
        $existing = Join-Path $folder 'out.zst'
        [System.IO.File]::WriteAllText($existing, 'keep me')
        $stopped = Stop-Midstream 'Compress-Zstd' @{ Path = $source; DestinationPath = $existing; Level = 19; Force = $true } $folder
        $stopped.Seen | Should -BeTrue -Because $stopped.Reason
        $stopped.State | Should -Be 'Stopped' -Because $stopped.Reason
        $stopped.Ended | Should -Be 'PipelineStoppedException'
        $stopped.Left | Should -Be @($existing)
        [System.IO.File]::ReadAllText($existing) | Should -Be 'keep me'
    }

    It 'stops New-ZstdDictionary during training and writes no dictionary' {
        # Sixteen samples of about 1.4 MB each, text and then noise, which
        # take libzstd's trainer a while; the stop comes once the verbose
        # stream says training has begun.
        $samples = Join-Path $TestDrive 'stop-samples'
        $null = New-Item -ItemType Directory -Path $samples
        $text = New-Text 20000
        for ($i = 0; $i -lt 16; $i++) {
            [System.IO.File]::WriteAllBytes((Join-Path $samples "sample$i.bin"), (Join-Bytes $text (New-Noise 262144 $i)))
        }
        $folder = Join-Path $TestDrive 'stop-train'
        $null = New-Item -ItemType Directory -Path $folder
        $stopped = Stop-Midstream 'New-ZstdDictionary' @{ Path = (Join-Path $samples '*'); DestinationPath = (Join-Path $folder 'stopped.dict') } $folder 'Training a dictionary*'
        $stopped.Seen | Should -BeTrue -Because $stopped.Reason
        $stopped.State | Should -Be 'Stopped' -Because $stopped.Reason
        $stopped.Ended | Should -Be 'PipelineStoppedException'
        $stopped.Left | Should -BeNullOrEmpty
    }
}
