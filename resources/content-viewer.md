# Content viewer — read-only
Resolve the named object through tableau_search/tableau_explore, retaining exact IDs and project/workbook context. Do not choose an ambiguous match. For a requested dashboard locate its view first and retain the containing workbook ID.

Use tableau_view_image for a static PNG of the chosen view, saved at a new workspace path. This does not reproduce the interactive Codex side panel. Do not query rows or edit/publish just to show content. tableau_view_data is a separate, explicitly authorized data operation; a dashboard CSV is not all of its worksheets. Verify relevant sheets individually.

Report the exact view/workbook and whether export succeeded. A saved file can remain if preview rendering fails; inspect the returned output/hash instead of automatically repeating a write. A screenshot cannot establish filter-action, tooltip, keyboard or screen-reader behavior.
