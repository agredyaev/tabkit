"""Opt-in Tableau Public lineage qualification; keeps downloads outside the repo."""

import hashlib
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.request
import xml.etree.ElementTree as ET
import zipfile
from collections import Counter


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
    }


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
    for kind in ("worksheet", "dashboard", "action", "table_calculation", "story_point", "relationship", "join", "tooltip", "parameter_control"):
        assert actual[kind] == expected[kind], (name, kind, actual[kind], expected[kind])
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
        assert any(edge["kind"] == "extract_field_origin" for edge in graph["edges"])
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
