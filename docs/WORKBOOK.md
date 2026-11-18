# Workbook and package model

TabKit operates on TWB/TWBX snapshots. Package admission happens before semantic indexing.

## Open path

```mermaid
flowchart LR
    input["TWB / TWBX"] --> package["Package admission"]
    package --> twb["Embedded TWB bytes"]
    twb --> xml["Source-backed XML index"]
    xml --> semantic["Typed workbook index"]
    semantic --> lineage["Lineage index"]
```

| Layer | Owns |
| --- | --- |
| package | file type, size limits, TWBX ZIP safety, source hash |
| XML | source spans, attributes, parent/child links, subtree ranges |
| workbook | supported Tableau 26.2 semantic objects |
| lineage | dependency edges and coverage gaps |

The original TWB source remains authoritative. TabKit does not deserialize the entire workbook into a
second object model.


## Blank workbook creation

`workbook_create` writes one TWB 26.2 template into TabKit's temporary workspace.

| Root contract | Value |
| --- | --- |
| `version` | `26.2` |
| `original-version` | `26.2` |
| `source-build` | `0.0.0 (0000.0.0.0)` |
| `source-platform` | `win` |
| manifest | `ManifestByVersion` |
| initial semantic objects | zero datasources, worksheets, dashboards |

The template contains the 2026.2 `explain-data` element required by the published XSD root sequence.
It contains no synthetic worksheet or datasource.

Tableau documents `ManifestByVersion` and the zeroed `source-build` form for direct TWB authoring:
https://github.com/tableau/tableau-document-schemas

`workbook_create` creates the local snapshot only. `workbook_validate` submits the resulting TWB to
Tableau Server before publish.

## Admission limits

Default limits:

| Limit | Value |
| --- | ---: |
| embedded TWB XML | 256 MiB |
| package file | 4 GiB |
| TWBX entries | 8,192 |
| TWBX expanded bytes | 16 GiB |
| per-entry compression ratio guard | 5000x + 1 MiB allowance |

TWBX admission rejects:

- absolute paths, traversal segments, backslashes, colons, NUL, and empty path segments;
- duplicate or case-colliding names;
- symlink entries;
- expanded data above 16 GiB;
- an entry whose expanded size exceeds \`compressed_size * 5000 + 1 MiB\`;
- zero or multiple embedded TWB files.

## Version gate

The semantic index admits only:

```text
workbook/@version = 26.2
Tableau target     = 2026.2.4
XSD contract       = 2026.2.0
```

A different workbook version fails with `VERSION` before semantic operations run.

## Indexed objects

`Workbook::summary()` and MCP inspection cover:

| Object | Indexed data |
| --- | --- |
| datasource | name/caption and datasource identity |
| connection | connection identity and supported connection metadata |
| relation | physical/custom SQL/relationship subset used by lineage |
| field | datasource, name/caption, datatype, role/type, calculation link |
| calculation | output field + formula |
| parameter | name/caption, datatype, domain, current value |
| worksheet | name and XML node identity |
| dashboard | name and XML node identity |

Counts describe TabKit's semantic projection, not every element allowed by Tableau's XSD.

## Source-preserving edits

```mermaid
flowchart LR
    snapshot["Admitted snapshot + SHA-256"] --> plan["Semantic edit"]
    plan --> patches["Non-overlapping byte patches"]
    patches --> verify["Preservation checks"]
    verify --> candidate["Candidate TWB / TWBX"]
```

Patch admission requires:

- valid UTF-8 byte boundaries;
- non-overlapping ranges;
- exact expected source bytes;
- bounded candidate size.

Unchanged XML regions are compared byte-for-byte after patching.

For TWBX candidates, only the embedded TWB is replaced. Unrelated entries must preserve:

- entry count/order/name;
- uncompressed and compressed sizes;
- compression method;
- compressed bytes.

TabKit checks the source package hash before and after candidate repack. A hash change fails with
`STALE_BASE`.

## Candidate rule

A candidate keeps the source package type:

```text
.twb  -> .twb
.twbx -> .twbx
```

Existing output paths are never overwritten.

Tableau remains responsible for XSD and Tableau semantic validation:
[Tableau REST](TABLEAU_REST.md#validation).
