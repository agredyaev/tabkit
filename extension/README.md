# TabKit Web Dashboard extension

This directory contains the browser runtime embedded by `tabkit-extension-host` and the localhost development TREX manifest.

## Production

Use:

    tabkit-extension-host-v0.9.0-windows-x64.zip

The host can run on the same Windows VM as the official Tableau MCP server. They are independent processes and must use different local ports.

Example:

    tabkit-extension-host.exe --bind 127.0.0.1:8765 --base-path /tabkit

Expose `/tabkit/*` through HTTPS, safe-list the public `host.html` URL in Tableau Server, and allow Full Data Access.

Production does not require installing `tabkit-web-dashboard.trex`. TabKit writes the extension reference and URL directly into the workbook.

Configure the same base64 32-byte `TABKIT_EXTENSION_SIGNING_KEY` on the TabKit authoring process and on `tabkit-extension-host`.

See [docs/EXTENSION_DEPLOYMENT.md](../docs/EXTENSION_DEPLOYMENT.md).

## Development

Unsigned execution is allowed only on loopback.

Serve this directory locally:

    python -m http.server 8765 --directory extension

The development manifest points to:

    http://localhost:8765/host.html

## Runtime contract

TabKit stores:

- `tabkit.bundle_b64`
- `tabkit.bundle_mac`

The decoded bundle contains `runtime_version`, `html`, `css`, and `javascript`.

Generated JavaScript receives the bounded `window.tabkit` API rather than direct access to `tableau.extensions`.

## Vendored dependency

`vendor/tableau.extensions.1.17.0.min.js` is taken from the official Tableau Extensions API v1.17.0 release tag. The upstream Apache-2.0 license is retained as `vendor/LICENSE-tableau-extensions-api.txt`.
