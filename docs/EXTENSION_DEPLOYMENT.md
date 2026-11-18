# TabKit extension host deployment

This guide deploys the TabKit HTML dashboard runtime on Windows.

The host can run on the same Windows VM as the official Tableau MCP server. They are independent processes. They only need different local ports.

## What to deploy

Use:

    tabkit-extension-host-v0.9.0-windows-x64.zip

The host is the shared runtime for HTML dashboards created by TabKit. One host URL can serve any number of TabKit workbooks.

Production does not require installing the checked-in `.trex` file. TabKit writes the extension reference and production URL into the workbook. The `.trex` file is for localhost development.

## 1. Configure the signing key

Create one random 32-byte value and store its base64 form as:

    TABKIT_EXTENSION_SIGNING_KEY

The same value must be configured on:

- the TabKit process that creates the HTML dashboard;
- `tabkit-extension-host.exe`.

Do not give this key to the official Tableau MCP server. Tableau MCP is not part of TabKit bundle signing.

## 2. Start the host

Extract the ZIP and run:

    $env:TABKIT_EXTENSION_SIGNING_KEY="<base64-key>"
    .\tabkit-extension-host.exe --bind 127.0.0.1:8765 --base-path /tabkit

Local endpoints:

    http://127.0.0.1:8765/tabkit/healthz
    http://127.0.0.1:8765/tabkit/host.html

`/healthz` must return:

    ok

Running the official Tableau MCP on the same VM is fine. Keep the two services on different local ports.

## 3. Expose the host through HTTPS

Use the VM's existing reverse proxy, gateway, IIS proxy, or load balancer.

Example:

    https://tableau-mcp.company.net/tabkit/*
        ->
    http://127.0.0.1:8765/tabkit/*

Preserve the `/tabkit` path.

The following routes must work:

    GET  /tabkit/host.html
    GET  /tabkit/host.css
    GET  /tabkit/host.js
    GET  /tabkit/vendor/tableau.extensions.1.17.0.min.js
    POST /tabkit/verify
    GET  /tabkit/healthz

Do not put an interactive login redirect in front of these routes.

From a normal user workstation, verify:

    https://tableau-mcp.company.net/tabkit/healthz

Expected response:

    ok

The public URL must use HTTPS and a certificate trusted by user browsers.

## 4. Allow the extension in Tableau Server

In the Tableau Server site:

    Settings -> Extensions

Then:

1. enable extensions for the site;
2. add the exact URL:

       https://tableau-mcp.company.net/tabkit/host.html

3. set Full Data Access to Allow;
4. confirm the URL is not blocked by the server-level extension block list.

The browser that opens the Tableau dashboard must be able to reach the public host URL.

## 5. Use the URL from TabKit

When TabKit creates an HTML dashboard, use:

    extension_url = https://tableau-mcp.company.net/tabkit/host.html

TabKit stores the dashboard HTML, CSS, and JavaScript inside the workbook as a signed bundle. The host loads and runs that bundle.

The public host URL stays the same for all TabKit HTML dashboards.

## Test flow

    start host
    -> check /healthz
    -> add host.html to Tableau Server Extensions safe list
    -> allow Full Data Access
    -> create HTML dashboard with TabKit
    -> validateWorkbook
    -> publish
    -> open the dashboard in a browser

## Official Tableau documentation

- Manage Dashboard and Viz Extensions in Tableau Server:
  https://help.tableau.com/current/server/en-us/dashboard_extensions_server.htm
- Extension security:
  https://help.tableau.com/current/server/en-us/security_extensions.htm
- Tableau Extension manifest:
  https://tableau.github.io/extensions-api/docs/dashext/trex_manifest/
- Full Data Access:
  https://tableau.github.io/extensions-api/docs/security/trex_data_access/
