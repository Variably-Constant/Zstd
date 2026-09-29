---
title: Explanation
weight: 3
sidebar:
  open: true
---

The why behind the module, and what follows from how it is built.

{{< cards >}}
  {{< card link="Why-Zstd/" title="Why Zstd" subtitle="What the hosts offer for compression, what the module adds, and how it compares with ZstdSharp and Deflate, timed." >}}
  {{< card link="Streaming-And-Memory/" title="Streaming And Memory" subtitle="How files, bytes and samples move through the cmdlets, what that costs in memory, measured on a 5 GiB file, and how -MaxMemory bounds it." >}}
  {{< card link="Frames-And-Records/" title="Frames And Records" subtitle="What a zstd frame records, and how pipeline records become one." >}}
  {{< card link="Dictionaries/" title="Dictionaries" subtitle="What a dictionary is, the two kinds the module takes, and how a frame names the one it needs." >}}
{{< /cards >}}
