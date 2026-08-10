## Overview

`simonkv` is an experimental, Bitcask-inspired embedded key-value database. It was created as a learning exercise for me to better understand storage engines and Rust.

The project targets a single process using local storage. It prioritizes a simple, understandable implementation and durability over production-scale performance. The storage format and API will continue to change as the project develops.

`simonkv` does not currently support multiple processes accessing the same store or network filesystems, and it makes no production-reliability or performance guarantees.

## Performance defaults

`simonkv` has microbenchmarks for basic operations, but does not yet have the startup, compaction, and workload benchmarks needed to justify a segment-size default. Values such as the future segment-size threshold should therefore be configurable, with any initial default documented as provisional rather than optimal.

After segmentation is implemented, candidate thresholds should be benchmarked on a documented reference machine and workload. The measurements should include startup time, blocking compaction time, temporary disk usage, and the resulting number of segment files. If compaction later runs in the background, foreground read and write latency during compaction should also be measured. Those results can then justify a default based on the outcomes the project wants, rather than on an assumed disk speed.

## Record size limits

The current on-disk record header stores key length as an unsigned 16-bit integer and value length as an unsigned 32-bit integer. This gives the storage format the following representational limits:

- Keys may contain at most 65,535 bytes - this is roughly the full text of a short story, or a favicon. Keys are expected to be relatively compact identifiers, so two bytes provide a generous range without spending four or eight bytes on every record's key-length field.
- Values may contain at most 4,294,967,295 bytes - this is ~1 hour of 1080p video, ~5000 photos, or thousands of novels worth of text. Values are expected to vary much more in size than keys, so four bytes preserve a compact fixed-width header while allowing substantially larger payloads.

These widths reflect `simonkv`'s intended workload: compact string identifiers mapped to substantially larger UTF-8 values, such as serialized JSON documents. Because every live key is retained in the in-memory keydir, key size is deliberately limited to 65,535 bytes. Values remain on disk and vary more widely, so their four-byte length field provides format-level capacity up to approximately 4 GiB without enlarging every record header further. This upper bound is representational headroom, not a claim that multi-gigabyte values are efficient or recommended; practical record limits will be determined alongside segmentation and benchmarked workloads.

## Use It

[simonkv on crates.io](https://crates.io/crates/simonkv)
