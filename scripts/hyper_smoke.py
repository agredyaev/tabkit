#!/usr/bin/env python3
"""Native Hyper CLI/MCP smoke against a real extract and the official SDK."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import zipfile

if len(sys.argv) not in (4, 5):
    raise SystemExit("usage: hyper_smoke.py <binary> <sdk-dir> <book.twbx> <hyper-entry> | <binary> <sdk-dir> <sample.hyper>")

binary, sdk, package = (Path(arg).resolve() for arg in sys.argv[1:4])
entry = sys.argv[4] if len(sys.argv) == 5 else "Data/sample.hyper"
runtime = binary.parent / "hyper"
bundled = (runtime / "hyperd.exe").is_file() or (runtime / "hyperd").is_file()
if not bundled:
    runtime = sdk / "lib/hyper"
    assert (runtime / "hyperd").is_file()
env = os.environ.copy()
if bundled:
    env.pop("HYPER_SDK_DIR", None)
    if os.name == "nt":
        sdk_bin = (sdk / "bin").resolve()
        env["PATH"] = os.pathsep.join(path for path in env["PATH"].split(os.pathsep)
                                          if Path(path).resolve() != sdk_bin)
if not bundled and (sdk / "lib/libtableauhyperapi.dylib").is_file():
    env["DYLD_LIBRARY_PATH"] = str(sdk / "lib")


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def quoted(name):
    return '"' + name.replace('"', '""') + '"'


with tempfile.TemporaryDirectory(prefix="tabkit-hyper-smoke-") as directory:
    workspace = Path(directory)
    source = workspace / "book.twbx"
    if package.suffix == ".hyper":
        with zipfile.ZipFile(source, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.write(Path(__file__).resolve().parents[1] / "examples/synthetic.twb", "book.twb")
            archive.write(package, entry)
    else:
        shutil.copyfile(package, source)
    source_sha = sha(source)
    common = [str(binary), "--workspace", str(workspace)]
    if not bundled:
        common += ["--hyper-runtime-directory", str(runtime)]

    def call(tool, args, *, data=True):
        command = common + (["--allow-data-output"] if data else []) + ["call", tool]
        run = subprocess.run(command, input=json.dumps(args), text=True,
                             capture_output=True, timeout=60, env=env)
        result = json.loads(run.stdout if run.returncode == 0 else run.stderr)
        return run.returncode, result

    def good(tool, args):
        code, result = call(tool, args)
        assert code == 0, (tool, result)
        return result

    status = good("system_status", {})
    assert status["hyper_compiled"] and status["hyper_configured"]
    extracted = good("workbook_extract_hyper", {
        "input": source.name, "expected_sha256": source_sha,
        "entry": entry, "output": "extract.hyper"})
    extract = workspace / extracted["output"]
    assert sha(extract) == extracted["sha256"]

    def query(operation, max_rows=10):
        return good("hyper_query", {
            "input": extract.name, "expected_sha256": extracted["sha256"],
            "operation": operation, "max_rows": max_rows})

    tables = query({"operation": "tables"}, 100)["rows"]
    assert tables and all(row[2] == "BASE TABLE" for row in tables)
    if len(tables) > 1:
        assert query({"operation": "tables"}, 1)["truncated"]
    selected = None
    for schema, table, _ in tables:
        columns = query({"operation": "columns", "schema": schema, "table": table}, 100)["rows"]
        text = next((row[0] for row in columns if row[1] == "TEXT"), None)
        numeric = next((row[0] for row in columns if row[1] in ("BIGINT", "INT", "INTEGER")), None)
        if text and numeric:
            selected = schema, table, text, numeric
            break
    assert selected, "Fixture needs a table with text and numeric columns"
    schema, table, text_column, numeric_column = selected
    assert query({"operation": "columns", "schema": schema, "table": table}, 1)["truncated"]
    relation = f"{quoted(schema)}.{quoted(table)}"
    count = int(query({"operation": "query", "sql": f"SELECT COUNT(*) FROM {relation}"})["rows"][0][0])
    assert count > 2, "Fixture needs at least three rows"
    sample = query({"operation": "sample", "schema": schema, "table": table}, 2)
    assert len(sample["rows"]) == 2 and sample["truncated"]
    distinct = query({"operation": "distinct", "schema": schema, "table": table,
                      "column": text_column}, 3)
    assert distinct["rows"] and len(distinct["rows"]) <= 3
    bounds = query({"operation": "min_max", "schema": schema, "table": table,
                    "column": numeric_column})
    assert len(bounds["rows"]) == 1 and int(bounds["rows"][0][2]) == count

    request = {"input": extract.name, "expected_sha256": extracted["sha256"],
               "operation": {"operation": "tables"}}
    code, denied = call("hyper_query", request, data=False)
    assert code == 2 and denied["error"]["code"] == "DATA_POLICY"
    code, stale = call("hyper_query", {**request, "expected_sha256": "0" * 64})
    assert code == 2 and stale["error"]["code"] == "STALE_BASE"
    code, write = call("hyper_query", {**request,
        "operation": {"operation": "query", "sql": f"DELETE FROM {relation}"}})
    assert code == 2 and write["error"]["code"] == "SQL_POLICY"
    code, missing = call("hyper_query", {**request,
        "operation": {"operation": "sample", "schema": schema, "table": "tabkit_missing_table"}})
    assert code == 2 and missing["error"]["code"] == "HYPER_QUERY"
    if count > 100:
        limited = subprocess.run(common + ["--allow-data-output", "--result-bytes", "8192", "call", "hyper_query"],
            input=json.dumps({**request, "operation": {"operation": "sample", "schema": schema, "table": table},
                              "max_rows": 1000}), text=True, capture_output=True, timeout=60, env=env)
        assert limited.returncode == 2 and json.loads(limited.stderr)["error"]["message"] == "HYPER_RESULT_LIMIT"

    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "hyper-smoke", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "hyper_query", "arguments": {**request,
                "operation": {"operation": "query", "sql": f"SELECT COUNT(*) FROM {relation}"}}}},
    ]
    run = subprocess.run(common + ["--allow-data-output", "mcp"],
                         input="\n".join(map(json.dumps, messages)) + "\n", text=True,
                         capture_output=True, timeout=60, env=env)
    assert run.returncode == 0, run.stderr
    response = next(json.loads(line) for line in run.stdout.splitlines()
                    if json.loads(line).get("id") == 2)
    assert not response["result"].get("isError")
    mcp_result = json.loads(response["result"]["content"][0]["text"])
    assert int(mcp_result["rows"][0][0]) == count
    assert sha(source) == source_sha and sha(extract) == extracted["sha256"]
    print(json.dumps({"passed": True, "tables": len(tables), "tested_table": table,
                      "rows": count, "source_preserved": True, "mcp_query": True}))
