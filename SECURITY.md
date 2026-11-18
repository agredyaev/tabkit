# Security policy

Security fixes target the current `main` branch and latest release.

## Report a vulnerability

Use GitHub private vulnerability reporting:

https://github.com/agredyaev/tabkit/security/advisories

Include:

- affected version or commit;
- impact;
- minimal reproduction;
- relevant logs with secrets removed.

Do **not** include Tableau credentials, PAT values, private workbook data, signing keys, or other
production secrets. Do not open a public issue for an unpatched vulnerability.

## Web-dashboard security boundary

| Control | Enforcement |
| --- | --- |
| bundle integrity | HMAC-SHA256 signature verified before execution |
| signing key | base64 32-byte `TABKIT_EXTENSION_SIGNING_KEY`; shared only by authoring and host processes |
| unsigned bundles | loopback development only |
| generated code | sandboxed iframe; no raw `tableau.extensions` object |
| Tableau access | explicit host bridge methods |
| HTML | sanitized before iframe load |
| bridge | request/response/concurrency/time limits |
| child network | blocked by CSP |
| remote host | HTTPS |
| underlying data | Tableau Full Data Access + Tableau user/workbook/datasource permissions |

The sandbox and CSP are isolation layers; they do not replace bundle signature verification.

Deployment and Tableau allow-list configuration:
[Extension host deployment](docs/EXTENSION_DEPLOYMENT.md).
