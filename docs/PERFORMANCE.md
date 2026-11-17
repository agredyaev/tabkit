# Performance

Synthetic release benchmark for TapKit.

Command:

```bash
cargo test --release perf::tests::runtime_profile -- \
  --ignored --nocapture --test-threads=1
```

The benchmark generates Tableau 26.2 TWB files for the Tableau 2026.2.4 target with fields,
calculated fields, worksheets, source SQL, and padding to the requested raw XML size. It measures
TapKit itself; it does not include Tableau Server network latency.

## Warm release run

| Input | Semantic scale | Parse + typed index | Parse throughput | Lineage build | Exact lineage find | Trace <=1,000 nodes | Exact field inspect | Calc + dashboard edit | 100-calculation batch |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 MiB | 2,000 fields; 100 calcs; 22 worksheets | 5.44 ms | 184 MiB/s | 0.172 ms | 0.042 us | 7.63 us | 1.12 us | 5.51 ms | 6.36 ms |
| 10 MiB | 20,000 fields; 1,000 calcs; 202 worksheets | 54.05 ms | 185 MiB/s | 1.75 ms | 0.040 us | 53.15 us | 0.99 us | 54.51 ms | 60.46 ms |
| 50 MiB | 80,000 fields; 4,000 calcs; 802 worksheets | 259.22 ms | 193 MiB/s | 7.46 ms | 0.049 us | 115.88 us | 1.14 us | 263.11 ms | 284.82 ms |

The edit measurements include source-preserving patching and the intentional full candidate
reparse/postcondition check. The reparsed candidate `Workbook` is retained and handed to the caller,
so TapKit no longer copies the whole candidate through `String -> Vec<u8> -> String` or reparses it
again merely to create an MCP candidate session.

The 50 MiB edit cases use one iteration to keep the profile practical. Treat those values as
engineering reference points rather than statistical p95 measurements.

## Dashboard filter scaling

A separate fixture contains one worksheet dependency with 20,000 fields (about 3.20 MiB). The same
dashboard edit is repeated with increasing numbers of filter cards over that worksheet:

| Filter cards | Edit median |
| ---: | ---: |
| 1 | 29.05 ms |
| 10 | 28.85 ms |
| 100 | 28.97 ms |

The near-flat result is intentional. Filter metadata is indexed once per worksheet for an edit batch;
each additional card resolves from that index instead of rescanning the worksheet subtree. This is
the regression guard for the former `O(filters x worksheet-subtree)` path.

## Comparison with the previous review baseline

The previous warm 50 MiB baseline was:

- parse/index: 260.68 ms;
- lineage build: 5.79 ms;
- bounded trace: 99.80 us;
- exact field inspect: 514.83 us;
- calculation + dashboard edit: 410.94 ms;
- 100-calculation batch: 431.37 ms.

Current 50 MiB results are:

- parse/index: 259.22 ms;
- lineage build: 7.46 ms;
- exact lineage name lookup: 0.049 us;
- bounded trace: 115.88 us;
- exact field inspect: 1.14 us;
- calculation + dashboard edit: 263.11 ms;
- 100-calculation batch: 284.82 ms.

The extra lineage-build work constructs the exact-name index up front. That cost moves repeated exact
lookups from a full node scan to a verified hash-index lookup. Exact field inspection similarly moves
from roughly 0.5 ms at 80,000 fields to about 1 us.

The edit reduction comes from removing redundant whole-candidate copies/hashes/reopens while keeping
the full semantic postcondition reparse.

## Complexity model after cleanup

The hot paths now have the following intended behavior:

- XML nodes are numbered in preorder and store an exclusive `subtree_end`; descendant iteration is a
  contiguous node-ID range rather than repeated parent climbing.
- Closing-tag insertion offsets are indexed during XML parse, so edit insertion-point lookup is O(1).
- Workbook exact field labels and lineage exact names have case-insensitive hash indexes with actual
  string verification; ambiguous labels fall back safely instead of returning an arbitrary match.
- A bounded lineage trace allocates only for the bounded visited set and builds its result from
  traversed adjacency lists. It no longer allocates/zeros a graph-sized `seen` vector or scans all
  nodes and all edges after BFS. Runtime still depends on the actual local degree/subgraph visited,
  which is why trace time is not expected to be constant across fixtures.
- Dashboard filter metadata is scanned/indexed once per source worksheet per edit batch.
- Calculation insertion does not repeatedly scan the enclosing datasource.
- Candidate postconditions still perform one full reparse by design. That safety boundary remains
  O(candidate XML size), but the parsed candidate is retained rather than thrown away and rebuilt.
- TWBX preservation still performs deliberate package/hash I/O to prove stale-base and unrelated-entry
  byte preservation.

## Repeatability and scope

A preceding final-code run produced 263-265 ms for the 50 MiB calculation + dashboard edit and
285-286 ms for the 100-calculation batch. The warmed figures above are representative for this
environment, not hardware-independent SLAs.

The profile is stage-oriented rather than a flame graph. No measured stage currently justifies unsafe
parser code, weaker preservation checks, or a generic graph/indexing framework. The next performance
evidence should come from real Tableau 2026.2.4 workbooks with representative TWBX package sizes and
semantic complexity.
