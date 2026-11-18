#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};
use tempfile::tempdir;

fn send(stdin: &mut impl Write, value: Value) {
    serde_json::to_writer(&mut *stdin, &value).expect("serialize MCP request");
    stdin.write_all(b"\n").expect("write MCP newline");
    stdin.flush().expect("flush MCP request");
}

fn receive(stdout: &mut impl BufRead) -> Value {
    let mut line = String::new();
    let count = stdout.read_line(&mut line).expect("read MCP response");
    assert!(count > 0, "MCP server closed stdout unexpectedly");
    serde_json::from_str(&line).expect("parse MCP response")
}

fn tool_text(response: &Value) -> Value {
    let text = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("tool text result");
    serde_json::from_str(text).expect("parse tool JSON")
}

#[test]
fn stdio_mcp_discovers_and_uses_local_workbook() {
    let dir = tempdir().expect("temp dir");
    let workbook = dir.path().join("fixture.twb");
    std::fs::write(
        &workbook,
        br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Sales">
      <table><view><datasource-dependencies datasource="Orders">
        <column datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      </datasource-dependencies></view></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000001}"/>
    </worksheet>
  </worksheets>
</workbook>"#,
    )
    .expect("write fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_tabkit-mcp"))
        .arg("--transport")
        .arg("stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start tabkit-mcp");

    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "tabkit-test", "version": "1"}
            }
        }),
    );
    let initialize = receive(&mut stdout);
    assert_eq!(initialize["id"], 1);
    assert!(initialize.get("result").is_some());

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    let tools = receive(&mut stdout);
    let names: Vec<&str> = tools
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    for expected in [
        "tableau_search_workbooks",
        "workbook_create",
        "workbook_open",
        "workbook_inspect",
        "workbook_lineage",
        "workbook_edit",
        "workbook_validate",
        "workbook_publish",
        "tableau_job",
        "workbook_close",
    ] {
        assert!(names.contains(&expected), "missing MCP tool {expected}");
    }

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":30,
            "method":"tools/call",
            "params":{
                "name":"workbook_create",
                "arguments":{}
            }
        }),
    );
    let created = tool_text(&receive(&mut stdout));
    let blank_handle = created["handle"]
        .as_str()
        .expect("blank workbook handle")
        .to_owned();
    assert_eq!(created["created"], true);
    assert_eq!(created["format"], "twb");
    assert_eq!(created.pointer("/summary/datasources"), Some(&json!(0)));
    assert_eq!(created.pointer("/summary/worksheets"), Some(&json!(0)));
    assert_eq!(created.pointer("/summary/dashboards"), Some(&json!(0)));

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":31,
            "method":"tools/call",
            "params":{
                "name":"workbook_edit",
                "arguments":{
                    "handle":blank_handle,
                    "batch":{
                        "edits":[{
                            "op":"create_web_dashboard",
                            "name":"Blank Web",
                            "width":1280,
                            "height":720,
                            "extension_url":"http://127.0.0.1:8765/host.html",
                            "html":"<main>Blank Web</main>",
                            "css":"",
                            "javascript":""
                        }]
                    }
                }
            }
        }),
    );
    let blank_edited = tool_text(&receive(&mut stdout));
    let blank_candidate = blank_edited["handle"]
        .as_str()
        .expect("blank candidate handle")
        .to_owned();
    assert_eq!(blank_edited.pointer("/summary/dashboards"), Some(&json!(1)));

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":32,
            "method":"tools/call",
            "params":{
                "name":"workbook_inspect",
                "arguments":{
                    "handle":blank_candidate,
                    "kind":"dashboard",
                    "query":"Blank Web"
                }
            }
        }),
    );
    let blank_dashboard = tool_text(&receive(&mut stdout));
    assert_eq!(blank_dashboard["count"], 1);
    assert_eq!(blank_dashboard["items"][0]["name"], "Blank Web");

    for (id, close_handle) in [(33, blank_candidate), (34, blank_handle)] {
        send(
            &mut stdin,
            json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{
                    "name":"workbook_close",
                    "arguments":{"handle":close_handle}
                }
            }),
        );
        let closed = tool_text(&receive(&mut stdout));
        assert_eq!(closed["closed"], true);
    }

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"workbook_open",
                "arguments":{"path": workbook.to_string_lossy()}
            }
        }),
    );
    let opened = tool_text(&receive(&mut stdout));
    let handle = opened["handle"]
        .as_str()
        .expect("workbook handle")
        .to_owned();
    assert_eq!(opened.pointer("/summary/version"), Some(&json!("26.2")));

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"workbook_inspect",
                "arguments":{"handle":handle,"kind":"worksheet","query":"Sales"}
            }
        }),
    );
    let inspected = tool_text(&receive(&mut stdout));
    assert_eq!(inspected["count"], 1);
    assert_eq!(inspected["items"][0]["name"], "Sales");

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"tools/call",
            "params":{
                "name":"workbook_edit",
                "arguments":{
                    "handle":handle,
                    "batch":{
                        "edits":[{
                            "op":"create_dashboard",
                            "name":"MCP Dashboard",
                            "width":1200,
                            "height":800,
                            "padding":16,
                            "background":"#FFFFFF",
                            "layout":{
                                "type":"sheet",
                                "sheet":"Sales",
                                "show_title":false,
                                "fit":"entire_view"
                            }
                        }]
                    }
                }
            }
        }),
    );
    let edited = tool_text(&receive(&mut stdout));
    let candidate_handle = edited["handle"]
        .as_str()
        .expect("candidate handle")
        .to_owned();
    assert_eq!(edited.pointer("/summary/dashboards"), Some(&json!(1)));

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":6,
            "method":"tools/call",
            "params":{
                "name":"workbook_inspect",
                "arguments":{"handle":candidate_handle,"kind":"dashboard","query":"MCP Dashboard"}
            }
        }),
    );
    let dashboard = tool_text(&receive(&mut stdout));
    assert_eq!(dashboard["count"], 1);
    assert_eq!(dashboard["items"][0]["name"], "MCP Dashboard");

    for (id, close_handle) in [(7, candidate_handle), (8, handle)] {
        send(
            &mut stdin,
            json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{
                    "name":"workbook_close",
                    "arguments":{"handle":close_handle}
                }
            }),
        );
        let closed = tool_text(&receive(&mut stdout));
        assert_eq!(closed["closed"], true);
    }

    drop(stdin);
    let status = child.wait().expect("wait for tabkit-mcp");
    assert!(status.success(), "tabkit-mcp exited with {status}");
}
