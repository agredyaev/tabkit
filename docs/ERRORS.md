# Error contract

TapKit errors have two parts:

```text
CODE: human-readable message
```

The code is stable enough for clients to branch on. The message explains the concrete failed operation or violated precondition.

For MCP errors, TapKit also places the code in:

```json
{
  "tapkit_code": "WORKSHEET_NOT_FOUND"
}
```

under the JSON-RPC error `data` field.

## MCP mapping

TapKit maps errors to standard JSON-RPC/MCP categories:

- missing workbook handle -> resource not found;
- invalid tool input or unsupported workbook/edit precondition -> invalid params;
- runtime state, transport, Tableau configuration, or upstream Tableau failures -> internal error.

The original TapKit code is preserved in `data.tapkit_code`.

## Main error groups

### Input and edit preconditions

Examples:

```text
ARGUMENT
DATASOURCE_NOT_FOUND
FIELD_NOT_FOUND
WORKSHEET_NOT_FOUND
CALCULATION_EXISTS
FILTER_FIELD_NOT_FOUND
FILTER_INSTANCE_AMBIGUOUS
DASHBOARD_EXISTS
STYLE
LAYOUT
```

Messages identify the requested object or constraint. They do not use bare object names as the full message.

### Workbook and package admission

Examples:

```text
VERSION
FORMAT
ENCODING
XML
ZIP_LIMIT
ZIP_PATH
ZIP_SYMLINK
LIMIT
STALE_BASE
PRESERVATION
```

These errors reject unsupported versions, malformed XML, unsafe/bounded package conditions, or source-preservation failures before publishing.

### Tableau REST

Network failures are separated from Tableau responses:

```text
HTTP_TIMEOUT
HTTP
REST_REJECTED
REST_SHAPE
AUTH_CONFIG
AUTH
```

If Tableau returns a structured REST error, TapKit preserves the HTTP status and available Tableau `error.code`, `summary`, and `detail` in the message.

Example:

```text
REST_REJECTED: Workbook download was rejected by Tableau
(HTTP 404, code 404002): Workbook not found: ...
```

PAT values and Tableau session tokens are never added to error text.

### Internal invariants

`INTERNAL` indicates a TapKit runtime invariant or poisoned shared state. These errors are not mapped to invalid MCP parameters.

## Lineage gaps

Lineage coverage gaps are not runtime errors. They are returned as explicit graph metadata because the workbook can still be inspected or edited.

Examples:

```text
initial_sql_lineage_unresolved
initial_sql_lineage_partial
sql_lineage_unresolved
temp_table_producer_unresolved
global_temp_table_unsupported
```

TapKit does not convert an unresolved lineage path into guessed provenance.
