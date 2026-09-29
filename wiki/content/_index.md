---
title: Zstd Wiki
toc: false
---

Zstandard (zstd) compression for Windows PowerShell 5.1 and PowerShell 7.4 or later, with libzstd 1.5.7 compiled into the module. `Compress-Zstd` and `Expand-Zstd` work on files and on bytes in memory, and `New-ZstdDictionary` trains dictionaries from samples.

```powershell
Install-Module Zstd -Scope CurrentUser
$text = 'The quick brown fox jumps over the lazy dog. ' * 20
$packed = Compress-Zstd -InputObject ([System.Text.Encoding]::UTF8.GetBytes($text))
'{0} bytes of text, {1} bytes compressed' -f $text.Length, $packed.Length
[System.Text.Encoding]::UTF8.GetString((Expand-Zstd -InputObject $packed)) -eq $text
```

```text
900 bytes of text, 68 bytes compressed
True
```

{{< cards >}}
  {{< card link="docs/tutorials/" title="Tutorials" subtitle="Learning-oriented. Getting Started installs the module, compresses and expands a file, then bytes in memory." icon="academic-cap" >}}
  {{< card link="docs/how-to/" title="How-to" subtitle="Task-oriented recipes: files, bytes, dictionaries, errors, levels and threads." icon="cog" >}}
  {{< card link="docs/explanation/" title="Explanation" subtitle="Why the module exists, how it moves data and what that costs in memory, what a frame records, how dictionaries work." icon="book-open" >}}
  {{< card link="docs/reference/" title="Reference" subtitle="Every cmdlet with its parameters and output types, every error id, and where it has been run." icon="document-text" >}}
  {{< card link="docs/building/" title="Building From Source" subtitle="The build, the module folder it writes, and the tests, for working on the module itself." icon="code" >}}
{{< /cards >}}

## Quick links

- **New here?** [Getting Started](Getting-Started.md): install, compress a file, expand it.
- **Looking up a parameter or an output type?** [Compress-Zstd](Compress-Zstd.md), [Expand-Zstd](Expand-Zstd.md) and [New-ZstdDictionary](New-ZstdDictionary.md).
- **An error record in hand?** [Error Reference](Error-Reference.md), by the id its `FullyQualifiedErrorId` starts with, and [How To Handle Errors](How-To-Handle-Errors.md).
- **Wondering what it costs in memory?** [Streaming And Memory](Streaming-And-Memory.md).

## Where the output on these pages comes from

Every example on these pages was run on Windows 11 Pro 10.0.26200 on an AMD Ryzen 9 7900X, in PowerShell 7.6.6 and in Windows PowerShell 5.1.26100.9444, against the module `cargo pwrs build --release` built from the code these pages describe, with PoWerRuSt 0.3.0 and cargo-pwrs 0.3.0 from crates.io. The output shown is PowerShell 7's; Windows PowerShell printed the same, apart from the differences each page names. Where a page says outside an example what a run showed, it reports a build, a test run or an end-to-end run that [Where It Has Been Run](Where-It-Has-Been-Run.md) lists with its machine and build.

## License

MIT, in [LICENSE](https://github.com/Variably-Constant/Zstd/blob/main/LICENSE) on the repository.
