(() => {
  "use strict";

  const CHANNEL = "tapkit-web-dashboard";
  const VERSION = 1;
  const MAX_SUMMARY_ROWS = 100000;
  const MAX_INLINE_BYTES = 1024 * 1024;

  const frame = document.getElementById("dashboard");
  const status = document.getElementById("status");

  const encoder = new TextEncoder();

  function fail(message) {
    status.className = "error";
    status.textContent = message;
    status.hidden = false;
    frame.style.display = "none";
  }

  function setting(name) {
    return tableau.extensions.settings.get(name) || "";
  }

  function inlineSize(parts) {
    return parts.reduce((sum, part) => sum + encoder.encode(part).byteLength, 0);
  }

  function decodeBundle(value) {
    if (!value) throw new Error("TapKit extension setting tapkit.bundle_b64 is missing");
    let binary;
    try {
      binary = atob(value);
    } catch {
      throw new Error("TapKit dashboard bundle is not valid base64");
    }
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    let parsed;
    try {
      parsed = JSON.parse(new TextDecoder().decode(bytes));
    } catch {
      throw new Error("TapKit dashboard bundle is not valid UTF-8 JSON");
    }
    if (!parsed || typeof parsed !== "object") throw new Error("TapKit dashboard bundle is invalid");
    return {
      html: typeof parsed.html === "string" ? parsed.html : "",
      css: typeof parsed.css === "string" ? parsed.css : "",
      javascript: typeof parsed.javascript === "string" ? parsed.javascript : ""
    };
  }

  function safeValue(value) {
    if (value instanceof Date) return value.toISOString();
    if (value === undefined) return null;
    if (typeof value === "number" && !Number.isFinite(value)) return null;
    return value;
  }

  function dataCell(cell) {
    if (!cell) return null;
    if ("nativeValue" in cell) return safeValue(cell.nativeValue);
    if ("value" in cell) return safeValue(cell.value);
    return cell.formattedValue ?? null;
  }

  function tableToJson(table, maxRows) {
    const columns = (table.columns || []).map((column) => ({
      fieldName: column.fieldName,
      dataType: column.dataType,
      isReferenced: column.isReferenced
    }));
    const rows = (table.data || []).slice(0, maxRows).map((row) => row.map(dataCell));
    return {
      columns,
      rows,
      totalRowCount: table.totalRowCount ?? rows.length,
      truncated: (table.totalRowCount ?? rows.length) > rows.length
    };
  }

  function dashboard() {
    const content = tableau.extensions.dashboardContent;
    if (!content || !content.dashboard) throw new Error("Dashboard extension context is unavailable");
    return content.dashboard;
  }

  function worksheetByName(name) {
    const sheet = dashboard().worksheets.find((candidate) => candidate.name === name);
    if (!sheet) throw new Error(`Worksheet ${JSON.stringify(name)} does not exist on this dashboard`);
    return sheet;
  }

  async function listWorksheets() {
    return dashboard().worksheets.map((sheet) => ({ name: sheet.name }));
  }

  async function summaryData(payload) {
    const sheet = worksheetByName(payload.worksheet);
    const requested = Number.isInteger(payload.maxRows) ? payload.maxRows : MAX_SUMMARY_ROWS;
    const maxRows = Math.max(1, Math.min(requested, MAX_SUMMARY_ROWS));
    const reader = await sheet.getSummaryDataReaderAsync();
    try {
      const table = await reader.getAllPagesAsync();
      return tableToJson(table, maxRows);
    } finally {
      await reader.releaseAsync();
    }
  }

  async function dataSourceByName(name) {
    const sources = await tableau.extensions.workbook.getAllDataSourcesAsync();
    const source = sources.find((candidate) => candidate.name === name);
    if (!source) throw new Error(`Data source ${JSON.stringify(name)} does not exist in this workbook`);
    return source;
  }

  async function listDataSources() {
    const sources = await tableau.extensions.workbook.getAllDataSourcesAsync();
    return sources.map((source) => ({ name: source.name }));
  }

  async function listLogicalTables(payload) {
    const source = await dataSourceByName(payload.datasource);
    const tables = await source.getLogicalTablesAsync();
    return tables.map((table) => ({ id: table.id, caption: table.caption }));
  }

  async function logicalTableData(payload) {
    const source = await dataSourceByName(payload.datasource);
    const tables = await source.getLogicalTablesAsync();
    const table = tables.find(
      (candidate) => candidate.id === payload.table || candidate.caption === payload.table
    );
    if (!table) {
      throw new Error(
        `Logical table ${JSON.stringify(payload.table)} does not exist in data source ${JSON.stringify(payload.datasource)}`
      );
    }

    const requested = Number.isInteger(payload.maxRows) ? payload.maxRows : MAX_SUMMARY_ROWS;
    const maxRows = Math.max(1, Math.min(requested, MAX_SUMMARY_ROWS));
    const reader = await source.getLogicalTableDataReaderAsync(table.id, Math.min(maxRows, 10000));
    try {
      let columns = [];
      const rows = [];
      for (let pageNumber = 0; pageNumber < reader.pageCount && rows.length < maxRows; pageNumber += 1) {
        const page = await reader.getPageAsync(pageNumber);
        if (columns.length === 0) {
          columns = (page.columns || []).map((column) => ({
            fieldName: column.fieldName,
            dataType: column.dataType,
            isReferenced: column.isReferenced
          }));
        }
        for (const row of page.data || []) {
          if (rows.length >= maxRows) break;
          rows.push(row.map(dataCell));
        }
      }
      const totalRowCount = reader.totalRowCount ?? rows.length;
      return { columns, rows, totalRowCount, truncated: totalRowCount > rows.length };
    } finally {
      await reader.releaseAsync();
    }
  }

  async function applyFilter(payload) {
    const values = Array.isArray(payload.values) ? payload.values : [payload.values];
    await worksheetByName(payload.worksheet).applyFilterAsync(
      payload.field,
      values,
      tableau.FilterUpdateType.Replace
    );
    return { ok: true };
  }

  async function clearFilter(payload) {
    await worksheetByName(payload.worksheet).clearFilterAsync(payload.field);
    return { ok: true };
  }

  async function parameters() {
    const items = await dashboard().getParametersAsync();
    return items.map((parameter) => ({
      name: parameter.name,
      dataType: parameter.dataType,
      currentValue: dataCell(parameter.currentValue),
      allowableValues: parameter.allowableValues
    }));
  }

  async function setParameter(payload) {
    const items = await dashboard().getParametersAsync();
    const parameter = items.find((candidate) => candidate.name === payload.name);
    if (!parameter) throw new Error(`Parameter ${JSON.stringify(payload.name)} does not exist`);
    const result = await parameter.changeValueAsync(payload.value);
    return { name: parameter.name, value: dataCell(result) };
  }

  const handlers = new Map([
    ["worksheets.list", listWorksheets],
    ["data.summary", summaryData],
    ["datasources.list", listDataSources],
    ["datasource.tables", listLogicalTables],
    ["datasource.data", logicalTableData],
    ["filter.apply", applyFilter],
    ["filter.clear", clearFilter],
    ["parameters.list", parameters],
    ["parameter.set", setParameter]
  ]);

  function post(message) {
    if (frame.contentWindow) frame.contentWindow.postMessage(message, "*");
  }

  async function handleMessage(event) {
    if (event.source !== frame.contentWindow) return;
    const message = event.data;
    if (!message || message.channel !== CHANNEL || message.version !== VERSION || message.type !== "request") return;
    if (typeof message.id !== "string" || typeof message.method !== "string") return;

    const handler = handlers.get(message.method);
    if (!handler) {
      post({ channel: CHANNEL, version: VERSION, type: "response", id: message.id, ok: false, error: "Unsupported bridge method" });
      return;
    }

    try {
      const result = await handler(message.payload || {});
      post({ channel: CHANNEL, version: VERSION, type: "response", id: message.id, ok: true, result });
    } catch (error) {
      post({
        channel: CHANNEL,
        version: VERSION,
        type: "response",
        id: message.id,
        ok: false,
        error: error instanceof Error ? error.message : String(error)
      });
    }
  }

  function childSdk() {
    return `
(() => {
  "use strict";
  const CHANNEL = "${CHANNEL}";
  const VERSION = ${VERSION};
  let nextId = 1;
  const pending = new Map();
  const listeners = new Map();

  function call(method, payload = {}) {
    const id = String(nextId++);
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      parent.postMessage({ channel: CHANNEL, version: VERSION, type: "request", id, method, payload }, "*");
    });
  }

  addEventListener("message", (event) => {
    if (event.source !== parent) return;
    const message = event.data;
    if (!message || message.channel !== CHANNEL || message.version !== VERSION) return;
    if (message.type === "response") {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      if (message.ok) waiter.resolve(message.result);
      else waiter.reject(new Error(message.error || "TapKit bridge request failed"));
      return;
    }
    if (message.type === "event") {
      for (const handler of listeners.get(message.event) || []) handler(message.payload);
    }
  });

  window.tapkit = Object.freeze({
    listWorksheets: () => call("worksheets.list"),
    getSummaryData: (worksheet, maxRows) => call("data.summary", { worksheet, maxRows }),
    listDataSources: () => call("datasources.list"),
    listLogicalTables: (datasource) => call("datasource.tables", { datasource }),
    getLogicalTableData: (datasource, table, maxRows) =>
      call("datasource.data", { datasource, table, maxRows }),
    applyFilter: (worksheet, field, values) => call("filter.apply", { worksheet, field, values }),
    clearFilter: (worksheet, field) => call("filter.clear", { worksheet, field }),
    listParameters: () => call("parameters.list"),
    setParameter: (name, value) => call("parameter.set", { name, value }),
    on(event, handler) {
      if (typeof handler !== "function") throw new TypeError("handler must be a function");
      const set = listeners.get(event) || new Set();
      set.add(handler);
      listeners.set(event, set);
      return () => set.delete(handler);
    }
  });

  parent.postMessage({ channel: CHANNEL, version: VERSION, type: "ready" }, "*");
})();
`;
  }

  function srcdoc(html, css, javascript) {
    const csp = "default-src 'none'; img-src data: blob:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'; media-src data: blob:; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'";
    return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<style>html,body{margin:0;width:100%;min-height:100%;box-sizing:border-box}*,*:before,*:after{box-sizing:inherit}
${css}</style>
</head>
<body>
${html}
<script>${childSdk().replace(/<\/script/gi, "<\\/script")}<\/script>
<script>${javascript.replace(/<\/script/gi, "<\\/script")}<\/script>
</body>
</html>`;
  }

  function registerEvents() {
    const eventType = tableau.TableauEventType;
    for (const sheet of dashboard().worksheets) {
      sheet.addEventListener(eventType.FilterChanged, (event) => {
        post({ channel: CHANNEL, version: VERSION, type: "event", event: "filterChanged", payload: { worksheet: sheet.name, fieldName: event.fieldName } });
      });
      sheet.addEventListener(eventType.MarkSelectionChanged, () => {
        post({ channel: CHANNEL, version: VERSION, type: "event", event: "selectionChanged", payload: { worksheet: sheet.name } });
      });
      if (eventType.SummaryDataChanged) {
        sheet.addEventListener(eventType.SummaryDataChanged, () => {
          post({ channel: CHANNEL, version: VERSION, type: "event", event: "summaryDataChanged", payload: { worksheet: sheet.name } });
        });
      }
    }
  }

  async function start() {
    try {
      await tableau.extensions.initializeAsync();
      const bundle = decodeBundle(setting("tapkit.bundle_b64"));
      if (!bundle.html && !bundle.javascript) {
        throw new Error("TapKit dashboard bundle does not contain HTML or JavaScript");
      }
      if (inlineSize([bundle.html, bundle.css, bundle.javascript]) > MAX_INLINE_BYTES) {
        throw new Error("TapKit inline dashboard exceeds the 1 MiB runtime limit");
      }

      addEventListener("message", handleMessage);
      registerEvents();
      frame.srcdoc = srcdoc(bundle.html, bundle.css, bundle.javascript);
      frame.style.display = "block";
      status.hidden = true;

      if (typeof tableau.extensions.reportExtensionRenderComplete === "function") {
        frame.addEventListener("load", () => {
          tableau.extensions.reportExtensionRenderComplete().catch(() => {});
        }, { once: true });
      }
    } catch (error) {
      fail(error instanceof Error ? error.message : String(error));
    }
  }

  start();
})();
