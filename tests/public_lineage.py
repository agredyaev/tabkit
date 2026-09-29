"""Opt-in Tableau Public lineage qualification; keeps downloads outside the repo."""

import hashlib
import io
import json
import pathlib
import select
import subprocess
import sys
import tempfile
import time
import urllib.request
import xml.etree.ElementTree as ET
import zipfile
from collections import Counter
from contextlib import contextmanager


# These hashes pin the extracted TWB, not the transport ZIP.
BOOKS = {
    "SuperMartSalesOverview": "30e5f4b0bf98214ac32b0aa9b88ed8d896770ef381cca943c577a15942078035",
    "ExecutiveSummary_17887725885850": "47735bcf082b2860b3ef45cd549dd4eb29c7dc5730c2abfb9b2e0b8877f66339",
    "exttest": "20acd3f00ac859ab7b8012d5eccd696b15f3e65e6126c2e2b3a8a273a36163cb",
    "VisualizeQuotaAttainmentforExecutivesinMultipleWays": "f566c027c7e202a1f8e3cf53d89d5f6fafc4708c87f4c039f5c46d4daa12ba69",
    "ParameterActions_16012998268320": "b56b652ec1bae8b41bcee0df510fe1a546ea0fea575897afddf1c17396019b66",
    "IntrotoSetActions-Part1Next-LevelTableau": "c4ef2d2b7809ec0f6d181d9b07512fc34e1e7f6c8522c3e62a2e634c84196128",
    "SizingStoryPoints": "1e53ada381ea048553ce6072c0a3a23bb00791fa970886bf9ab789686e4dcdc7",
    "VIZINTOOLTIP": "b96e64a529cab5a3cd605bc859af1b53082dcc53a0dc46cd0389e982ed9f9fd1",
    "MasteringTableau6_Tablecalculations1-Rollingmeasures": "f2a51750fdc472112360deec3f4ab67954fba66662587be27defb890c245d140",
}
GITHUB_BOOK = (
    "https://raw.githubusercontent.com/tableau/community-tableau-server-insights/"
    "master/datasources/ts_web_requests/ts_web_requests_03.01.twb",
    "fa11eb92394e9f49cd451ded7528c729963438be1f00f86cc7e1f41707ed905a",
)

ROUTES = {
    "ExecutiveSummary_17887725885850": (
        [("field", "[Order Date]"),
         ("field", "[Calculation_2589366273679360]", "Sample - Superstore"),
         ("datasource_filter", "[Calculation_2589366273679360]", "Sample - Superstore"),
         ("datasource", "Sample - Superstore"), ("worksheet", "Profit"),
         ("dashboard", "Overview")],
        ["calculation", "datasource_filter_field", "datasource_filtered",
         "datasource_used", "sheet_in_dashboard"],
    ),
    "exttest": (
        [("field", "[Order ID]"), ("field", "[Orders # (copy)_1519555718684745]"),
         ("custom_encoding", "[Sample - Superstore].[usr:Orders # (copy)_1519555718684745:qk]"),
         ("worksheet", "Sheet 9"), ("dashboard", "Overview dash")],
        ["calculation", "custom_encoding_field", "custom_encoding_on_sheet",
         "sheet_in_dashboard"],
    ),
    "VisualizeQuotaAttainmentforExecutivesinMultipleWays": (
        [("field", "[Sales_Amount]"), ("field", "[Calculation_1075813527691289]"),
         ("field", "[Calculation_1075813408002051]"),
         ("custom_encoding", "[federated.0dh0elc0safk6319llyns0jx4gy4].[usr:Calculation_1075813408002051:qk]"),
         ("worksheet", "Gauge "),
         ("dashboard", "Visualize Quota Attainment for Executives in Multiple Ways")],
        ["calculation", "calculation", "custom_encoding_field",
         "custom_encoding_on_sheet", "sheet_in_dashboard"],
    ),
}


def fetch(url, expected):
    with urllib.request.urlopen(url, timeout=30) as response:
        data = response.read(64 * 1024 * 1024 + 1)
    assert len(data) <= 64 * 1024 * 1024, f"Oversized workbook: {url}"
    if data.startswith(b"PK"):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            assert len(archive.infolist()) <= 4096, f"Too many ZIP entries: {url}"
            members = [name for name in archive.namelist() if name.endswith(".twb")]
            assert len(members) == 1, (url, members)
            assert archive.getinfo(members[0]).file_size <= 64 * 1024 * 1024
            data = archive.read(members[0])
    assert hashlib.sha256(data).hexdigest() == expected, f"Changed workbook: {url}"
    return data


def count_xml(root):
    actions = root.find("actions")
    return {
        "worksheet": len(root.findall("./worksheets/worksheet")),
        "dashboard": len(root.findall("./dashboards/dashboard")),
        "action": sum(child.tag == "action" or child.tag.endswith("-action") for child in (actions if actions is not None else [])),
        "set_or_group": len(list(root.iter("group"))),
        "table_calculation": len(list(root.iter("table-calc"))),
        "story_point": len(list(root.iter("story-point"))),
        "relationship": len(list(root.iter("relationship"))),
        "join": sum(relation.get("type") == "join" for relation in root.iter("relation")),
        "tooltip": len(list(root.iter("customized-tooltip"))),
        "parameter_control": sum(zone.get("type-v2", zone.get("type")) == "paramctrl" for zone in root.iter("zone")),
        "custom_encoding": len(root.findall("./worksheets/worksheet//encodings/custom")),
    }


@contextmanager
def mcp_process(binary, workspace):
    process = subprocess.Popen([str(binary), "--workspace", str(workspace), "mcp"],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL)
    try:
        yield process
    finally:
        if not process.stdin.closed:
            process.stdin.close()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def check_route(binary, workspace, name, xml, graph):
    if name not in ROUTES:
        return
    steps, kinds = ROUTES[name]

    def one(step):
        tag, value, *source = step
        owner = next((node["reference"] for node in graph["nodes"]
                      if node["reference"]["kind"] == "datasource"
                      and source and node["name"] == source[0]), None)
        matches = [node for node in graph["nodes"]
                   if node["reference"]["kind"] == tag and node["name"] == value
                   and (not source or node.get("datasource") == owner)]
        assert len(matches) == 1, (name, tag, value, len(matches))
        return matches[0]["reference"]

    def formula(field, source=None):
        matches = [column.find("calculation") for ds in xml.findall("./datasources/datasource")
                   if source is None or ds.get("name") == source
                   for column in ds.findall("./column") if column.get("name") == field]
        assert len(matches) == 1 and matches[0] is not None, (name, field)
        return matches[0].get("formula")

    refs = [one(step) for step in steps]
    route = [{"from": left, "to": right, "kind": kind}
             for left, right, kind in zip(refs, refs[1:], kinds)]
    action_edges = []
    assert all(edge in graph["edges"] for edge in route), (name, route)
    assert any(record.findtext("local-name") == steps[0][1]
               for record in xml.iter("metadata-record")), (name, "source field")

    if name == "ExecutiveSummary_17887725885850":
        assert "[Order Date]" in formula(steps[1][1], steps[3][1])
        source = next(ds for ds in xml.findall("./datasources/datasource")
                      if ds.get("name") == steps[3][1])
        assert any(f.get("column") == steps[1][1] for f in source.findall("./filter"))
        sheet = next(s for s in xml.findall("./worksheets/worksheet")
                     if s.get("name") == steps[4][1])
        assert any(ds.get("name") == source.get("name")
                   for ds in sheet.findall("./table/view/datasources/datasource"))
    else:
        assert steps[0][1] in formula(steps[1][1])
        if name == "VisualizeQuotaAttainmentforExecutivesinMultipleWays":
            assert steps[1][1] in formula(steps[2][1])
        sheet = next(s for s in xml.findall("./worksheets/worksheet")
                     if s.get("name") == steps[-2][1])
        assert any(custom.get("column") == steps[-3][1]
                   for custom in sheet.findall(".//encodings/custom"))
    dashboard = next(d for d in xml.findall("./dashboards/dashboard")
                     if d.get("name") == steps[-1][1])
    assert any(zone.get("name") == steps[-2][1] for zone in dashboard.iter("zone"))
    if name == "exttest":
        action = next(a for a in xml.findall("./actions/edit-parameter-action")
                      if a.get("caption") == "ParameterColorStart1")
        parameters = {p.get("name"): p.get("value") for p in action.findall("./params/param")}
        assert action.find("source").get("worksheet") == "retention"
        assert parameters == {
            "source-field": "[Sample - Superstore].[usr:Calculation_1519553562972165:qk]",
            "target-parameter": "[Parameters].[ColorStart]",
        }
        action_ref = one(("action", action.get("name")))
        action_edges = [
            {"from": one(("field", "[Calculation_1519553562972165]")),
             "to": action_ref, "kind": "action_input"},
            {"from": one(("worksheet", "retention")),
             "to": action_ref, "kind": "action_source"},
            {"from": action_ref, "to": one(("field", "[ColorStart]")),
             "kind": "action_target"},
        ]
        assert all(edge in graph["edges"] for edge in action_edges)

    with mcp_process(binary, workspace) as process:
        sequence = 0

        def rpc(method, params):
            nonlocal sequence
            sequence += 1
            process.stdin.write((json.dumps({"jsonrpc": "2.0", "id": sequence,
                                             "method": method, "params": params}) + "\n").encode())
            process.stdin.flush()
            assert select.select([process.stdout], [], [], 10)[0], (name, method, "timeout")
            reply = json.loads(process.stdout.readline())
            assert reply.get("id") == sequence and "error" not in reply, (name, reply)
            return reply["result"]

        rpc("initialize", {"protocolVersion": "2025-03-26", "capabilities": {},
                           "clientInfo": {"name": "public-lineage", "version": "1"}})
        process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        process.stdin.flush()

        def tool(tool_name, args):
            result = rpc("tools/call", {"name": tool_name, "arguments": args})
            assert not result.get("isError"), (name, tool_name, result)
            return json.loads(next(part["text"] for part in result["content"]
                                   if part["type"] == "text"))

        opened = tool("workbook_lineage_open", {"input": "book.twb"})
        assert opened["input_sha256"] == graph["input_sha256"]
        snapshot = opened["snapshot_id"]
        for edge in route + action_edges:
            offset = 0
            while True:
                page = tool("workbook_lineage_neighbors", {"snapshot_id": snapshot,
                            "node": edge["from"], "direction": "downstream",
                            "offset": offset, "limit": 100})
                if any(item["edge"] == edge for item in page["items"]):
                    break
                offset = page["next_offset"]
                assert offset is not None, (name, edge)
        if action_edges:
            detail = tool("workbook_lineage_details", {"snapshot_id": snapshot,
                           "node": action_ref})
            assert detail["details"]["source"]["worksheet"] == "retention"
            assert {p["name"]: p["value"] for p in detail["details"]["parameters"]} == parameters
        args = {"snapshot_id": snapshot, "from": refs[0],
                "direction": "downstream", "limit": 100}
        seen = set()
        seen_edges = []
        for _ in range(len(graph["edges"]) + 1):
            page = tool("workbook_lineage_impact", args)
            seen.update((node["node"]["reference"]["kind"],
                         node["node"]["reference"]["id"]) for node in page["nodes"])
            seen_edges.extend(page["edges"])
            if page["done"]:
                break
            args = {"snapshot_id": snapshot, "cursor": page["cursor"], "limit": 100}
        else:
            raise AssertionError((name, "impact did not finish"))
        assert (refs[-1]["kind"], refs[-1]["id"]) in seen, (name, "dashboard impact")
        assert all(edge in seen_edges for edge in route), (name, "incomplete impact path")


def check(binary, workspace, name, data):
    (workspace / "book.twb").write_bytes(data)
    output = f"{name}.json"
    started = time.perf_counter()
    run = subprocess.run(
        [str(binary), "--workspace", str(workspace), "call", "workbook_lineage_export"],
        input=json.dumps({"input": "book.twb", "snapshot_id": None, "output": output}),
        text=True, capture_output=True, check=True,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    report = json.loads(run.stdout)
    graph = json.loads((workspace / output).read_text())
    assert report["input_sha256"] == hashlib.sha256(data).hexdigest()
    assert graph["schema_version"] == 2
    assert len(graph["nodes"]) == len(graph["details"])
    nodes = {(node["reference"]["kind"], node["reference"]["id"]) for node in graph["nodes"]}
    assert len(nodes) == len(graph["nodes"])
    for edge in graph["edges"]:
        assert (edge["from"]["kind"], edge["from"]["id"]) in nodes
        assert (edge["to"]["kind"], edge["to"]["id"]) in nodes
    source = ET.fromstring(data)
    expected = count_xml(source)
    actual = Counter(node["reference"]["kind"] for node in graph["nodes"])
    for kind in ("worksheet", "dashboard", "action", "table_calculation", "story_point", "relationship", "join", "tooltip", "parameter_control", "custom_encoding"):
        assert actual[kind] == expected[kind], (name, kind, actual[kind], expected[kind])
    assert Counter(node.get("custom-type-name", "custom") for node in source.findall("./worksheets/worksheet//encodings/custom")) == Counter(
        graph["details"][i]["custom_type"]
        for i, node in enumerate(graph["nodes"])
        if node["reference"]["kind"] == "custom_encoding"
    )
    assert actual["set"] + actual["group"] == expected["set_or_group"]
    assert Counter(len(group.findall("./groupfilter")) for group in source.iter("group")) == Counter(
        len(graph["details"][i]["definition"])
        for i, node in enumerate(graph["nodes"])
        if node["reference"]["kind"] in ("set", "group")
    )
    assert Counter(len(relation.findall("./clause")) for relation in source.iter("relation") if relation.get("type") == "join") == Counter(
        len(graph["details"][i]["conditions"])
        for i, node in enumerate(graph["nodes"])
        if node["reference"]["kind"] == "join"
    )
    if name == "VIZINTOOLTIP":
        assert any(edge["kind"] == "tooltip_sheet" for edge in graph["edges"])
    if name == "tableau-ts-web-requests":
        assert actual["join"] and actual["custom_sql"]
        assert any(edge["kind"] == "sql_read" for edge in graph["edges"])
    if name == "IntrotoSetActions-Part1Next-LevelTableau":
        assert not graph["gaps"]
        assert any(edge["from"]["kind"] == "set" and edge["kind"] == "table_calculation_order" for edge in graph["edges"])
    if name in ("ExecutiveSummary_17887725885850", "exttest", "VisualizeQuotaAttainmentforExecutivesinMultipleWays"):
        assert not any(gap["code"] == "FIELD_ORIGIN" for gap in graph["gaps"])
        assert not any(gap["code"] == "CUSTOM_ENCODING_FIELD" for gap in graph["gaps"])
        assert any(edge["kind"] == "extract_field_origin" for edge in graph["edges"])
    if name == "ExecutiveSummary_17887725885850":
        assert {gap["code"] for gap in graph["gaps"]} == {"UNSUPPORTED_CALCULATION_CLASS"}
    if name == "VisualizeQuotaAttainmentforExecutivesinMultipleWays":
        assert not graph["gaps"]
    if name == "exttest":
        assert all(gap["code"] == "UNSUPPORTED_CALCULATION_CLASS" or any(
            special in gap.get("context", "") for special in (":Measure Names", "Multiple Values")
        ) for gap in graph["gaps"])
    check_route(binary, workspace, name, source, graph)
    print(f"{name}: {len(graph['nodes'])} nodes, {len(graph['edges'])} edges, {elapsed_ms:.1f} ms")


def main():
    binary = pathlib.Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="tabkit-public-") as directory:
        workspace = pathlib.Path(directory)
        for name, digest in BOOKS.items():
            check(binary, workspace, name,
                  fetch(f"https://public.tableau.com/workbooks/{name}.twb", digest))
        check(binary, workspace, "tableau-ts-web-requests", fetch(*GITHUB_BOOK))


if __name__ == "__main__":
    main()
