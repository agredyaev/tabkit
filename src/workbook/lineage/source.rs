use super::{Extra, Resolver};
use crate::workbook::lineage::{EdgeKind, NodeKind, NodeRef};
use crate::{
    formula,
    workbook::{DatasourceId, Workbook},
    xml::{NodeId, Xml},
};
use serde_json::{Value, json};
use sqlparser::{
    ast::{Query, Statement, TableFactor, Visit, Visitor},
    dialect::GenericDialect,
    parser::Parser,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::ControlFlow,
};

type Connections = BTreeMap<(DatasourceId, String), NodeRef>;
type Tables = BTreeMap<(DatasourceId, String, String, String), NodeRef>;
type Sqls = BTreeMap<(DatasourceId, String, String, String), NodeRef>;
#[derive(Default)]
struct SourceIndex {
    connections: Connections,
    ambiguous_connections: BTreeSet<(DatasourceId, String)>,
    tables: Tables,
    sqls: Sqls,
}

pub(super) fn build(book: &Workbook, resolver: &Resolver<'_>, out: &mut Extra) {
    let xml = &book.xml;
    let Ok(root) = xml.one_child(NodeId(0), "datasources") else {
        return;
    };
    let mut index = SourceIndex::default();
    for (i, ds_node) in xml.named_children(root, "datasource").enumerate() {
        let ds = DatasourceId(i as u32);
        let owner = NodeRef::new(NodeKind::Datasource, i);
        for node in descendants(xml, ds_node) {
            if xml.tag(node) != "connection" {
                continue;
            }
            let parent = xml.node(node).parent();
            let name = parent
                .filter(|&p| xml.tag(p) == "named-connection")
                .and_then(|p| xml.value(p, "name"))
                .unwrap_or_else(|| xml.value(node, "class").unwrap_or("connection"));
            let scope = if xml.ancestor(node, "extract").is_some() {
                "extract"
            } else {
                "source"
            };
            let detail = json!({"class":xml.value(node,"class"),"server":xml.value(node,"server"),
                "database":xml.value(node,"dbname"),"schema":xml.value(node,"schema"),"scope":scope});
            let connection = out.add(NodeKind::Connection, name, name, Some(owner), None, detail);
            out.edge(connection, owner, EdgeKind::ConnectionInDatasource);
            if scope == "source" {
                let key = (ds, name.to_owned());
                if index.ambiguous_connections.contains(&key) {
                    out.gap("CONNECTION_REFERENCE", node, "Connection name is ambiguous");
                } else if index.connections.insert(key.clone(), connection).is_some() {
                    index.connections.remove(&key);
                    index.ambiguous_connections.insert(key);
                    out.gap("CONNECTION_REFERENCE", node, "Connection name is ambiguous");
                }
            }
            if let Some(sql) = xml.value(node, "one-time-sql").filter(|s| !s.is_empty()) {
                let initial = out.add(
                    NodeKind::InitialSql,
                    name,
                    "Initial SQL",
                    Some(owner),
                    None,
                    json!({"sql":sql,"scope":scope}),
                );
                out.edge(connection, initial, EdgeKind::InitialSqlOnConnection);
                if scope == "source" {
                    add_sql_reads(sql, node, ds, name, initial, &mut index, out);
                } else {
                    out.gap(
                        "INITIAL_SQL_SCOPE",
                        node,
                        "Extract connection SQL effects are not modeled",
                    );
                }
            }
        }
        let mut objects = BTreeMap::new();
        for node in descendants(xml, ds_node) {
            if xml.tag(node) != "object" || xml.ancestor(node, "object-graph").is_none() {
                continue;
            }
            let name = xml.value(node, "id").unwrap_or("");
            let caption = xml.value(node, "caption").unwrap_or(name);
            let logical = out.add(
                NodeKind::LogicalTable,
                name,
                caption,
                Some(owner),
                None,
                json!({"object_id":name,"caption":caption}),
            );
            objects.insert(name.to_owned(), logical);
            out.edge(logical, owner, EdgeKind::LogicalInDatasource);
            let mut source_relations = 0;
            for properties in xml.named_children(node, "properties") {
                if xml
                    .value(properties, "context")
                    .is_some_and(|c| !c.is_empty())
                {
                    continue;
                }
                for relation in xml.named_children(properties, "relation") {
                    source_relations += 1;
                    if let Some(input) = relation_node(xml, relation, ds, owner, &mut index, out) {
                        out.edge(
                            input,
                            logical,
                            match input.kind {
                                NodeKind::CustomSql => EdgeKind::CustomSqlInLogical,
                                NodeKind::Join => EdgeKind::JoinOutput,
                                _ => EdgeKind::PhysicalInLogical,
                            },
                        );
                    }
                }
            }
            if source_relations == 0 {
                out.gap(
                    "SOURCE_RELATION",
                    node,
                    "Logical object has no source relation",
                );
            }
        }
        if objects.is_empty() {
            for node in descendants(xml, ds_node) {
                if xml.tag(node) != "relation" || xml.ancestor(node, "extract").is_some() {
                    continue;
                }
                if xml.ancestor(node, "object-graph").is_some() {
                    continue;
                }
                if xml
                    .node(node)
                    .parent()
                    .is_some_and(|p| xml.tag(p) == "relation")
                {
                    continue;
                }
                let name = xml
                    .value(node, "name")
                    .or_else(|| xml.value(node, "table"))
                    .unwrap_or("logical table");
                let logical = out.add(
                    NodeKind::LogicalTable,
                    name,
                    name,
                    Some(owner),
                    None,
                    json!({"source":"legacy_relation"}),
                );
                out.edge(logical, owner, EdgeKind::LogicalInDatasource);
                if let Some(input) = relation_node(xml, node, ds, owner, &mut index, out) {
                    out.edge(
                        input,
                        logical,
                        match input.kind {
                            NodeKind::CustomSql => EdgeKind::CustomSqlInLogical,
                            NodeKind::Join => EdgeKind::JoinOutput,
                            _ => EdgeKind::PhysicalInLogical,
                        },
                    );
                }
            }
        }
        for node in descendants(xml, ds_node) {
            if xml.tag(node) != "relationship" {
                continue;
            }
            let Some(ends) = xml
                .named_children(node, "first-end-point")
                .next()
                .zip(xml.named_children(node, "second-end-point").next())
            else {
                out.gap(
                    "RELATIONSHIP_ENDPOINT",
                    node,
                    "Relationship has no two endpoints",
                );
                continue;
            };
            let (Some(left), Some(right)) = (
                xml.value(ends.0, "object-id"),
                xml.value(ends.1, "object-id"),
            ) else {
                out.gap(
                    "RELATIONSHIP_ENDPOINT",
                    node,
                    "Relationship endpoint lacks object ID",
                );
                continue;
            };
            let (Some(&l), Some(&r)) = (objects.get(left), objects.get(right)) else {
                out.gap(
                    "RELATIONSHIP_ENDPOINT",
                    node,
                    "Relationship object is unresolved",
                );
                continue;
            };
            let expr = xml
                .named_children(node, "expression")
                .next()
                .map(|n| expression(xml, n, 0));
            if expr.is_none() {
                out.gap(
                    "RELATIONSHIP_CONDITION",
                    node,
                    "Relationship has no condition",
                );
            }
            let relation = out.add(NodeKind::Relationship, &format!("{left} ↔ {right}"),
                &format!("{left} ↔ {right}"), Some(owner), None,
                json!({"first_object":left,"second_object":right,"condition":expr,
                    "first_endpoint":xml.attributes(ends.0).map(|a|(a.name.to_owned(),a.value.to_owned())).collect::<BTreeMap<_,_>>(),
                    "second_endpoint":xml.attributes(ends.1).map(|a|(a.name.to_owned(),a.value.to_owned())).collect::<BTreeMap<_,_>>()}));
            out.edge(l, relation, EdgeKind::RelationshipEnd);
            out.edge(r, relation, EdgeKind::RelationshipEnd);
            for e in descendants(xml, node)
                .into_iter()
                .filter(|&n| xml.tag(n) == "expression")
            {
                if let Some(field) =
                    exact_field(resolver, None, Some(ds), xml.value(e, "op").unwrap_or(""))
                {
                    out.edge(field, relation, EdgeKind::RelationshipKey);
                }
            }
        }
        let mut extract_tables = BTreeMap::<(String, String, String), NodeRef>::new();
        let mut extract_aliases = BTreeMap::<String, Vec<NodeRef>>::new();
        for node in descendants(xml, ds_node) {
            if xml.tag(node) != "relation"
                || !(xml.ancestor(node, "extract").is_some()
                    || xml
                        .ancestor(node, "properties")
                        .is_some_and(|p| xml.value(p, "context") == Some("extract")))
            {
                continue;
            }
            if xml.value(node, "type") != Some("table") {
                continue;
            }
            let Some(table) = xml.value(node, "table") else {
                out.gap("EXTRACT_RELATION", node, "Extract table name is missing");
                continue;
            };
            let alias = xml.value(node, "name").unwrap_or(table);
            let connection = xml.value(node, "connection").unwrap_or("");
            let key = (alias.to_owned(), table.to_owned(), connection.to_owned());
            let reference = if let Some(&existing) = extract_tables.get(&key) {
                existing
            } else {
                let reference = out.add(
                    NodeKind::PhysicalTable,
                    table,
                    alias,
                    Some(owner),
                    None,
                    json!({"table":table,"alias":alias,"connection":connection,"scope":"extract"}),
                );
                extract_tables.insert(key, reference);
                extract_aliases
                    .entry(alias.to_owned())
                    .or_default()
                    .push(reference);
                reference
            };
            let logical = xml
                .ancestor(node, "object")
                .and_then(|o| xml.value(o, "id"))
                .and_then(|id| objects.get(id).copied());
            if let Some(logical) = logical {
                out.edge(reference, logical, EdgeKind::ExtractInLogical);
            } else {
                out.edge(reference, owner, EdgeKind::ExtractInDatasource);
            }
        }
        let explicit: BTreeSet<_> = out
            .edges
            .iter()
            .filter(|(_, _, kind)| {
                matches!(kind, EdgeKind::JoinInput | EdgeKind::PhysicalInLogical)
            })
            .map(|(from, _, _)| *from)
            .collect();
        let mut by_alias = BTreeMap::<String, Vec<NodeRef>>::new();
        for ((table_ds, _, alias, _), &reference) in &index.tables {
            if *table_ds == ds && explicit.contains(&reference) {
                by_alias.entry(alias.clone()).or_default().push(reference);
            }
        }
        for ((table_ds, _, alias, _), &reference) in &index.sqls {
            if *table_ds == ds {
                by_alias.entry(alias.clone()).or_default().push(reference);
            }
        }
        for &record in &xml.semantic.metadata_records {
            if xml.ancestor(record, "datasource") != Some(ds_node)
                || xml.value(record, "class") != Some("column")
            {
                continue;
            }
            let Some(parent) = xml
                .named_children(record, "parent-name")
                .next()
                .and_then(|n| xml.text_content(n).ok())
            else {
                continue;
            };
            let Some(local) = xml
                .named_children(record, "local-name")
                .next()
                .and_then(|n| xml.text_content(n).ok())
            else {
                continue;
            };
            let Some(&field) = book.field_lookup[i].get(local) else {
                continue;
            };
            if book.fields[field.0 as usize].formula.is_some() {
                continue;
            }
            let alias = unquote(parent);
            let (origins, edge_kind) = match (by_alias.get(alias), extract_aliases.get(alias)) {
                (Some(_), Some(_)) => {
                    out.gap("FIELD_ORIGIN", record, "Source and extract aliases overlap");
                    continue;
                }
                (Some(origins), None) => (origins, EdgeKind::FieldOrigin),
                (None, Some(origins)) => (origins, EdgeKind::ExtractFieldOrigin),
                (None, None) => {
                    out.gap(
                        "FIELD_ORIGIN",
                        record,
                        "Physical field source relation is unresolved",
                    );
                    continue;
                }
            };
            if origins.len() != 1 {
                out.gap("FIELD_ORIGIN", record, "Field source relation is ambiguous");
                continue;
            }
            out.edge(
                origins[0],
                NodeRef::new(NodeKind::Field, field.0 as usize),
                edge_kind,
            );
        }
    }
}

fn relation_node(
    xml: &Xml,
    n: NodeId,
    ds: DatasourceId,
    owner: NodeRef,
    index: &mut SourceIndex,
    out: &mut Extra,
) -> Option<NodeRef> {
    match xml.value(n, "type").unwrap_or("") {
        "table" => {
            let Some(table) = xml.value(n, "table") else {
                out.gap("SOURCE_RELATION", n, "Physical relation has no table name");
                return None;
            };
            let alias = xml.value(n, "name").unwrap_or(table);
            let connection = xml.value(n, "connection").unwrap_or("");
            if !connection.is_empty() && !index.connections.contains_key(&(ds, connection.into())) {
                out.gap(
                    "CONNECTION_REFERENCE",
                    n,
                    "Physical relation connection is unresolved",
                );
            }
            Some(physical(ds, connection, alias, table, owner, index, out))
        }
        "text" => {
            let sql = match xml.text_content(n) {
                Ok(s) => s,
                Err(_) => {
                    out.gap("CUSTOM_SQL", n, "Custom SQL is not leaf text");
                    return None;
                }
            };
            let alias = xml.value(n, "name").unwrap_or("Custom SQL");
            let connection = xml.value(n, "connection").unwrap_or("");
            if !connection.is_empty() && !index.connections.contains_key(&(ds, connection.into())) {
                out.gap(
                    "CONNECTION_REFERENCE",
                    n,
                    "Custom SQL connection is unresolved",
                );
            }
            let key = (
                ds,
                connection.into(),
                alias.into(),
                crate::fs::sha256(sql.as_bytes()),
            );
            if let Some(&existing) = index.sqls.get(&key) {
                return Some(existing);
            }
            let reference = out.add(
                NodeKind::CustomSql,
                alias,
                alias,
                Some(owner),
                None,
                json!({"sql":sql,"connection":connection}),
            );
            index.sqls.insert(key, reference);
            add_sql_reads(sql, n, ds, connection, reference, index, out);
            Some(reference)
        }
        "join" => {
            let join_type = xml.value(n, "join").unwrap_or("unknown");
            let conditions: Vec<_> = xml
                .named_children(n, "clause")
                .map(|c| {
                    let expr = xml.named_children(c, "expression").next();
                    if expr.is_none() {
                        out.gap("JOIN_CONDITION", c, "Join clause has no expression");
                    }
                    json!({"type":xml.value(c,"type"),
                        "expression":expr.map(|e|expression(xml,e,0))})
                })
                .collect();
            if conditions.is_empty() {
                out.gap("JOIN_CONDITION", n, "Join has no clauses");
            }
            let condition = conditions
                .first()
                .and_then(|c| c.get("expression"))
                .cloned();
            let reference = out.add(
                NodeKind::Join,
                join_type,
                &format!("{join_type} join"),
                Some(owner),
                None,
                json!({"join_type":join_type,"condition":condition,"conditions":conditions}),
            );
            if xml.named_children(n, "relation").count() != 2 {
                out.gap("JOIN_INPUT", n, "Join does not have exactly two inputs");
            }
            for child in xml.named_children(n, "relation") {
                if let Some(input) = relation_node(xml, child, ds, owner, index, out) {
                    out.edge(input, reference, EdgeKind::JoinInput);
                }
            }
            Some(reference)
        }
        "collection" => {
            out.gap(
                "SOURCE_RELATION",
                n,
                "Collection relation needs an explicit semantic model",
            );
            None
        }
        _ => {
            out.gap("SOURCE_RELATION", n, "Unknown relation type");
            None
        }
    }
}

fn physical(
    ds: DatasourceId,
    connection: &str,
    alias: &str,
    table: &str,
    owner: NodeRef,
    index: &mut SourceIndex,
    out: &mut Extra,
) -> NodeRef {
    let key = (ds, connection.into(), alias.into(), table.into());
    if let Some(&r) = index.tables.get(&key) {
        return r;
    }
    let reference = out.add(
        NodeKind::PhysicalTable,
        table,
        alias,
        Some(owner),
        None,
        json!({"table":table,"alias":alias,"connection":connection,"scope":"source"}),
    );
    if let Some(&conn) = index.connections.get(&(ds, connection.into())) {
        out.edge(conn, reference, EdgeKind::TableOnConnection);
    }
    index.tables.insert(key, reference);
    reference
}

fn add_sql_reads(
    sql: &str,
    node: NodeId,
    ds: DatasourceId,
    connection: &str,
    sql_node: NodeRef,
    index: &mut SourceIndex,
    out: &mut Extra,
) {
    match read_tables(sql) {
        Ok(reads) => {
            for table in reads {
                let r = physical(
                    ds,
                    connection,
                    &table,
                    &table,
                    NodeRef::new(NodeKind::Datasource, ds.0 as usize),
                    index,
                    out,
                );
                out.edge(r, sql_node, EdgeKind::SqlRead);
            }
        }
        Err(()) => out.gap(
            "SQL_REFERENCES",
            node,
            "SQL table references are not safely resolved",
        ),
    }
}

fn expression(xml: &Xml, node: NodeId, depth: usize) -> Value {
    if depth == 64 {
        return json!({"depth_limit":true});
    }
    json!({"op":xml.value(node,"op"),
        "args":xml.named_children(node,"expression").map(|n|expression(xml,n,depth+1)).collect::<Vec<_>>()})
}

fn exact_field(
    resolver: &Resolver<'_>,
    sheet: Option<NodeId>,
    ds: Option<DatasourceId>,
    text: &str,
) -> Option<NodeRef> {
    let analysis = formula::analyze(text).ok()?;
    if analysis.references.len() != 1 {
        return None;
    }
    resolver.reference(sheet, ds, &analysis.references[0])
}

pub(super) fn descendants(xml: &Xml, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<_> = xml.children(root).collect();
    while let Some(node) = stack.pop() {
        out.push(node);
        stack.extend(xml.children(node));
    }
    out
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(value)
}

struct Reads {
    scopes: Vec<BTreeSet<String>>,
    names: BTreeSet<String>,
    unsupported: bool,
}
impl Visitor for Reads {
    type Break = ();
    fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<()> {
        let aliases = q
            .with
            .as_ref()
            .map(|w| {
                w.cte_tables
                    .iter()
                    .map(|c| c.alias.name.value.to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        self.scopes.push(aliases);
        ControlFlow::Continue(())
    }
    fn post_visit_query(&mut self, _: &Query) -> ControlFlow<()> {
        self.scopes.pop();
        ControlFlow::Continue(())
    }
    fn pre_visit_table_factor(&mut self, f: &TableFactor) -> ControlFlow<()> {
        match f {
            TableFactor::Table { name, args, .. } => {
                if args.is_some() {
                    self.unsupported = true;
                } else {
                    let rendered = name.to_string();
                    let cte = name.0.len() == 1
                        && self
                            .scopes
                            .iter()
                            .any(|scope| scope.contains(&name.0[0].value.to_lowercase()));
                    if !cte {
                        self.names.insert(rendered);
                    }
                }
            }
            TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => {}
            _ => self.unsupported = true,
        }
        ControlFlow::Continue(())
    }
}
fn read_tables(sql: &str) -> Result<BTreeSet<String>, ()> {
    let statements = Parser::parse_sql(&GenericDialect {}, sql).map_err(|_| ())?;
    if statements.is_empty() || statements.iter().any(|s| !matches!(s, Statement::Query(_))) {
        return Err(());
    }
    let mut reads = Reads {
        scopes: Vec::new(),
        names: BTreeSet::new(),
        unsupported: false,
    };
    let _ = statements.visit(&mut reads);
    if reads.unsupported {
        Err(())
    } else {
        Ok(reads.names)
    }
}

#[cfg(test)]
mod tests {
    use super::read_tables;
    #[test]
    fn sql_reads_exclude_cte_names_and_reject_effects() {
        let reads=read_tables("WITH c AS (SELECT id FROM public.orders) SELECT id FROM c JOIN audit.events e ON c.id=e.id").unwrap();
        assert_eq!(
            reads.into_iter().collect::<Vec<_>>(),
            ["audit.events", "public.orders"]
        );
        assert_eq!(
            read_tables("WITH \"C\" AS (SELECT * FROM public.orders) SELECT * FROM \"C\"")
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["public.orders"]
        );
        assert!(read_tables("CREATE TEMP TABLE t AS SELECT * FROM public.orders").is_err());
    }
}
