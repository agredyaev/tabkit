# Web dashboard extension

TabKit web dashboards use one Tableau Dashboard Extension object as the presentation surface.

    Tableau workbook
      -> datasource(s)
      -> dashboard
           -> TabKit Web Dashboard extension
                -> signed HTML/CSS/JavaScript bundle

The web-dashboard path does not use the native row/column dashboard compiler. Tableau still owns the workbook, data sources, permissions, parameters, publish lifecycle, and extension zone. HTML/CSS/JavaScript owns presentation and local interaction.

## Stable host URL

All compatible dashboards use one stable URL, for example:

    https://bi.example.com/tabkit/host.html

Do not create a different URL for each dashboard and do not version the public path for normal runtime changes.

The bundle itself contains:

    runtime_version = 1

Future host releases can keep supporting older runtime versions at the same host.html URL.

## Extension runtime

The production runtime is delivered as a dedicated binary:

    tabkit-extension-host

It embeds:

    host.html
    host.css
    host.js
    vendor/tableau.extensions.1.17.0.min.js

The source files remain under extension/ for development and review.

For local development:

    python -m http.server 8765 --directory extension

The development manifest points to:

    http://localhost:8765/host.html

Remote production deployment requires HTTPS.

See EXTENSION_DEPLOYMENT.md for the full Windows/Linux deployment and Tableau Server configuration procedure.

## Authoring contract

create_web_dashboard creates a fixed-size dashboard containing one full-canvas TabKit Dashboard Extension object.

Example request:

    {
      "op": "create_web_dashboard",
      "name": "Executive Overview",
      "width": 1440,
      "height": 900,
      "extension_url": "https://bi.example.com/tabkit/host.html",
      "html": "<main id=\"app\"></main>",
      "css": "#app { width: 100vw; height: 100vh; }",
      "javascript": "const sources = await tabkit.listDataSources();"
    }

extension_url must be a valid absolute URL. Remote URLs must use HTTPS. HTTP is accepted only for localhost, 127.0.0.1, or ::1. User-info URLs, malformed hostless URLs, and fragments are rejected.

HTML, CSS, and JavaScript are serialized as UTF-8 JSON with runtime_version, base64 encoded, and persisted under:

    tabkit.bundle_b64

Production also stores:

    tabkit.bundle_mac

The unencoded HTML/CSS/JavaScript payload is limited to 1 MiB. The serialized bundle is limited separately before it reaches the runtime.

## Bundle signing

Remote production authoring requires:

    TABKIT_EXTENSION_SIGNING_KEY

The value is base64 for exactly 32 random bytes.

TabKit signs the exact serialized bundle with HMAC-SHA256. tabkit-extension-host receives the same key and verifies the MAC through its same-origin /verify endpoint before host.js decodes or executes the dashboard.

This prevents a workbook author from editing tabkit.bundle_b64 and running arbitrary unsigned JavaScript under a safe-listed TabKit extension URL.

Unsigned bundles are accepted only for loopback development.

## Sandbox and browser boundary

After signature verification:

1. host.js decodes and validates the versioned bundle;
2. raw HTML is parsed and sanitized;
3. script, iframe, frame, object, embed, link, meta, base, and form nodes are removed;
4. inline event-handler and navigation-capable attributes are removed;
5. CSS is escaped so it cannot terminate the trusted style element;
6. the result is loaded into an iframe with sandbox="allow-scripts";
7. generated JavaScript runs after the trusted child bridge bootstrap.

The child is not granted same-origin access and does not receive tableau.extensions directly.

The child CSP denies external connections and external subresources. Navigation is also canceled where the browser Navigation API exposes a cancelable event.

The iframe/CSP layer is defense in depth, not the primary trust boundary. CSP cannot universally prohibit every document-navigation path in every browser. Production safety therefore depends on executing only bundles signed by the trusted TabKit authoring path.

## Bridge API

Data-source access works without hidden worksheet zones:

    listDataSources()
    listLogicalTables(datasource)
    getLogicalTableData(datasource, table, maxRows?)

Logical-table reads use Tableau full-data APIs. The extension manifest therefore declares Full Data Access and Tableau still enforces user/workbook/data-source permissions.

Hybrid dashboards may also use:

    listWorksheets()
    getSummaryData(worksheet, maxRows?)
    applyFilter(worksheet, field, values)
    clearFilter(worksheet, field)

Parameter access:

    listParameters()
    setParameter(name, value)

Events:

    tabkit.on("filterChanged", handler)
    tabkit.on("selectionChanged", handler)
    tabkit.on("summaryDataChanged", handler)

## Runtime limits

Current runtime limits include:

- 100,000 rows per data request;
- 8 MiB approximate returned row-data budget;
- 16 MiB maximum bridge response serialization;
- 64 KiB maximum bridge request serialization;
- four concurrent bridge requests;
- 30 second host-side bridge timeout;
- 35 second child-side request timeout;
- 1 MiB HTML/CSS/JavaScript payload;
- 2 MiB serialized bundle.

Summary data is read page-by-page. The runtime no longer calls getAllPagesAsync before applying its row limit.

## Security invariants

- remote extension URLs require HTTPS;
- production bundles require a valid signature;
- the signing key is not stored in the workbook or browser;
- generated code does not receive tableau.extensions;
- Tableau access is limited to the explicit bridge allowlist;
- bridge request size, response size, concurrency, timeout, row count, and data bytes are bounded;
- generated HTML is sanitized before it is placed in the child document;
- the child has no same-origin permission;
- external fetch/XHR/WebSocket and external subresources are denied by child CSP;
- the host emits CSP, nosniff, no-referrer, no-store, and restrictive Permissions-Policy headers;
- Tableau Full Data Access remains explicit in the manifest and safe-list configuration;
- Tableau validateWorkbook remains the final workbook validation authority.

## Native authoring

Native Tableau authoring remains available for workbooks that require editable Tableau worksheets, native dashboard layout, filter cards, or native actions.

The web-dashboard path is intended for dashboards whose presentation surface is one custom web application.
