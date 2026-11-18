# TabKit web-dashboard runtime

Browser assets and development manifest for `tabkit-extension-host`.

## Files

| Path | Purpose |
| --- | --- |
| `host.html` | extension entry point |
| `host.css` | host status/frame styles |
| `host.js` | bundle verification, sandbox, Tableau bridge |
| `vendor/tableau.extensions.1.17.0.min.js` | vendored Tableau Extensions API |
| `tabkit-web-dashboard.trex` | localhost development manifest only |

## Production

Use the Windows release artifact:

```text
tabkit-extension-host-v<version>-windows-x64.zip
```

Start:

```powershell
$env:TABKIT_EXTENSION_SIGNING_KEY="<base64-key>"
.\tabkit-extension-host.exe --bind 127.0.0.1:8765 --base-path /tabkit
```

Expose `/tabkit/*` through HTTPS and allow the final `host.html` URL in Tableau Server with
Full Data Access.

Production does not require installing `tabkit-web-dashboard.trex`.

Full procedure: [Extension host deployment](../docs/EXTENSION_DEPLOYMENT.md).

## Development

Unsigned bundles are accepted only on loopback.

```bash
python -m http.server 8765 --directory extension
```

Development URL:

```text
http://localhost:8765/host.html
```

## Workbook settings

TabKit stores:

| Key | Content |
| --- | --- |
| `tabkit.bundle_b64` | base64 runtime bundle |
| `tabkit.bundle_mac` | HMAC-SHA256 signature; empty only for localhost development |

Runtime/bridge contract: [Web dashboards](../docs/WEB_DASHBOARD.md).
