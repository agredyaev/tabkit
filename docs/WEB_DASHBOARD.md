# Web dashboard extension

TabKit web dashboards use one full-canvas Tableau Dashboard Extension as the presentation surface.

    Tableau workbook
      -> TabKit Web Dashboard extension
           -> signed HTML/CSS/JavaScript bundle

Tableau owns the workbook, data sources, permissions, parameters, publish lifecycle, and extension zone. HTML/CSS/JavaScript owns presentation and local interaction.

## Host URL

All TabKit HTML dashboards use one stable HTTPS URL, for example:

    https://tableau-mcp.company.net/tabkit/host.html

The Windows extension host can run on the same VM as the official Tableau MCP server. They are independent services and use different local ports.

See [Extension deployment](EXTENSION_DEPLOYMENT.md) for setup.

## Authoring

`create_web_dashboard` creates a fixed-size dashboard containing one full-canvas TabKit Dashboard Extension.

Example:

    {
      "op": "create_web_dashboard",
      "name": "Executive Overview",
      "width": 1440,
      "height": 900,
      "extension_url": "https://tableau-mcp.company.net/tabkit/host.html",
      "html": "<main id=\"app\"></main>",
      "css": "#app { width: 100vw; height: 100vh; }",
      "javascript": "const sources = await tabkit.listDataSources();"
    }

Remote extension URLs must use HTTPS.

TabKit stores the signed runtime bundle in workbook extension settings:

    tabkit.bundle_b64
    tabkit.bundle_mac

Production authoring and `tabkit-extension-host` must use the same base64 32-byte `TABKIT_EXTENSION_SIGNING_KEY`.

Production does not require installing the development `.trex` file. TabKit writes the referenced extension manifest and production URL into the workbook.

## Bridge API

Generated JavaScript can use:

    listDataSources()
    listLogicalTables(datasource)
    getLogicalTableData(datasource, table, maxRows?)
    listWorksheets()
    getSummaryData(worksheet, maxRows?)
    applyFilter(worksheet, field, values)
    clearFilter(worksheet, field)
    listParameters()
    setParameter(name, value)
    on(event, handler)

Generated JavaScript does not receive direct access to `tableau.extensions`.

## Limits

Current runtime limits:

- 100,000 rows per data request;
- 8 MiB approximate returned row-data budget;
- 16 MiB maximum bridge response serialization;
- 64 KiB maximum bridge request serialization;
- four concurrent bridge requests;
- 30 second host-side timeout;
- 35 second child-side timeout;
- 1 MiB HTML/CSS/JavaScript payload;
- 2 MiB serialized bundle.

## Security boundary

Remote URLs require HTTPS. Production bundles require a valid signature. Tableau Full Data Access remains explicit and Tableau continues to enforce workbook, datasource, and user permissions.

The sandbox and CSP are defense in depth. The production trust boundary is the signed bundle created by the trusted TabKit authoring path.
