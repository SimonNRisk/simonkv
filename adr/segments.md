# Decision
introduce segments to simonkv

## Current Issue
Currently we store all records (SET, DEL) in a single file. Our keydir refers to this file via offsets. This is great for simplicity, but leads to a few issues:
1. Compaction becomes very slow at scale. Current compaction must: create the replacement log, copy every live record to it, sync it, replace the original file.
2. The file being compacted is *changing*. Even if you ran compaction in a separate process (currently, we just block new logs with a `&mut self`), the snapshot of "compaction started" is still not guaranteed to be the current state. If in between copying every live record, and replacing the original file, we get new appends to the log, they're lost

## Proposed Solution
Segmentation involves a large-scale refactor of the current codebase. Rather than a single log file, we'd introduce multiple "segment" files, each with their own threshold on file size. After a file fills, we'd close it, mark it immutable, and direct new appends to a new file. For compaction, we can merge two immutable files, keeping only live records, without having to worry about the segment currently being written to.

### Why file size and not number of records
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
1. Background compaction shares the disk with foreground reads and writes (as it's an embedded database). A huge job, like compacting a huge segment, can monopolize bandwidth and increase user-facing latency.
2. Dead space cannot be reclaimed until the entire segment has been processed and safely replaced - by avoiding giant segments, we avoid delaying reclamation.
3. If there is a transient failure, we avoid retrying the compaction of a huge segment.
4. It makes work schedulable. For segments containing normally sized records, the storage engine could eventually decide that one compaction job selects at most two old segments and use their known file sizes to estimate how much data it will read and write. A segment containing one oversized record is the explicit exception and must be scheduled using its actual size.

A record count would bound different costs:
- Number of decoding operations
- Number of keydir updates
- Per-record CPU overhead

While potentially a hybrid approach is the best long-term design, we will use a file-size target to make ordinary compaction jobs more predictable.

### Maintaining the size target across merges
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
Additionally, magic values and versions will be deferred. Headerless segments become an implicit "version 0", which is fine as we have no consumers.

### Exact rotation rule

The segment size is a soft target, not a hard upper bound. Records are indivisible: SimonKV will never truncate a record or split one across segments.

Before appending a record, SimonKV will encode it so its complete size is known. It will rotate first only when the active segment already contains at least one record and appending the new record would exceed the target:

```text
if active has records and active size + encoded record size > target:
    rotate

append the complete record
```

The comparison is `>` rather than `>=`, so a record that makes a segment exactly equal to the target remains in that segment. Rotation is lazy: SimonKV creates the next segment only when another write requires it.

#### Oversized records

If one encoded record is itself larger than the target, SimonKV writes the complete record into an otherwise-empty segment. The segment is allowed to exceed the target and the following write rotates to a new segment. For example, with a 64 MiB target:

```text
current segment: 20 MiB
next record:    100 MiB

1. Rotate away from the 20 MiB segment.
2. Write the complete 100 MiB record to the empty segment.
3. Rotate before the following record is written.
```

This exception keeps ordinary segments near the configured target without forcing every segment to be larger than the format's maximum record size. A record this large already requires work proportional to its own size, so it is treated as an exceptional compaction unit and scheduled using its actual file size.

Merge output uses the same rule. If the next live record would exceed a nonempty output segment's target, the merge rotates first. An oversized record is written whole to an empty output segment.

Splitting records across segments is deferred. Supporting it would require chunk identifiers, commit markers, multi-file reads, incomplete-record recovery, and compaction that moves every chunk atomically.

For two ordinary segments of size `S`, worst-case merge I/O remains approximately:

```
read inputs: 2S
write outputs: 2S
total: 4S
```

### Store layout

We have two broad choices for storing segments. We could keep accepting a path like `simonkv.log` and create sibling files such as `simonkv.log.000001`, or we could make each store a directory that owns all of its segment and temporary files.

For SimonKV, `KVStore::open(path)` will treat `path` as a store directory. Keeping every file inside one directory gives startup one place to discover segments, keeps temporary merge files isolated, and gives future hint and lock files a natural home. A database will look like:

```text
store/
  00000000000000000001.data
  00000000000000000002.data
  00000000000000000003.active
```

Files ending in `.data` are immutable. Exactly one file ends in `.active` during normal operation, and only that file may receive appends. Merge output is first written with a `.merge` suffix and is not part of the database until it is renamed to `.data`.

We could give every file the same extension and infer that the highest ID is active. An explicit `.active` suffix is preferable because startup can distinguish a writable file from immutable merge output without relying on position alone. The store directory is owned by SimonKV. An unexpected regular file will cause `open` to return an error instead of silently ignoring something that might contain database data.

### Segment IDs and replay order

Segment IDs could be timestamps, random identifiers, or monotonic integers. Timestamps introduce clock behavior and collisions that SimonKV does not need. Random identifiers do not establish replay order.

SimonKV will use monotonically increasing `u64` IDs starting at `1`. Finalized IDs will never be reused. The 20-digit zero-padded filename makes lexical order match numeric order, but startup will still parse and sort IDs numerically rather than trust directory iteration order.

Startup replays segments from the lowest ID to the highest ID. A newer record therefore replaces an older keydir entry. The active segment must have the highest existing segment ID during normal operation.

Merge output receives fresh IDs greater than every existing segment. Before `compact` accepts another write, it creates a new active segment whose ID is greater than every merge output. This keeps future writes newer than merged records when the database is replayed.

We will not add a per-record timestamp. Synchronous compaction gives us a stable keydir, exact `(file_id, offset)` comparisons identify live records, and segment IDs establish replay order. A timestamp can be reconsidered if SimonKV later adds expiration or concurrent compaction.

### Segment-size configuration

The segment target could be a fixed constant, a required argument every time the store is opened, or an option with a default. A fixed constant would make tests and later tuning awkward. Requiring the value on every open would make the simple API unnecessarily noisy.

SimonKV will keep `KVStore::open(path)` with a provisional default target of 64 MiB and add an options-based way to override it. Tests can use very small targets to exercise rotation without creating large files. The configured value must be greater than zero.

The target measures the complete file size and the complete encoded record size, including record headers and checksums. Changing the configured target affects future rotation and merge output only. Existing larger segments remain readable.

The 64 MiB default is not a claim that 64 MiB is optimal. It is an initial usable value that keeps the API simple. Startup, compaction, and workload benchmarks will determine whether it should change.

### Keydir locations

With one log file, an offset uniquely identifies a record. With multiple files, the keydir must also identify the segment. We could store only `(file_id, offset)` and decode the record header whenever its size is needed, or also store the encoded record length.

SimonKV will store:

```text
Location {
    file_id,
    offset,
    record_len,
}
```

The additional length increases keydir memory use, but it lets reads validate that they consumed the expected record and lets compaction account for live bytes without reading record headers again. All three fields will use `u64`, matching file offsets and allowing for the full encoded record size.

### Accessing immutable segment files

SimonKV could keep every immutable segment open, open the required file for every read, or maintain a bounded cache of open files. Keeping everything open provides fast reads but eventually risks operating-system file descriptor limits. A bounded cache avoids that limit but adds eviction and lifecycle behavior before benchmarks show it is needed.

For the MVP, SimonKV will keep the active file open and open immutable files on demand for reads. It will maintain an in-memory mapping from file ID to path. This prioritizes simple ownership and avoids an unbounded file-handle collection. If benchmarks show that opening files dominates point-read latency, a bounded file-handle cache can be added without changing the on-disk format.

### Rotation durability

Closing one file and creating another is not enough for durable rotation. File contents and directory mappings are persisted separately. SimonKV could infer the active file after a crash and avoid renaming, but explicit active and immutable states make recovery easier to reason about.

When a pending record requires rotation, SimonKV will:

1. Sync and close the current active file.
2. Rename it from `.active` to `.data`.
3. Create the next ID as `.active` using `create_new`.
4. Sync the new file.
5. Sync the store directory so the rename and creation are durable.
6. Append and sync the pending record.
7. Update the keydir only after the record is durable.

A crash before the pending record is synced may leave an empty active file, which is valid. A crash between the rename and creation may leave no active file, which startup can recover by creating a new highest-ID active segment. SimonKV will never acknowledge a write before its record has been synced.

### Startup and recovery

Startup could try to repair every malformed state automatically, or it could recover only states produced by SimonKV's documented write protocols. Broad automatic repair risks silently choosing the wrong history, so SimonKV will recover only unambiguous states.

On open, SimonKV will:

1. Enumerate and validate recognized filenames.
2. Remove leftover `.merge` files. Merge inputs are not deleted until all output files are published, so an unpublished temporary file is never the only durable copy.
3. Reject an ID used by more than one recognized file, unexpected regular files, or more than one `.active` file.
4. Sort `.data` files and the `.active` file by numeric ID and replay them from oldest to newest.
5. Treat corruption or truncation in an immutable `.data` file as an error.
6. Recover a truncated final record only in `.active` by truncating back to the last valid offset and syncing the file.
7. Create ID `1` as active for an empty store.
8. If an interrupted rotation or compaction left no active file, create a new active file with an ID greater than every finalized segment.

An active file whose ID is lower than a finalized data file, or multiple active files, does not match a state produced by the chosen protocols. SimonKV will return an error rather than guess which file is newest.

### Which segments to compact

Compaction could merge every immutable segment, choose files with the most dead bytes, or choose a small oldest group. Merging everything recreates the unbounded job that segmentation is intended to avoid. Selecting arbitrary fragmented files requires stronger tombstone tracking because older records may remain outside the selected set.

For the MVP, `compact()` remains an explicit operation and selects the oldest one or two immutable segments. If the oldest segment exceeds the configured target because it contains an oversized record, SimonKV will compact that segment by itself. The active segment is never an input.

Selecting an oldest prefix bounds ordinary input work and permits safe tombstone removal. Automatic compaction and selection based on dead-space ratios will be considered after SimonKV records per-segment statistics and has representative benchmarks.

### Live records and tombstones

Compaction could update the keydir as it copies records, rebuild it afterward, or preserve every record and rely only on replay order. Preserving every record would not reclaim space. Incremental keydir mutation is faster but creates more states that must remain consistent with partially published files.

Because `compact(&mut self)` blocks writes, the keydir is stable for the entire merge. A SET record is live only when the keydir points to its exact `(file_id, offset)`. Only live SET records are copied. After publication, SimonKV will rebuild the keydir by replaying the finalized segments. Rebuilding is slower than patching locations, but it reuses the startup path and reduces the chance of an in-memory state disagreeing with disk.

The current keydir removes a key when it sees a DEL, so it does not retain the tombstone's location. This makes arbitrary partial compaction unsafe. For example, deleting a tombstone while an older SET remains would resurrect that value on restart.

For example, suppose segment 1 contains `SET color blue` and segment 2 contains `DEL color`. If compaction selects only segment 2 and drops the DEL, segment 1 remains on disk. On restart, replay sees only `SET color blue`, so the deleted key is incorrectly restored. Compacting the oldest prefix would select both segments, allowing both records to be removed safely.

SimonKV will avoid a separate tombstone index by compacting only an oldest prefix. When a selected tombstone is removed, every older version it could hide is also in the selected prefix or has already been removed. If a newer SET exists outside the prefix, the tombstone is no longer the current state. This permits tombstones in the selected prefix to be discarded safely.

The alternative is to retain tombstone locations in memory and copy tombstones until every older version is gone. That would permit arbitrary segment selection, but it adds memory use and garbage-collection rules that the MVP does not need.

### Merge output and publication

Replacing several input files with several output files cannot be done with one atomic rename. We could introduce a manifest that atomically selects a generation, or design publication so old inputs and new outputs may safely coexist after any crash. A manifest gives a clean commit point but introduces another durable file and recovery protocol.

For the MVP, SimonKV will use coexistence-safe publication:

1. Scan the selected input segments and write output files with fresh IDs and `.merge` suffixes. Apply the same size-target and oversized-record rules used for normal writes.
2. Sync every temporary output file. If there are no live output records, retain the existing active file, delete the selected inputs, sync the directory, rebuild the keydir, and return.
3. If outputs exist, sync and close the current active file, rename it to `.data`, and sync the directory. Writes remain blocked.
4. Rename every complete `.merge` file to `.data`, then sync the directory.
5. Create and sync a new active segment whose ID is greater than every output, then sync the directory.
6. Delete the selected input segments only after every output is durable, then sync the directory again.
7. Rebuild the keydir from the finalized segments before returning.

If a crash occurs before publication, the original inputs remain and temporary outputs are discarded on startup. If a crash occurs after only some outputs are published, the original inputs still remain. A published output contains only complete records that were live in the stable keydir, so replaying it after its source produces the same logical value. If a crash occurs while inputs are being deleted, every output is already durable and any remaining input is only a duplicate.

Freezing the old active file before publishing outputs ensures startup never needs to append to a lower-ID active segment after higher-ID merge output. The new active segment is created before writes are accepted, so future records always replay after merged records.

### Compaction trigger

SimonKV could rotate into automatic compaction after a segment count, a dead-byte ratio, or a time interval. None of those policies can be justified until the engine records useful segment statistics and the workload is measured.

For the MVP, compaction remains manual through `compact()`. This separates correctness of segmented merging from scheduling policy. Later benchmarks can determine whether segment count, dead bytes, or a combination should trigger background work.

### Implementation plan

The work will be split into approximately twelve pull requests:

1. Extract the record codec and scanner, including encoded record lengths.
2. Add `StoreOptions`, segment-target validation, `SegmentId`, and the expanded `Location`.
3. Switch from a log file to a store directory, including filename parsing and inventory validation.
4. Implement ordered multi-segment replay and segment-aware reads.
5. Implement durable active-segment rotation, including exact-fit and oversized records.
6. Add startup recovery for torn active tails and missing active files, plus rejection of invalid states.
7. Add compaction input selection for the oldest one or two segments and oversized segments.
8. Implement exact live-record selection and safe tombstone removal.
9. Implement rotating `.merge` output files with fresh IDs.
10. Implement the complete crash-safe publication protocol and keydir rebuild.
11. Add interruption tests covering each publication boundary and tombstone resurrection.
12. Update examples, documentation, and benchmarks for the segmented layout.

Rotation and merge publication are durability protocols, so each will land atomically with its correctness tests rather than being split into individual filesystem operations.

### Verification

The implementation will need tests covering:

- Rotation when the next record would cross the target.
- A record that exactly fills the remaining target.
- One oversized record and the following rotation.
- Reads, updates, and deletes across several segments.
- Replay order after reopening.
- Recovery of a truncated active tail.
- Rejection of a truncated or corrupt immutable segment.
- Startup with no active segment and rejection of multiple active segments.
- Merge output that rotates into several files.
- Compaction of an oversized segment by itself.
- Tombstones that do not resurrect older values after compaction and reopening.
- Leftover temporary output, partially published output, and duplicated input files from interrupted publication.
- A write after compaction remaining newer after the next reopen.
- Rejection of the old single-file layout.

Background compaction, automatic triggers, arbitrary segment selection, tombstone tracking, bounded file-handle caching, hint files, multi-process access, and storage-format migration remain explicitly deferred.
