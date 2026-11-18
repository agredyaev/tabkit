# Error contract

TabKit errors are stable codes plus diagnostic text:

```text
CODE: message
```

Branch on the code. Do not parse the message.

MCP also returns the code as:

```json
{ "data": { "tabkit_code": "WORKSHEET_NOT_FOUND" } }
```

## MCP mapping

| Condition | MCP category |
| --- | --- |
| missing workbook handle | resource not found |
| invalid input / unsupported edit precondition | invalid params |
| runtime state / configuration / Tableau upstream failure | internal error |

## Error groups

| Group | Common codes | Meaning |
| --- | --- | --- |
| edit/input | `ARGUMENT`, `FIELD_NOT_FOUND`, `WORKSHEET_NOT_FOUND`, `DASHBOARD_EXISTS`, `STYLE`, `LAYOUT` | request cannot be applied to this workbook |
| package/XML | `VERSION`, `FORMAT`, `ENCODING`, `XML`, `ZIP_LIMIT`, `STALE_BASE`, `PRESERVATION` | workbook/package admission or preservation failure |
| Tableau REST | `HTTP_TIMEOUT`, `HTTP`, `REST_REJECTED`, `REST_SHAPE`, `AUTH_CONFIG`, `AUTH` | network, authentication, or Tableau response failure |
| runtime | `INTERNAL` | TabKit invariant/state failure |

Tableau structured REST failures preserve HTTP status and available Tableau `error.code`,
`summary`, and `detail`. PAT values and Tableau session tokens are not included in error text.

## Lineage gaps are not errors

A workbook can remain usable when provenance is incomplete. Lineage therefore returns coverage gaps
inside the graph result instead of failing the operation.

Common gap codes:

```text
initial_sql_lineage_unresolved
initial_sql_lineage_partial
sql_lineage_unresolved
temp_table_producer_unresolved
global_temp_table_unsupported
relation_shape_unsupported
worksheet_field_unresolved
dashboard_sheet_unresolved
```

A gap means TabKit could not prove that lineage segment. It does not invent a replacement path.
