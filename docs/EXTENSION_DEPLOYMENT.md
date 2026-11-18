# TabKit extension host: Windows deployment

Use this guide to deploy the TabKit HTML dashboard runtime on a Windows VM.

The host can run on the same VM as the official Tableau MCP server. They are independent processes. The official Tableau MCP does not need to be changed; the two services only need different local ports.

## Goal

At the end of this procedure, this URL must be available:

    https://<public-host>/tabkit/host.html

TabKit will use that value as `extension_url`.

## Inputs

Required:

- `tabkit-extension-host-v<version>-windows-x64.zip` from https://github.com/agredyaev/tabkit/releases/latest;
- access to the Windows VM;
- a public HTTPS hostname already used by the environment, or another approved HTTPS endpoint for the VM;
- ability to add a route in the existing HTTPS reverse proxy/gateway;
- Tableau Server site-administrator access, or a Tableau administrator who can update extension settings;
- one base64-encoded 32-byte `TABKIT_EXTENSION_SIGNING_KEY`.

The signing key must be the same on the TabKit authoring process and on `tabkit-extension-host.exe`. The official Tableau MCP does not use this key.

If no key has been provided, generate one in PowerShell:

    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $key = New-Object byte[] 32
    $rng.GetBytes($key)
    [Convert]::ToBase64String($key)

Store and transfer the value through an approved secret store or secure channel. Do not put it in Git, the workbook, or the public URL.

## 1. Start the host

Extract:

    tabkit-extension-host-v<version>-windows-x64.zip

Open PowerShell in the extracted directory and run:

    $env:TABKIT_EXTENSION_SIGNING_KEY="<base64-key>"
    .\tabkit-extension-host.exe --bind 127.0.0.1:8765 --base-path /tabkit

The process must stay running. For persistent use, run it under the VM's existing process supervisor. `tabkit-extension-host.exe` does not install itself as a Windows service.

Verify locally on the VM:

    (Invoke-WebRequest http://127.0.0.1:8765/tabkit/healthz).Content

Expected body:

    ok

The official Tableau MCP can continue running on the same VM on its existing port.

## 2. Publish the host through HTTPS

The host listens on localhost HTTP. Add one route to the VM's existing HTTPS reverse proxy or gateway:

    https://<public-host>/tabkit/*
        ->
    http://127.0.0.1:8765/tabkit/*

Preserve the `/tabkit` path.

These routes must pass through:

    GET  /tabkit/host.html
    GET  /tabkit/host.css
    GET  /tabkit/host.js
    GET  /tabkit/vendor/tableau.extensions.1.17.0.min.js
    POST /tabkit/verify
    GET  /tabkit/healthz

Do not put an interactive login redirect in front of these routes.

From a workstation that will open Tableau dashboards, verify:

    (Invoke-WebRequest https://<public-host>/tabkit/healthz).Content

Expected body:

    ok

The HTTPS certificate must be trusted by the user browser.

## 3. Allow the extension in Tableau Server

Open the Tableau Server site:

    Settings -> Extensions

Then:

1. enable extensions for the site;
2. under **Enable Specific Extensions**, add the exact URL:

       https://<public-host>/tabkit/host.html

3. set **Full Data Access** to **Allow**;
4. choose **Show** or **Hide** for the user permission prompt according to site policy;
5. confirm that the same URL is not blocked by the server-level extension block list.

Use the exact URL rather than a broad wildcard unless the wider trust scope is intentional.

Official Tableau documentation:

- Manage Dashboard and Viz Extensions in Tableau Server:
  https://help.tableau.com/current/server/en-us/dashboard_extensions_server.htm
- Extension security:
  https://help.tableau.com/current/server/en-us/security_extensions.htm
- HTTPS and extension source locations:
  https://tableau.github.io/extensions-api/docs/security/trex_security/
- Full Data Access:
  https://tableau.github.io/extensions-api/docs/security/trex_data_access/

Tableau documents Network-enabled extensions as web applications that must be explicitly allowed on the site. Tableau Server accepts Network-enabled extensions over HTTPS, except for localhost development, and Full Data Access must be explicitly allowed when the extension requires underlying data or datasource metadata.

## 4. Return the deployment result

Send the requester:

    Extension URL:
    https://<public-host>/tabkit/host.html

    Health URL:
    https://<public-host>/tabkit/healthz

Also confirm:

- health URL returns `ok` from a workstation that will open Tableau dashboards;
- the extension URL is present in the Tableau Server site safe list;
- Full Data Access is set to Allow;
- the URL is not on the server block list;
- the host is using the agreed `TABKIT_EXTENSION_SIGNING_KEY`.

Do not send the signing key in the deployment confirmation.

## Definition of Done

The host is ready when all of these are true:

- [ ] `tabkit-extension-host.exe` is running on the Windows VM;
- [ ] `http://127.0.0.1:8765/tabkit/healthz` returns `ok` on the VM;
- [ ] `https://<public-host>/tabkit/healthz` returns `ok` from a user workstation;
- [ ] `https://<public-host>/tabkit/host.html` opens without an authentication redirect or certificate error;
- [ ] the exact `host.html` URL is allowed in Tableau Server **Settings -> Extensions**;
- [ ] Full Data Access is **Allow**;
- [ ] the URL is not blocked at server level;
- [ ] the same signing key is configured on the host and the TabKit authoring process.

Once these checks pass, return the `host.html` URL. No additional TabKit host settings are required.

## What TabKit does with the URL

TabKit creates the Dashboard Extension reference inside the workbook and stores the dashboard HTML, CSS, and JavaScript as a signed workbook bundle.

Production deployment does not require installing the checked-in `.trex` file. That file is for localhost development.

One deployed host can serve multiple TabKit workbooks and dashboards.
