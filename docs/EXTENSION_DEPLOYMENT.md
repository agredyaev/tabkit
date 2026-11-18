# TapKit extension deployment

This guide deploys the TapKit Dashboard Extension host for Tableau Server 2026.2.4.

The production deployment uses one stable extension URL for every TapKit web dashboard:

    https://tableau-mcp.company.net/tapkit/host.html

Dashboard-specific HTML, CSS, and JavaScript stay in the workbook. The host is a generic runtime and bridge.

## Release artifacts

CI and GitHub releases produce separate deployment ZIPs:

    tabkit-mcp-v0.9.0-linux-x64.zip
    tabkit-mcp-v0.9.0-windows-x64.zip
    tabkit-extension-host-v0.9.0-linux-x64.zip
    tabkit-extension-host-v0.9.0-windows-x64.zip

The extension-host binary embeds host.html, host.css, host.js, and the pinned Tableau Extensions API runtime. No Node.js, Python, IIS static file site, or separate asset directory is required.

## Runtime contract

Each authored web dashboard stores:

    tapkit.bundle_b64
    tapkit.bundle_mac

The decoded bundle is JSON with runtime_version, html, css, and javascript fields. Runtime version 1 is the current contract.

The public URL stays stable. Future compatible runtimes are selected by runtime_version; do not create /v1/host.html, /v2/host.html, and similar URLs unless the compatibility model changes.

Production bundles are HMAC-SHA256 signed. The same 32-byte signing key must be configured on the TapKit process that authors the workbook and on tabkit-extension-host.

The signing key is never stored in the workbook and is never sent to the browser.

Treat access to the TapKit authoring/signing process as a publisher privilege. Any caller that can submit `create_web_dashboard` to a signer can create code that will later run with the extension permissions of dashboard viewers. Do not expose the signing-enabled MCP endpoint to ordinary workbook viewers or untrusted authors.

## 1. Generate the signing key

Generate one random 32-byte value and store it in the normal server secret store.

### Windows PowerShell

    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $key = New-Object byte[] 32
    $rng.GetBytes($key)
    $secret = [Convert]::ToBase64String($key)
    $secret

### Linux

    openssl rand -base64 32

Configure the resulting base64 value as:

    TABKIT_EXTENSION_SIGNING_KEY

Do not put the key in Git, a workbook, a TREX file, a command line, or the reverse-proxy configuration.

Changing the key invalidates signatures in already authored workbooks. Re-sign or recreate affected web dashboards before rotating the key.

## 2. Configure the TapKit authoring process

The process that executes create_web_dashboard must receive the same key.

### Windows

    $env:TABKIT_EXTENSION_SIGNING_KEY="<base64-key>"

Then start or restart tabkit-mcp.exe using the existing MCP configuration.

### Linux

    export TABKIT_EXTENSION_SIGNING_KEY='<base64-key>'

A remote extension_url is rejected during authoring when the key is missing. Unsigned bundles are accepted only for localhost development.

## 3. Start the extension host

### Windows

Extract:

    tabkit-extension-host-v0.9.0-windows-x64.zip

Example directory:

    C:\TapKit\ExtensionHost\
      tabkit-extension-host.exe
      EXTENSION_DEPLOYMENT.md
      LICENSE
      README.md

Set the signing key in the service environment and start the host:

    $env:TABKIT_EXTENSION_SIGNING_KEY="<base64-key>"

    C:\TapKit\ExtensionHost\tabkit-extension-host.exe --bind 127.0.0.1:8765 --base-path /tapkit

The local endpoints are:

    http://127.0.0.1:8765/tapkit/host.html
    http://127.0.0.1:8765/tapkit/healthz

/healthz must return:

    ok

### Linux

    export TABKIT_EXTENSION_SIGNING_KEY='<base64-key>'
    ./tabkit-extension-host --bind 127.0.0.1:8765 --base-path /tapkit

## 4. Put HTTPS in front of the host

The binary deliberately serves HTTP. Terminate TLS at the existing corporate reverse proxy, load balancer, IIS reverse proxy, ingress, or gateway.

Recommended topology:

    user browser
        |
        | HTTPS
        v
    https://tableau-mcp.company.net/tapkit/host.html
        |
        | reverse proxy
        v
    http://127.0.0.1:8765/tapkit/host.html
        |
        v
    tabkit-extension-host

The browser running Tableau must be able to resolve and open the public HTTPS URL. It is not sufficient for only Tableau Server to reach the host.

Keep the backend listener on loopback when the reverse proxy is on the same machine. If a remote load balancer must reach the process, bind to the required private interface and restrict the port at the firewall.

The host refuses a non-loopback bind when TABKIT_EXTENSION_SIGNING_KEY is missing.

### Reverse-proxy requirements

Preserve the /tapkit prefix when --base-path /tapkit is used.

Proxy at least:

    GET  /tapkit/host.html
    GET  /tapkit/host.css
    GET  /tapkit/host.js
    GET  /tapkit/vendor/tableau.extensions.1.17.0.min.js
    POST /tapkit/verify
    GET  /tapkit/healthz

Do not cache /verify. Do not add authentication redirects in front of host.html; iframe login redirects commonly break Tableau extension initialization. Use network controls, Tableau safe-listing, and the signed-bundle boundary instead.

The public production URL must use a certificate trusted by user browsers.

## 5. Verify the public host

From a normal workstation that will use Tableau, open:

    https://tableau-mcp.company.net/tapkit/healthz

Expected response:

    ok

Then open:

    https://tableau-mcp.company.net/tapkit/host.html

Outside Tableau, the page can report that the Tableau extension context is unavailable. That is expected. The important checks are successful HTTPS loading, a trusted certificate, and no authentication redirect.

## 6. Configure Tableau Server

TapKit is a network-enabled Dashboard Extension.

Tableau Server requires network-enabled extensions to be explicitly safe-listed. Tableau also requires explicit permission for Full Data Access when an extension reads underlying data, table names, field names, or datasource metadata.

In Tableau Server:

1. Open the site administration page.
2. Go to Settings > Extensions.
3. Ensure Allow extensions to run on this site is enabled.
4. Under Enable Specific Extensions, add the exact host URL:

       https://tableau-mcp.company.net/tapkit/host.html

5. Set Full Data Access to Allow.
6. Choose whether Tableau should Show or Hide the user permission prompt according to the site policy.
7. Check the server-level extension block list. A global block entry overrides a site safe-list entry.

Prefer the exact URL. Do not safe-list a broad domain wildcard unless the same trust decision applies to every extension under that pattern.

Current Tableau documentation:

- https://help.tableau.com/current/server/en-us/dashboard_extensions_server.htm
- https://help.tableau.com/current/server/en-us/security_extensions.htm

## 7. Author TapKit web dashboards with the stable URL

Use the same URL for all TapKit web dashboards. For example, create_web_dashboard should use:

    extension_url = https://tableau-mcp.company.net/tapkit/host.html

TapKit writes runtime_version 1, signs the serialized bundle, and stores the bundle plus MAC in the workbook.

One host serves any number of dashboards and workbooks:

    Sales workbook       -> same /tapkit/host.html -> signed bundle A
    Finance workbook     -> same /tapkit/host.html -> signed bundle B
    Operations workbook  -> same /tapkit/host.html -> signed bundle C

## 8. Publish and validate

For the target Tableau Server 2026.2.4 environment, use the normal TapKit sequence:

    create_web_dashboard
    -> validateWorkbook
    -> publish
    -> open dashboard in a browser
    -> verify extension load
    -> verify Full Data Access
    -> verify datasource/logical-table reads
    -> download/read back the workbook if preservation needs to be checked

Do not treat local XML validation as proof of Tableau runtime behavior. validateWorkbook, publish, and browser execution on the target server remain the final integration checks.

## 9. Security behavior

The production path enforces these boundaries:

- remote extension URLs require HTTPS;
- URL parsing rejects user-info tricks, malformed hostless URLs, and remote HTTP;
- production dashboard bundles require a valid HMAC-SHA256 signature;
- raw workbook edits cannot forge a valid bundle without the signing key;
- bundle size is bounded before and after base64 decoding;
- HTML is sanitized before it enters the sandboxed document;
- inline event handlers, embedded scripts, forms, frames, objects, and remote element URLs are removed from authored HTML;
- generated JavaScript does not receive tableau.extensions;
- Tableau access is exposed only through the explicit window.tapkit bridge;
- bridge requests are size-bounded, concurrency-bounded, and time-bounded;
- data reads are row-bounded and byte-bounded;
- the child CSP blocks fetch/XHR/WebSocket and external subresources;
- navigation is blocked where the browser Navigation API exposes a cancelable navigation event;
- the signed-bundle trust boundary remains authoritative for arbitrary generated JavaScript.

Do not describe the child iframe as an absolute network security boundary. Browser document navigation cannot be universally prohibited by CSP. Production safety relies on executing only bundles signed by the trusted TapKit authoring path.

## 10. Local development

For unsigned localhost development, the static files can still be served directly:

    python -m http.server 8765 --directory extension

Use:

    http://localhost:8765/host.html

Unsigned execution is accepted only when the browser itself sees a loopback hostname. This mode is not for shared environments.

The production tabkit-extension-host should be used for any remote Tableau deployment.

## Troubleshooting

### Tableau shows a blank or blocked extension

Check:

- the public URL is HTTPS;
- the browser can open the URL directly;
- the certificate is trusted;
- the exact URL is in Settings > Extensions > Enable Specific Extensions;
- Full Data Access is allowed;
- the URL is not present on the global block list.

### TapKit dashboard signature is missing

The workbook was created without a production signature. Configure TABKIT_EXTENSION_SIGNING_KEY on the authoring process and recreate or re-sign the web dashboard.

### TapKit dashboard signature verification failed

The host and authoring process are using different signing keys, or the bundle was changed after signing.

### Signature verification endpoint is unavailable

Check that the reverse proxy forwards:

    POST /tapkit/verify

to the extension-host process and does not redirect it to an authentication page.

### Unsupported TapKit dashboard runtime version

The workbook was authored for a runtime newer than the deployed host. Upgrade tabkit-extension-host. The public host.html URL does not need to change.

### Host starts locally but refuses a remote bind

Configure a valid base64 32-byte TABKIT_EXTENSION_SIGNING_KEY. Non-loopback unsigned hosting is rejected.

### Data calls fail after the extension loads

Check Tableau Full Data Access and the user's Tableau permissions. The extension does not bypass workbook, datasource, or user authorization.
