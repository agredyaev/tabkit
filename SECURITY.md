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

Generated web-dashboard HTML/CSS/JavaScript runs in a sandboxed iframe without same-origin or network permission. It does not receive the Tableau Extensions API object. Tableau access is mediated by the trusted host bridge. Remote extension hosts must use HTTPS; localhost HTTP is development-only. Direct logical-table reads require Tableau `full data` permission and remain subject to Tableau user permissions.
