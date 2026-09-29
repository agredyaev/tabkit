#![no_main]
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeSet;
use tabkit_fuzz::{
    config::Limits,
    fs::sha256,
    workbook::{
        Workbook,
        lineage::{Direction, Graph, ImpactCursor, NodeRef},
    },
};

fn check(source: Vec<u8>, must_parse: bool, custom: bool, manual_sort: bool) {
    let limits = Limits {
        xml_bytes: 65536,
        xml_nodes: 10000,
        ..Default::default()
    };
    let book = match Workbook::parse(source.clone(), &limits) {
        Ok(book) => book,
        Err(error) => {
            assert!(!must_parse, "generated workbook failed: {error}");
            return;
        }
    };
    let graph = Graph::build(book, sha256(&source)).unwrap();
    let mut output = Vec::new();
    graph.export(&mut output).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let nodes = json["nodes"].as_array().unwrap();
    let details = json["details"].as_array().unwrap();
    let edges = json["edges"].as_array().unwrap();
    assert_eq!(graph.count(), (nodes.len(), edges.len()));
    assert_eq!(nodes.len(), details.len());
    if must_parse {
        assert_eq!(
            nodes.iter().filter(|node| node["reference"]["kind"] == "custom_encoding").count(),
            usize::from(custom)
        );
        assert_eq!(
            edges.iter().filter(|edge| edge["kind"] == "custom_encoding_field").count(),
            usize::from(custom)
        );
        assert_eq!(
            edges.iter().filter(|edge| edge["kind"] == "sort_field").count(),
            usize::from(manual_sort)
        );
        assert!(!json["gaps"].as_array().unwrap().iter().any(|gap| {
            gap["code"] == "CUSTOM_ENCODING_FIELD" || gap["code"] == "SORT_REFERENCE"
        }));
    }
    let mut refs = BTreeSet::new();
    for node in nodes {
        let reference: NodeRef = serde_json::from_value(node["reference"].clone()).unwrap();
        assert!(refs.insert((reference.kind, reference.id)));
        assert_eq!(
            graph.node(reference).unwrap().name,
            node["name"].as_str().unwrap()
        );
        assert_eq!(
            graph.details(reference).unwrap()["details"],
            details[refs.len() - 1]
        );
        let _ = graph
            .neighbors(reference, Direction::Downstream, 0, 100)
            .unwrap();
    }
    for edge in edges {
        for end in ["from", "to"] {
            let reference: NodeRef = serde_json::from_value(edge[end].clone()).unwrap();
            assert!(refs.contains(&(reference.kind, reference.id)));
        }
    }
    if let Some(first) = nodes.first() {
        let reference: NodeRef = serde_json::from_value(first["reference"].clone()).unwrap();
        let mut cursor = ImpactCursor::new(&graph, reference, Direction::Downstream).unwrap();
        for _ in 0..edges.len() + 1 {
            let page = cursor.page(&graph, 3).unwrap();
            assert!(page["edges"].as_array().unwrap().len() <= 3);
            assert!(
                !page["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["kind"] == "relationship_end")
            );
            if page["done"] == true {
                return;
            }
        }
        panic!("impact pagination did not terminate");
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 4096 {
        return;
    }
    if data.starts_with(b"<") {
        check(data.to_vec(), false, false, false);
        return;
    }
    let bit = |i: usize| data.get(i).copied().unwrap_or(0) & 1 != 0;
    let sql = match data.get(7).copied().unwrap_or(0) % 4 {
        0 => "SELECT id FROM public.orders",
        1 => "WITH c AS (SELECT id FROM public.orders) SELECT id FROM c",
        2 => "CREATE TEMP TABLE x AS SELECT id FROM public.orders",
        _ => "SELECT id FROM audit.events JOIN public.orders USING (id)",
    };
    let relation = if bit(0) {
        format!(
            "<relation type='join' join='left'><clause type='join'><expression op='='><expression op='[id]'/><expression op='[id]'/></expression></clause><relation type='table' name='orders' table='[public].[orders]' connection='c'/><relation type='text' name='q' connection='c'>{sql}</relation></relation>"
        )
    } else {
        "<relation type='table' name='orders' table='[public].[orders]' connection='c'/>".into()
    };
    let extract = if bit(1) {
        "<properties context='extract'><relation type='table' name='Extract' table='[Extract].[Extract]'/></properties>"
    } else {
        ""
    };
    let relationship = if bit(2) {
        "<object id='lookup'><properties context=''><relation type='table' name='lookup' table='[public].[lookup]' connection='c'/></properties></object>"
    } else {
        ""
    };
    let relationship_edge = if bit(2) {
        "<relationships><relationship><expression op='='><expression op='[id]'/><expression op='[id]'/></expression><first-end-point object-id='orders'/><second-end-point object-id='lookup'/></relationship></relationships>"
    } else {
        ""
    };
    let action = if bit(3) {
        "<actions><edit-group-action name='set action'><source type='sheet' worksheet='S'/><params><param name='target-group' value='[d].[S]'/></params></edit-group-action></actions>"
    } else {
        ""
    };
    let tooltip = if bit(4) {
        "<customized-tooltip><formatted-text><run>&lt;[d].[id]&gt;</run></formatted-text></customized-tooltip>"
    } else {
        ""
    };
    let story = if bit(5) {
        "<stories><story name='Story'><story-points><story-point id='1' captured-sheet='S'/></story-points></story></stories>"
    } else {
        ""
    };
    let table_calc = if bit(6) {
        "<table-calc ordering-type='Field'><order field='[d].[io:S:nk]'/></table-calc>"
    } else {
        ""
    };
    let custom = if bit(8) {
        "<custom custom-type-name='target' column='[d].[sum:id:qk]'/>"
    } else {
        ""
    };
    let manual_sort = if bit(9) {
        "<manual-sort column='[d].[yr:id:ok]' direction='ASC'/>"
    } else {
        ""
    };
    let source = format!(
        "<workbook source-build='2025.3.1' xmlns:user='http://www.tableausoftware.com/xml/user'><datasources><datasource name='d'><named-connections><named-connection name='c'><connection class='postgres'/></named-connection></named-connections><column name='[id]' datatype='integer' role='dimension'/><column name='[calc]' datatype='integer' role='measure'><calculation class='tableau' formula='[id]'>{table_calc}</calculation></column><group name='[S]' user:ui-builder='filter-group'><groupfilter function='level-members' level='[id]'/></group><object-graph><objects><object id='orders'><properties context=''>{relation}</properties>{extract}</object>{relationship}</objects>{relationship_edge}</object-graph></datasource></datasources><worksheets><worksheet name='S'><table><view><datasources><datasource name='d'/></datasources><filter class='categorical' column='[d].[S]'/>{manual_sort}</view><panes><pane><encodings><color column='[d].[io:S:nk]'/>{custom}</encodings>{tooltip}</pane></panes><rows>[d].[io:S:nk]</rows></table></worksheet></worksheets><dashboards><dashboard name='D'><zones><zone name='S'/></zones></dashboard></dashboards>{action}{story}</workbook>"
    );
    check(source.into_bytes(), true, bit(8), bit(9));
});
