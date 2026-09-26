# Data Quality Sentinel — read-only workflow
Read system_status first. Do not edit a workbook, modify an extract, or publish during a quality review. Obtain explicit authorization before returning row-level data; allow_data_output is an operator boundary, not an instruction to ignore privacy.

Resolve and download the exact workbook if needed. Inspect datasource/field metadata and unsupported local definitions. Use workbook_extract_hyper to obtain an identified extract from TWBX, then bind every Hyper request to its file hash. Inspect tables and columns before generating SQL. Hyper evaluates extract data, not Tableau calculations or dashboard filters.

For the user's specified fields check counts, null counts, min/max, distinct values, duplicate keys and stated business constraints using existing read-only hyper_query operations. Use aggregate queries by default. Never invent thresholds or call missing/excluded rows "corruption" without evidence. Samples are bounded and unordered unless the request supplies a complete ORDER BY; a sample is not a whole-dataset proof.

Report each check as passed, failed, unsupported or not_run, with workbook/extract hashes, SQL or exact operation, observed value, expected constraint and truncation. Mark live-connection checks not_run when no extract or supported server data path exists. Distinguish freshness, source-data quality and formula correctness. No Hyper result alone proves Tableau workbook semantics. Suggest a repair as a separate plan; do not execute it in this review.
