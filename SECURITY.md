# Security policy

Security fixes target the current `main` branch and the latest release.

Report vulnerabilities through GitHub private vulnerability reporting:

https://github.com/agredyaev/tabkit/security/advisories

Include:
- affected version or commit;
- impact;
- minimal reproduction;
- relevant logs with secrets removed.

Do not include Tableau credentials, PAT values, private workbook data, or other production secrets.

Do not open a public issue for an unpatched vulnerability.

## Web dashboard extension

Remote TabKit web dashboards execute generated JavaScript only after bundle signature verification.

Production authoring requires a base64 32-byte TABKIT_EXTENSION_SIGNING_KEY. TabKit signs the exact serialized dashboard bundle with HMAC-SHA256 and stores only the bundle plus MAC in the workbook. tabkit-extension-host verifies the MAC server-side before browser execution. Unsigned bundles are accepted only for loopback development.

Generated code runs in a sandboxed iframe without same-origin permission and does not receive the Tableau Extensions API object. Tableau access is mediated by the explicit host bridge. HTML is sanitized before sandbox load, bridge traffic is bounded, and the child CSP blocks external connections and subresources.

The sandbox and CSP add isolation but do not replace signature verification. Production execution requires bundles signed by the configured TabKit authoring key.

Remote extension hosts must use HTTPS. Direct logical-table reads require Tableau Full Data Access and remain subject to Tableau user, workbook, and datasource permissions.

See docs/EXTENSION_DEPLOYMENT.md for deployment and safe-list configuration.
