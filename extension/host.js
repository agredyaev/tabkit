(() => {
  "use strict";

  const CHANNEL = "tabkit-web-dashboard";
  const BRIDGE_VERSION = 1;
  const RUNTIME_VERSION = 1;
  const MAX_ROWS = 100000;
  const MAX_INLINE_BYTES = 1024 * 1024;
  const MAX_BUNDLE_BYTES = 2 * 1024 * 1024;
  const MAX_BUNDLE_B64_BYTES = Math.ceil(MAX_BUNDLE_BYTES / 3) * 4 + 8;
  const MAX_RPC_REQUEST_BYTES = 64 * 1024;
  const MAX_RPC_RESPONSE_BYTES = 16 * 1024 * 1024;
  const MAX_DATA_BYTES = 8 * 1024 * 1024;
  const MAX_IN_FLIGHT = 4;
  const RPC_TIMEOUT_MS = 30000;
  const CHILD_RPC_TIMEOUT_MS = 35000;

  const frame = document.getElementById("dashboard");
  const status = document.getElementById("status");
  const encoder = new TextEncoder();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  const nonce = Array.from(crypto.getRandomValues(new Uint32Array(4)), (value) =>
    value.toString(16).padStart(8, "0")
  ).join("");
  let inFlight = 0;

  function fail(message) {
    status.className = "error";
    status.textContent = message;
    status.hidden = false;
    frame.style.display = "none";
  }

  function setting(name) {
    return tableau.extensions.settings.get(name) || "";
  }

  function byteSize(value) {
    return encoder.encode(value).byteLength;
  }

  function jsonByteSize(value) {
    return byteSize(JSON.stringify(value));
  }

  function isLoopbackHost() {
    const host = location.hostname.toLowerCase();
    return host === "localhost" || host === "127.0.0.1" || host === "::1";
  }

  async function verifyBundle(bundleB64, mac) {
    if (!mac && isLoopbackHost()) return;
    if (!mac) throw new Error("TabKit dashboard signature is missing");

    let response;
    try {
      response = await fetch("./verify", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ bundle_b64: bundleB64, mac }),
        credentials: "omit",
        cache: "no-store",
        redirect: "error",
        referrerPolicy: "no-referrer"
      });
    } catch {
      throw new Error("TabKit dashboard signature verification endpoint is unavailable");
    }
    if (response.status !== 204) {
      throw new Error("TabKit dashboard signature verification failed");
    }
  }

  function decodeBundle(value) {
    if (!value) throw new Error("TabKit extension setting tabkit.bundle_b64 is missing");
    if (value.length > MAX_BUNDLE_B64_BYTES) {
      throw new Error("TabKit dashboard bundle exceeds the encoded runtime limit");
    }

    let binary;
    try {
      binary = atob(value);
    } catch {
      throw new Error("TabKit dashboard bundle is not valid base64");
    }
    if (binary.length > MAX_BUNDLE_BYTES) {
      throw new Error("TabKit dashboard bundle exceeds the decoded runtime limit");
    }

    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    let parsed;
    try {
      parsed = JSON.parse(decoder.decode(bytes));
    } catch {
      throw new Error("TabKit dashboard bundle is not valid UTF-8 JSON");
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("TabKit dashboard bundle is invalid");
    }
    const keys = Object.keys(parsed);
    if (keys.some((key) => !["runtime_version", "html", "css", "javascript"].includes(key))) {
      throw new Error("TabKit dashboard bundle contains unsupported fields");
    }
    if (parsed.runtime_version !== RUNTIME_VERSION) {
      throw new Error(
        `Unsupported TabKit dashboard runtime version ${JSON.stringify(parsed.runtime_version)}; host supports ${RUNTIME_VERSION}`
      );
    }

    const bundle = {
      runtimeVersion: parsed.runtime_version,
      html: typeof parsed.html === "string" ? parsed.html : "",
      css: typeof parsed.css === "string" ? parsed.css : "",
      javascript: typeof parsed.javascript === "string" ? parsed.javascript : ""
    };
    if (!bundle.html && !bundle.javascript) {
      throw new Error("TabKit dashboard bundle does not contain HTML or JavaScript");
    }
    if (byteSize(bundle.html) + byteSize(bundle.css) + byteSize(bundle.javascript) > MAX_INLINE_BYTES) {
      throw new Error("TabKit inline dashboard exceeds the 1 MiB runtime limit");
    }
    return bundle;
  }

  function sanitizeHtml(html) {
    const template = document.createElement("template");
    template.innerHTML = html;
    const forbidden = new Set([
      "script", "iframe", "frame", "object", "embed", "link", "meta", "base", "form"
    ]);
    for (const element of [...template.content.querySelectorAll("*")]) {
      if (forbidden.has(element.localName.toLowerCase())) {
        element.remove();
        continue;
      }
      for (const attribute of [...element.attributes]) {
        const name = attribute.name.toLowerCase();
        const value = attribute.value.trim();
        if (
          name.startsWith("on") ||
          ["srcdoc", "action", "formaction", "ping", "target", "download"].includes(name)
        ) {
          element.removeAttribute(attribute.name);
          continue;
        }
        if (name === "href" || name === "xlink:href") {
          if (!value.startsWith("#")) element.removeAttribute(attribute.name);
          continue;
        }
        if (["src", "srcset", "poster", "data"].includes(name)) {
          if (!(name === "src" && /^data:image\//i.test(value))) {
            element.removeAttribute(attribute.name);
          }
        }
      }
    }
    return template.innerHTML;
  }

  function safeCss(css) {
    return css.replace(/<\/style/gi, "<\\/style");
  }

  function safeScript(javascript) {
    return javascript.replace(/<\/script/gi, "<\\/script");
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

  function boundedRows(reader, maxRows) {
    return (async () => {
      let columns = [];
      const rows = [];
      let dataBytes = 0;
      let byteTruncated = false;

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
          const values = row.map(dataCell);
          const rowBytes = jsonByteSize(values);
          if (dataBytes + rowBytes > MAX_DATA_BYTES) {
            byteTruncated = true;
            break;
          }
          rows.push(values);
          dataBytes += rowBytes;
        }
        if (byteTruncated) break;
      }

      const totalRowCount = reader.totalRowCount ?? rows.length;
      return {
        columns,
        rows,
        totalRowCount,
        truncated: byteTruncated || totalRowCount > rows.length
      };
    })();
  }

  async function listWorksheets() {
    return dashboard().worksheets.map((sheet) => ({ name: sheet.name }));
  }

  async function summaryData(payload) {
    const sheet = worksheetByName(payload.worksheet);
    const requested = Number.isInteger(payload.maxRows) ? payload.maxRows : MAX_ROWS;
    const maxRows = Math.max(1, Math.min(requested, MAX_ROWS));
    const reader = await sheet.getSummaryDataReaderAsync();
    try {
      return await boundedRows(reader, maxRows);
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

    const requested = Number.isInteger(payload.maxRows) ? payload.maxRows : MAX_ROWS;
    const maxRows = Math.max(1, Math.min(requested, MAX_ROWS));
    const reader = await source.getLogicalTableDataReaderAsync(table.id, Math.min(maxRows, 10000));
    try {
      return await boundedRows(reader, maxRows);
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
    if (frame.contentWindow) frame.contentWindow.postMessage({ ...message, nonce }, "*");
  }

  const TIMEOUT = Symbol("timeout");

  async function handleMessage(event) {
    if (event.source !== frame.contentWindow) return;
    const message = event.data;
    if (
      !message ||
      message.channel !== CHANNEL ||
      message.version !== BRIDGE_VERSION ||
      message.nonce !== nonce ||
      message.type !== "request"
    ) return;
    if (typeof message.id !== "string" || typeof message.method !== "string") return;

    let requestBytes;
    try {
      requestBytes = jsonByteSize(message.payload || {});
    } catch {
      post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "response", id: message.id, ok: false, error: "Bridge request is not serializable" });
      return;
    }
    if (requestBytes > MAX_RPC_REQUEST_BYTES) {
      post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "response", id: message.id, ok: false, error: "Bridge request is too large" });
      return;
    }
    const handler = handlers.get(message.method);
    if (!handler) {
      post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "response", id: message.id, ok: false, error: "Unsupported bridge method" });
      return;
    }
    if (inFlight >= MAX_IN_FLIGHT) {
      post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "response", id: message.id, ok: false, error: "Too many concurrent bridge requests" });
      return;
    }

    inFlight += 1;
    const operation = Promise.resolve().then(() => handler(message.payload || {}));
    let timedOut = false;
    let timer;
    try {
      const timeout = new Promise((_, reject) => {
        timer = setTimeout(() => reject(TIMEOUT), RPC_TIMEOUT_MS);
      });
      const result = await Promise.race([operation, timeout]);
      if (jsonByteSize(result) > MAX_RPC_RESPONSE_BYTES) {
        throw new Error("Bridge response is too large");
      }
      post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "response", id: message.id, ok: true, result });
    } catch (error) {
      if (error === TIMEOUT) {
        timedOut = true;
        operation.finally(() => {
          inFlight -= 1;
        }).catch(() => {});
      }
      post({
        channel: CHANNEL,
        version: BRIDGE_VERSION,
        type: "response",
        id: message.id,
        ok: false,
        error: error === TIMEOUT
          ? "Bridge request timed out"
          : error instanceof Error ? error.message : String(error)
      });
    } finally {
      if (timer) clearTimeout(timer);
      if (!timedOut) inFlight -= 1;
    }
  }

  function childSdk() {
    return `
(() => {
  "use strict";
  const CHANNEL = ${JSON.stringify(CHANNEL)};
  const VERSION = ${BRIDGE_VERSION};
  const NONCE = ${JSON.stringify(nonce)};
  const MAX_PENDING = ${MAX_IN_FLIGHT};
  const REQUEST_TIMEOUT_MS = ${CHILD_RPC_TIMEOUT_MS};
  let nextId = 1;
  const pending = new Map();
  const listeners = new Map();

  document.addEventListener("click", (event) => {
    const target = event.target;
    const anchor = target && target.closest ? target.closest("a[href]") : null;
    if (anchor) event.preventDefault();
  }, true);
  document.addEventListener("submit", (event) => event.preventDefault(), true);
  if (window.navigation && typeof window.navigation.addEventListener === "function") {
    window.navigation.addEventListener("navigate", (event) => {
      if (event.cancelable) event.preventDefault();
    });
  }
  try { window.open = () => null; } catch {}

  function call(method, payload = {}) {
    if (pending.size >= MAX_PENDING) {
      return Promise.reject(new Error("Too many concurrent TabKit requests"));
    }
    const id = String(nextId++);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error("TabKit bridge request timed out"));
      }, REQUEST_TIMEOUT_MS);
      pending.set(id, { resolve, reject, timer });
      parent.postMessage({
        channel: CHANNEL,
        version: VERSION,
        nonce: NONCE,
        type: "request",
        id,
        method,
        payload
      }, "*");
    });
  }

  addEventListener("message", (event) => {
    if (event.source !== parent) return;
    const message = event.data;
    if (
      !message ||
      message.channel !== CHANNEL ||
      message.version !== VERSION ||
      message.nonce !== NONCE
    ) return;
    if (message.type === "response") {
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      clearTimeout(waiter.timer);
      if (message.ok) waiter.resolve(message.result);
      else waiter.reject(new Error(message.error || "TabKit bridge request failed"));
      return;
    }
    if (message.type === "event") {
      for (const handler of listeners.get(message.event) || []) handler(message.payload);
    }
  });

  window.tabkit = Object.freeze({
    runtimeVersion: ${RUNTIME_VERSION},
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

  parent.postMessage({
    channel: CHANNEL,
    version: VERSION,
    nonce: NONCE,
    type: "ready"
  }, "*");
})();
`;
  }

  function srcdoc(html, css, javascript) {
    const csp = "default-src 'none'; img-src data: blob:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'; media-src data: blob:; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'";
    const sanitizedHtml = sanitizeHtml(html);
    return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<style>html,body{margin:0;width:100%;min-height:100%;box-sizing:border-box}*,*:before,*:after{box-sizing:inherit}
${safeCss(css)}</style>
</head>
<body>
${sanitizedHtml}
<script>${safeScript(childSdk())}</script>
<script>${safeScript(javascript)}</script>
</body>
</html>`;
  }

  function registerEvents() {
    const eventType = tableau.TableauEventType;
    for (const sheet of dashboard().worksheets) {
      sheet.addEventListener(eventType.FilterChanged, (event) => {
        post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "event", event: "filterChanged", payload: { worksheet: sheet.name, fieldName: event.fieldName } });
      });
      sheet.addEventListener(eventType.MarkSelectionChanged, () => {
        post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "event", event: "selectionChanged", payload: { worksheet: sheet.name } });
      });
      if (eventType.SummaryDataChanged) {
        sheet.addEventListener(eventType.SummaryDataChanged, () => {
          post({ channel: CHANNEL, version: BRIDGE_VERSION, type: "event", event: "summaryDataChanged", payload: { worksheet: sheet.name } });
        });
      }
    }
  }

  async function start() {
    try {
      await tableau.extensions.initializeAsync();
      const bundleB64 = setting("tabkit.bundle_b64");
      const bundleMac = setting("tabkit.bundle_mac");
      await verifyBundle(bundleB64, bundleMac);
      const bundle = decodeBundle(bundleB64);

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
