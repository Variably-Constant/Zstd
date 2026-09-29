---
title: Documentation
toc: false
sidebar:
  open: true
---

The Zstd docs are organized under the [Diataxis](https://diataxis.fr/) framework: four kinds of documentation, each answering a different reader question, and a fifth section for building the module from its repository. Every page names the source it describes; the module is one file, `src/lib.rs`, with its Pester suite in `tests/Zstd.Tests.ps1`.

{{< cards >}}
  {{< card link="tutorials/" title="Tutorials" subtitle="Learning-oriented. Getting Started: install, compress and expand a file, then bytes in memory." icon="academic-cap" >}}
  {{< card link="how-to/" title="How-to" subtitle="Task-oriented recipes: files, bytes, dictionaries, errors, levels and threads." icon="cog" >}}
  {{< card link="explanation/" title="Explanation" subtitle="Why the module exists, how it moves data and what that costs in memory, what a frame records, and how dictionaries work." icon="book-open" >}}
  {{< card link="reference/" title="Reference" subtitle="Every cmdlet with its parameters and output types, every error id, and where it has been run." icon="document-text" >}}
  {{< card link="building/" title="Building From Source" subtitle="What the build needs, cargo pwrs build and test, and the module folder." icon="code" >}}
{{< /cards >}}
