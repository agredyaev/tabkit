# Performance

Synthetic release benchmark for TabKit runtime stages.

## Run

```bash
cargo test --release perf::tests::runtime_profile -- \
  --ignored --nocapture --test-threads=1
```

The fixture contains Tableau 26.2 fields, calculations, worksheets, source SQL, and XML padding.
Tableau Server/network time is excluded.

## Reference run

| Input | Semantic scale | Parse + index | Lineage | Exact field inspect | Calc + dashboard edit | 100-calculation batch |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 MiB | 2k fields; 100 calcs; 22 sheets | 5.44 ms | 0.172 ms | 1.12 us | 5.51 ms | 6.36 ms |
| 10 MiB | 20k fields; 1k calcs; 202 sheets | 54.05 ms | 1.75 ms | 0.99 us | 54.51 ms | 60.46 ms |
| 50 MiB | 80k fields; 4k calcs; 802 sheets | 259.22 ms | 7.46 ms | 1.14 us | 263.11 ms | 284.82 ms |

The 50 MiB edit cases use one iteration. Values are regression references from one warmed release
run, not p95 measurements or hardware-independent SLAs.

Additional 50 MiB reference points:

| Operation | Result |
| --- | ---: |
| parse throughput | 193 MiB/s |
| exact lineage lookup | 0.049 us |
| trace <= 1,000 nodes | 115.88 us |

## Dashboard filter scaling

Fixture: one worksheet dependency with 20,000 fields (~3.20 MiB).

| Filter cards | Edit median |
| ---: | ---: |
| 1 | 29.05 ms |
| 10 | 28.85 ms |
| 100 | 28.97 ms |

Filter metadata is indexed once per source worksheet per edit batch.

## Complexity

| Path | Behavior |
| --- | --- |
| XML descendants | contiguous preorder node range |
| edit insertion point | O(1) indexed closing-tag offset |
| exact field / lineage name | verified case-insensitive hash index |
| bounded lineage trace | proportional to visited local subgraph |
| dashboard filter lookup | one worksheet index per edit batch |
| candidate verification | one full reparse, O(candidate XML size) |
| TWBX preservation | package/hash I/O for stale-base and byte-preservation checks |

Use this benchmark for same-environment regression comparisons only.
