# Decision
introduce segments to simonkv

## Current Issue
Currently we store all records (SET, DEL) in a single file. Our keydir refers to this file via offsets. This is great for simplicity, but leads to a few issues:
1. Compaction becomes very slow at scale. Current compaction must: create the replacement log, copy every live record to it, sync it, replace the original file.
2. The file being compacted is *changing*. Even if you ran compaction in a separate process (currently, we just block new logs with a `&mut self`), the snapshot of "compaction started" is still not guaranteed to be the current state. If in between copying every live record, and replacing the original file, we get new appends to the log, they're lost

## Proposed Solution
Segmentation involves a large-scale refactor of the current codebase. Rather than a single log file, we'd introduce multiple "segment" files, each with their own threshold on file size. After a file fills, we'd close it, mark it immutable, and direct new appends to a new file. For compaction, we can merge two immutable files, keeping only live records, without having to worry about the segment currently being written to.

###  Why file size and not number of records
For compaction, approximate work is:
time ≈ bytes read + live bytes written + record_count * per-record processing

- Bytes control disk I/O, temporary disk usage, retry cost, and how long reclamation may take
- Record count controls decoding, checksums, keydir lookups, and other per-record CPU work

Disk I/O is the limiting factor here, and is far more expensive than decoding, keydir lookups, and CPU work. 

To allow our engine to effectively predict how expensive compactions will be, the best approach is to bound the expensive part of the compaction - the disk I/O, and thus the bytes.

If you have two different segments:
```
Segment 1: 10,000 x 100-byte records = 1 MB
Segment 2: 10,000 x 1-MB records = 10 GB
```
Both segments have the same record count, but scanning or compacting the second requires 10,000x more disk I/O. By avoiding enormous segments, we get a few benefits:
1. Background compaction sahres the disk with foreground reads and writes (as it's an embedded database). A huge job, like compacting a huge segment, can monopolize bandwidth and increase user-facing latency.
2. Dead space cannot be reclaimed until the entire segment has been processed and safely replaced - by avoiding giant segments, we avoid delaying reclamation.
3. If there is a transient failure, we avoid retrying the compaction of a huge segment.
4. It makes work schedulable. If every segment has a file-size upper bound, the storage engine could eventually decide: one compaction job selects at most two old segments, and then know that it will read / write at most 128 MB each (or 2x your threshold). We can then decide when to compact two segments, given the foreground traffic, as we know how much work they'll take.

A record count would bound different costs:
- Number of decoding operations
- Number of keydir updates
- Per-record CPU overhead

While potentially a hybrid approach is the best long-term design, we will stick with a bounding record size to best achieve our goal of predictable compaction jobs.

### Maintianing size threshold invariant across merges
A scenario can occur where we attempt to merge two files whose sum would exceed our file size threshold.
Example with a 64 MB threshold:

Inputs:
0001.data = 60 MB
0002.data = 60 MB

If we suppose 100 MB remains live, we can't merge all to one file.
To resolve this, we can rotate the output file while scanning two input files record-by-record.
The merge would do this:
1. Read live records -> write ~64 MB to output A
2. Next record will not fit -> rotate output
3. Write remaining ~36 MB to output B

Result:
merged-A.data = ~64 MB
merged-B.data = ~36 MB

### Scope
For now, we remain single-writer and synchronous: `compact(&mut self)` blocks writes. Background compaction, hint files, and multi-process access will come later.
Additionally, magic values and versions will be deferred. Headerless segments become an implicit "version 0.", which is fine as we have no consumers.

### Exact rotation rule
This is really three separate design choices: when to rotate, how strict the bound is, and what to do with oversized records.
#### Rotate before or after writing
For SimonKV, we will rotate before writing. It means a segment can never exceed the target by an arbitrarily large record

#### Handle oversized records
The first question is: can't we just prevent oversized records in the first place? Introduce a hard limit, and bound our segment to always be larger than the max size of record we enforce? Yes, but currently the maximum encoded record can be over 4 GiB. We *could* make our segment that size, but that is an overly large job. Here's why:
If we merge two segments, each of size S, the worst-case I/O is approximately:
```
read inputs: 2S
write outputs: 2S
total: 4S
```
At