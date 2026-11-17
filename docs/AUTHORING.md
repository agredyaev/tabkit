# Authoring

TapKit authors a bounded Tableau 2026.2.4 subset through semantic edit operations. MCP clients provide typed intent, not Tableau XML, node IDs, zone IDs, or XML paths.

## Edit operations

```text
create_calculation
create_parameter
create_worksheet
create_filter_action
create_parameter_action
create_dashboard
```

Every edit produces a new candidate handle. The source handle is unchanged.

Except for calculation-to-calculation references already supported inside one batch, an edit may reference only objects that exist on the source handle. When one newly created object depends on another, use sequential `workbook_edit` calls.

## Parameters

`create_parameter` creates a field in Tableau's synthetic `Parameters` datasource.

Supported datatypes:

- `string`
- `integer`
- `real`
- `boolean`

Supported domains:

- `any`
- `list`
- `range` for integer/real parameters

A list contains 1..1000 values and must contain the current value. A range requires `min <= current <= max` and a positive `step`.

Example:

```json
{
  "op": "create_parameter",
  "caption": "View Mode",
  "datatype": "string",
  "value": "Bar",
  "domain": {
    "kind": "list",
    "values": ["Bar", "Line"]
  },
  "name": "[View Mode]"
}
```

## Worksheet creation

`create_worksheet` uses fields already indexed in one existing datasource. It does not create datasource fields, joins, relationships, calculations, bins, groups, sets, table calculations, or LOD expressions implicitly.

Supported worksheet families:

| kind | required fields | generated structure |
| --- | --- | --- |
| `bar` | one dimension, one numeric measure | discrete dimension + aggregated measure |
| `line` | one date/datetime dimension, one numeric measure | month-truncated continuous date + aggregated measure |
| `kpi` | one numeric measure | text encoding |
| `text_table` | one or two dimensions, one numeric measure | dimensions on shelves + measure text encoding |

Supported measure aggregations are `sum`, `average`, `min`, `max`, `count`, and `count_distinct`.

Example:

```json
{
  "op": "create_worksheet",
  "name": "Sales by Region",
  "datasource": "Orders",
  "kind": "bar",
  "dimension": "Region",
  "secondary_dimension": null,
  "measure": "Sales",
  "aggregation": "sum"
}
```

Unsupported field/type combinations are rejected before patch application.

## Filter actions

`create_filter_action` implements cross-sheet dashboard filtering through Tableau's filter-action contract.

The source worksheet must already be placed on the target dashboard. A selection on the source filters the other compatible worksheets on that dashboard. The source worksheet is excluded from its own action.

Example:

```json
{
  "op": "create_filter_action",
  "caption": "Filter other views",
  "dashboard": "Executive Overview",
  "source_sheet": "Sales by Region",
  "run_on": "select",
  "auto_clear": true
}
```

This is not synthetic authoring of Tableau's persisted `Apply to Worksheets` filter-sharing state. Dashboard filter cards still expose existing worksheet Filters-shelf filters; filter actions provide the MVP cross-sheet interaction.

## Parameter actions

`create_parameter_action` writes a worksheet field value into an existing Tableau parameter.

The source worksheet must be present on the dashboard. The source field must expose a unique `column-instance`; when it has multiple instances, pass `source_instance`. Source and target datatypes must match.

Supported triggers:

- `select`
- `hover`

The MVP uses Tableau `ATTR` semantics for the source field and `do-nothing` clear-selection behavior. Aggregate Parameter Actions are outside the current authoring contract.

Example:

```json
{
  "op": "create_parameter_action",
  "caption": "Set selected region",
  "dashboard": "Executive Overview",
  "source_sheet": "Sales by Region",
  "source_field": "Region",
  "source_instance": "[none:Region:nk]",
  "target_parameter": "Selected Region",
  "run_on": "select"
}
```

## Composition

Objects that depend on newly authored objects are chained through candidate handles:

```text
open
-> create worksheets
-> create parameters
-> create dashboard using the new worksheets
-> create filter/parameter actions using the new dashboard and parameters
-> validateWorkbook
-> publish
```

This keeps each candidate reparsed and indexed before it becomes a dependency of the next edit.

## Validation boundary

Local authoring enforces supported semantic preconditions, source-preserving patch checks, and post-edit reparse checks.

The official Tableau 2026.2.0 XSD is the structural contract. TapKit does not vendor or reimplement it. Tableau REST 3.29 `validateWorkbook` remains the final XSD and semantic validation authority before publish.
