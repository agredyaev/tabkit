# Web dashboard extension

TapKit web dashboards use one Tableau Dashboard Extension object as the presentation surface.

```text
Tableau workbook
  -> datasource(s)
  -> dashboard
       -> TapKit Web Dashboard extension
            -> sandboxed HTML/CSS/JavaScript
```

The web-dashboard path does not use TapKit's native dashboard row/column layout compiler. Tableau still owns the workbook, data sources, permissions, parameters, publish lifecycle, and extension host. HTML/CSS/JavaScript owns presentation and local interaction.

## Extension runtime

The static extension host is under `extension/`:

```text
extension/
  tapkit-web-dashboard.trex
  host.html
  host.css
  host.js
  vendor/
    tableau.extensions.1.latest.min.js
```

For local development:

```sh
python -m http.server 8765 --directory extension
```

The development manifest points to `http://localhost:8765/host.html`. Remote deployment requires HTTPS. Tableau Server/Cloud may also require extensions to be enabled and the host to be safe-listed.

## Authoring contract

`create_web_dashboard` creates a fixed-size dashboard containing one full-canvas TapKit Dashboard Extension object.

```json
{
  "op": "create_web_dashboard",
  "name": "Executive Overview",
  "width": 1440,
  "height": 900,
  "extension_url": "https://bi.example.com/tapkit/host.html",
  "html": "<main id=\"app\"></main>",
  "css": "#app { width: 100vw; height: 100vh; }",
  "javascript": "const sources = await tapkit.listDataSources();"
}
```

`extension_url` must use HTTPS. HTTP is accepted only for localhost development.

HTML, CSS, and JavaScript are serialized as UTF-8 JSON, base64 encoded, and persisted in extension instance settings under `tapkit.bundle_b64`. The combined unencoded payload is limited to 1 MiB.

## Sandbox boundary

Generated code runs in an iframe with `sandbox="allow-scripts"`. It is not granted same-origin access. The child CSP denies network, object, frame, form, and base-URI access. Generated code cannot call `tableau.extensions` directly.

The trusted host exposes a narrow message bridge as `window.tapkit`.

## Bridge API

Data-source access works without hidden worksheet zones:

```text
listDataSources()
listLogicalTables(datasource)
getLogicalTableData(datasource, table, maxRows?)
```

Logical-table reads are capped at 100,000 rows per request. They use Tableau full-data APIs, so the extension manifest declares `full data` permission and the user still needs the corresponding Tableau permission.

Hybrid dashboards may also use worksheet APIs when worksheets are present on the dashboard:

```text
listWorksheets()
getSummaryData(worksheet, maxRows?)
applyFilter(worksheet, field, values)
clearFilter(worksheet, field)
```

Parameter access:

```text
listParameters()
setParameter(name, value)
```

Generated code can subscribe through `tapkit.on(event, handler)`. Current emitted worksheet events are filter, mark-selection, and summary-data changes when worksheets exist on the dashboard.

## Security invariants

- generated code never receives the Tableau Extensions API object;
- child network access is denied by CSP;
- the extension URL is HTTPS except for loopback development;
- inline payload size is bounded before workbook mutation;
- Tableau full-data permission remains explicit in the extension manifest;
- workbook XML stores only base64-encoded bundle bytes, not executable XML fragments;
- Tableau `validateWorkbook` remains the workbook validation authority.

## Native authoring

Native Tableau authoring remains available for workbooks that require ordinary editable Tableau worksheets, native dashboard layout, filter cards, or native actions.

The web-dashboard path is the preferred path when the dashboard is intentionally one custom HTML/JavaScript application.
