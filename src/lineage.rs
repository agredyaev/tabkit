//! Evidence-backed workbook lineage graph.
//!
//! Edges are emitted only when the workbook structure or supported SQL analysis
//! proves a relationship. Unsupported, ambiguous, or partial provenance becomes
//! an explicit [`Gap`]; TabKit never fills a gap with a guessed edge.
//!
//! SQL Server local `#temp` tables are connection-scoped. They may link Initial
//! SQL producers to Custom SQL consumers only within the same Tableau connection;
//! global temp tables are reported as unsupported coverage instead.

use crate::{
    Error, Result, formula,
    text::{FastHashMap, FastHashSet, ascii_case_hash, contains_case_insensitive},
    workbook::{
        ConnectionId, DatasourceId, FieldId, RelationId, RelationKind, Workbook, WorksheetId,
    },
    xml::NodeId,
};
use serde::Serialize;
use sqlparser::{
    ast::{Query, SetExpr, Statement, TableFactor, TableObject, Visit, Visitor},
    dialect::{GenericDialect, MsSqlDialect},
    parser::Parser,
};
use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    ops::ControlFlow,
};

/// Semantic category used to type graph nodes and search filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Datasource,
    Connection,
    InitialSql,
    PhysicalTable,
    CustomSql,
    SqlTable,
    Join,
    LogicalTable,
    Relationship,
    Field,
    Calculation,
    Worksheet,
    Dashboard,
}

impl std::str::FromStr for Kind {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "datasource" => Ok(Self::Datasource),
            "connection" => Ok(Self::Connection),
            "initial_sql" => Ok(Self::InitialSql),
            "physical_table" => Ok(Self::PhysicalTable),
            "custom_sql" => Ok(Self::CustomSql),
            "sql_table" => Ok(Self::SqlTable),
            "join" => Ok(Self::Join),
            "logical_table" => Ok(Self::LogicalTable),
            "relationship" => Ok(Self::Relationship),
            "field" => Ok(Self::Field),
            "calculation" => Ok(Self::Calculation),
            "worksheet" => Ok(Self::Worksheet),
            "dashboard" => Ok(Self::Dashboard),
            _ => Err(Error::new(
                "ARGUMENT",
                format!("Unknown lineage kind {value:?}"),
            )),
        }
    }
}

/// Evidence-backed relationship; direction is from dependency/source toward consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    ConnectionInDatasource,
    InitialSqlOnConnection,
    ConnectionSetup,
    SourceInDatasource,
    SqlRead,
    TempTableInput,
    TempTableWrite,
    SourceInLogical,
    JoinInput,
    RelationshipEnd,
    FieldInDatasource,
    FieldOrigin,
    CalculationInput,
    CalculationOutput,
    WorksheetUse,
    SheetInDashboard,
}

/// Workbook object represented by a graph-local dense ID.
#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub id: u32,
    pub kind: Kind,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Edge {
    pub from: u32,
    pub to: u32,
    pub kind: EdgeKind,
}

/// Machine-readable record for unresolved provenance.
#[derive(Clone, Debug, Serialize)]
pub struct Gap {
    pub code: &'static str,
    pub context: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upstream,
    Downstream,
}

#[derive(Serialize)]
pub struct Trace<'a> {
    pub root: &'a Node,
    pub nodes: Vec<&'a Node>,
    pub edges: Vec<Edge>,
    pub truncated: bool,
}

#[derive(Clone, Copy)]
enum ExactNode {
    One(u32),
    Ambiguous,
}

/// Immutable graph plus explicit coverage gaps for one parsed workbook snapshot.
pub struct Lineage {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub gaps: Vec<Gap>,
    exact_names: FastHashMap<u64, ExactNode>,
    outgoing: Adjacency,
    incoming: Adjacency,
}

struct Adjacency {
    offsets: Vec<u32>,
    edge_ids: Vec<u32>,
}

impl Adjacency {
    fn edge_ids(&self, node: u32) -> &[u32] {
        let start = self.offsets[node as usize] as usize;
        let end = self.offsets[node as usize + 1] as usize;
        &self.edge_ids[start..end]
    }
}

impl Lineage {
    /// Builds a graph without inventing provenance for unsupported structures.
    ///
    /// Successful construction does not imply complete lineage: callers must also
    /// inspect [`Lineage::gaps`] when completeness matters.
    pub fn build(book: &Workbook) -> Result<Self> {
        let mut builder = Builder::new(book);
        builder.build()?;
        Ok(builder.finish())
    }

    /// Resolves names case-insensitively, preferring exact matches over substrings.
    ///
    /// If at least one exact match exists, partial matches are suppressed. This
    /// keeps a precise object request from being widened silently.
    pub fn find(&self, text: &str) -> Vec<&Node> {
        self.find_kind(text, None)
    }

    /// Applies the same exact-first lookup as [`Self::find`] with an optional kind filter.
    pub fn find_kind(&self, text: &str, kind: Option<Kind>) -> Vec<&Node> {
        if let Some(ExactNode::One(id)) = self.exact_names.get(&ascii_case_hash(text))
            && let Some(node) = self.nodes.get(*id as usize)
            && node.name.eq_ignore_ascii_case(text)
            && kind.is_none_or(|wanted| node.kind == wanted)
        {
            return vec![node];
        }

        let mut exact = Vec::new();
        let mut partial = Vec::new();
        for node in &self.nodes {
            if kind.is_some_and(|wanted| node.kind != wanted) {
                continue;
            }
            if node.name.eq_ignore_ascii_case(text) {
                exact.push(node);
            } else if contains_case_insensitive(&node.name, text) {
                partial.push(node);
            }
        }
        if exact.is_empty() { partial } else { exact }
    }

    /// Traverses all reachable proven edges from a graph-local node ID.
    ///
    /// Returns `None` when the node ID is not part of this graph. Coverage gaps
    /// remain separate metadata and are never converted into traversal edges.
    pub fn trace(&self, node_id: u32, direction: Direction) -> Option<Trace<'_>> {
        self.trace_limited(node_id, direction, usize::MAX)
    }

    /// Traverses proven lineage while bounding the number of returned nodes.
    ///
    /// The root counts toward `max_nodes`; zero therefore returns `None`. When
    /// additional reachable nodes exist beyond the bound, `Trace::truncated` is
    /// set rather than silently dropping the fact that traversal was incomplete.
    pub fn trace_limited(
        &self,
        node_id: u32,
        direction: Direction,
        max_nodes: usize,
    ) -> Option<Trace<'_>> {
        let root = self.nodes.get(node_id as usize)?;
        if max_nodes == 0 {
            return None;
        }

        let capacity = max_nodes.min(self.nodes.len()).min(1024);
        let mut seen = FastHashSet::default();
        seen.reserve(capacity);
        let mut visited = Vec::with_capacity(capacity);
        let mut queue = VecDeque::from([node_id]);
        let mut trace_edge_ids = Vec::new();
        let mut truncated = false;
        seen.insert(node_id);
        visited.push(node_id);

        while let Some(current) = queue.pop_front() {
            let edge_ids = match direction {
                Direction::Downstream => self.outgoing.edge_ids(current),
                Direction::Upstream => self.incoming.edge_ids(current),
            };
            let has_precise_origin = edge_ids
                .iter()
                .any(|&edge_id| self.edges[edge_id as usize].kind == EdgeKind::FieldOrigin);

            for &edge_id in edge_ids {
                let edge = self.edges[edge_id as usize];
                let suppressed_coarse_origin = has_precise_origin
                    && matches!(
                        (direction, edge.kind),
                        (Direction::Downstream, EdgeKind::SourceInDatasource)
                            | (Direction::Upstream, EdgeKind::FieldInDatasource)
                    );
                if suppressed_coarse_origin {
                    continue;
                }

                trace_edge_ids.push(edge_id);
                let next = match direction {
                    Direction::Downstream => edge.to,
                    Direction::Upstream => edge.from,
                };
                if seen.contains(&next) {
                    continue;
                }
                if visited.len() == max_nodes {
                    truncated = true;
                    continue;
                }
                seen.insert(next);
                visited.push(next);
                queue.push_back(next);
            }
        }

        visited.sort_unstable();
        let nodes = visited
            .iter()
            .map(|&id| &self.nodes[id as usize])
            .collect::<Vec<_>>();

        trace_edge_ids.retain(|&edge_id| {
            let edge = self.edges[edge_id as usize];
            seen.contains(&edge.from) && seen.contains(&edge.to)
        });
        trace_edge_ids.sort_unstable();
        trace_edge_ids.dedup();
        let edges = trace_edge_ids
            .into_iter()
            .map(|edge_id| self.edges[edge_id as usize])
            .collect();

        Some(Trace {
            root,
            nodes,
            edges,
            truncated,
        })
    }
}

struct Builder<'a> {
    book: &'a Workbook,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    gaps: Vec<Gap>,
    datasources: Vec<u32>,
    connections: Vec<u32>,
    initial_sql: Vec<Option<u32>>,
    relations: Vec<u32>,
    fields: Vec<u32>,
    calculations: Vec<u32>,
    worksheets: Vec<u32>,
    dashboards: Vec<u32>,
    relation_by_xml: HashMap<NodeId, RelationId>,
    connection_by_name: HashMap<(DatasourceId, String), ConnectionId>,
    sql_tables: HashMap<(DatasourceId, String), u32>,
    temp_tables: HashMap<(ConnectionId, String), u32>,
    temp_producers: HashSet<(ConnectionId, String)>,
}

impl<'a> Builder<'a> {
    fn new(book: &'a Workbook) -> Self {
        let node_capacity = book.datasources.len()
            + book.connections.len() * 2
            + book.relations.len()
            + book.fields.len()
            + book.calculations.len()
            + book.worksheets.len()
            + book.dashboards.len();
        let edge_capacity = book.connections.len() * 3
            + book.relations.len() * 3
            + book.fields.len() * 2
            + book.calculations.len() * 4
            + book.worksheets.len() * 8
            + book.dashboards.len() * 8;

        let mut connection_by_name = HashMap::with_capacity(book.connections.len());
        for connection in &book.connections {
            if !connection.name.is_empty() {
                connection_by_name
                    .entry((connection.datasource, connection.name.clone()))
                    .or_insert(connection.id);
            }
        }

        Self {
            book,
            nodes: Vec::with_capacity(node_capacity),
            edges: Vec::with_capacity(edge_capacity),
            gaps: Vec::new(),
            datasources: Vec::with_capacity(book.datasources.len()),
            connections: Vec::with_capacity(book.connections.len()),
            initial_sql: Vec::with_capacity(book.connections.len()),
            relations: Vec::with_capacity(book.relations.len()),
            fields: Vec::with_capacity(book.fields.len()),
            calculations: Vec::with_capacity(book.calculations.len()),
            worksheets: Vec::with_capacity(book.worksheets.len()),
            dashboards: Vec::with_capacity(book.dashboards.len()),
            relation_by_xml: HashMap::with_capacity(book.relations.len()),
            connection_by_name,
            sql_tables: HashMap::new(),
            temp_tables: HashMap::new(),
            temp_producers: HashSet::new(),
        }
    }

    fn build(&mut self) -> Result<()> {
        self.add_primary_nodes();
        self.add_source_edges();
        self.add_field_edges()?;
        self.add_calculation_edges()?;
        self.add_worksheet_edges()?;
        self.add_dashboard_edges()?;
        Ok(())
    }

    fn add_primary_nodes(&mut self) {
        for datasource in &self.book.datasources {
            let name = datasource
                .caption
                .as_deref()
                .filter(|caption| !caption.is_empty())
                .unwrap_or(&datasource.name);
            let node = self.add(Kind::Datasource, name);
            self.datasources.push(node);
        }

        for connection in &self.book.connections {
            let node = self.add(Kind::Connection, &connection.name);
            self.connections.push(node);
            let initial = if connection.initial_sql.is_some() {
                Some(self.add(
                    Kind::InitialSql,
                    &format!("Initial SQL: {}", connection.name),
                ))
            } else {
                None
            };
            self.initial_sql.push(initial);
        }

        for relation in &self.book.relations {
            let kind = match relation.kind {
                RelationKind::PhysicalTable => Kind::PhysicalTable,
                RelationKind::CustomSql => Kind::CustomSql,
                RelationKind::Join => Kind::Join,
                RelationKind::LogicalTable => Kind::LogicalTable,
                RelationKind::Relationship => Kind::Relationship,
            };
            let node = self.add(kind, &relation.name);
            self.relations.push(node);
            self.relation_by_xml.insert(relation.node, relation.id);
        }

        for field in &self.book.fields {
            let name = field.caption.as_deref().unwrap_or(&field.name);
            let node = self.add(Kind::Field, name);
            self.fields.push(node);
        }

        for calculation in &self.book.calculations {
            let field = &self.book.fields[calculation.field.0 as usize];
            let name = field.caption.as_deref().unwrap_or(&field.name);
            let node = self.add(Kind::Calculation, name);
            self.calculations.push(node);
        }

        for worksheet in &self.book.worksheets {
            let node = self.add(Kind::Worksheet, &worksheet.name);
            self.worksheets.push(node);
        }

        for dashboard in &self.book.dashboards {
            let node = self.add(Kind::Dashboard, &dashboard.name);
            self.dashboards.push(node);
        }
    }

    fn add_source_edges(&mut self) {
        for connection in &self.book.connections {
            let connection_node = self.connections[connection.id.0 as usize];
            let datasource_node = self.datasources[connection.datasource.0 as usize];
            self.edge(
                connection_node,
                datasource_node,
                EdgeKind::ConnectionInDatasource,
            );
            if let Some(initial_node) = self.initial_sql[connection.id.0 as usize] {
                self.edge(
                    connection_node,
                    initial_node,
                    EdgeKind::InitialSqlOnConnection,
                );
                self.edge(initial_node, datasource_node, EdgeKind::ConnectionSetup);
                if let Some(sql) = &connection.initial_sql {
                    match analyze_initial_sql(sql, connection.class.as_deref()) {
                        Ok(analysis) => {
                            for write in &analysis.temp_writes {
                                if is_global_temp_name(&write.target) {
                                    self.gap(
                                        "global_temp_table_unsupported",
                                        connection.name.clone(),
                                        format!(
                                            "SQL Server global temporary table {:?} is produced in Initial SQL; TabKit does not guess its server/session scope",
                                            write.target
                                        ),
                                    );
                                    continue;
                                }
                                let temp_node = self.temp_table(connection.id, &write.target);
                                let canonical = canonical_temp_name(&write.target)
                                    .unwrap_or_else(|| write.target.to_ascii_lowercase());
                                self.temp_producers.insert((connection.id, canonical));
                                self.edge(initial_node, temp_node, EdgeKind::TempTableWrite);
                            }

                            for write in &analysis.temp_writes {
                                if is_global_temp_name(&write.target) {
                                    continue;
                                }
                                let temp_node = self.temp_table(connection.id, &write.target);
                                for source in &write.sources {
                                    let source_node = if is_global_temp_name(source) {
                                        self.gap(
                                            "global_temp_table_unsupported",
                                            connection.name.clone(),
                                            format!(
                                                "SQL Server global temporary table {source:?} feeds local temporary table {:?}; producer scope is unresolved",
                                                write.target
                                            ),
                                        );
                                        self.sql_table(connection.datasource, source)
                                    } else if canonical_temp_name(source).is_some() {
                                        self.temp_table(connection.id, source)
                                    } else {
                                        self.sql_table(connection.datasource, source)
                                    };
                                    self.edge(source_node, temp_node, EdgeKind::TempTableInput);
                                }
                            }

                            for table in &analysis.direct_reads {
                                let table_node = self.sql_read_node(
                                    connection.datasource,
                                    Some(connection.id),
                                    table,
                                    &connection.name,
                                );
                                self.edge(table_node, initial_node, EdgeKind::SqlRead);
                            }

                            if analysis.partial {
                                self.gap(
                                    "initial_sql_lineage_partial",
                                    connection.name.clone(),
                                    "Initial SQL contains statements outside the modeled query/temp-table subset",
                                );
                            }
                        }
                        Err(()) => self.gap(
                            "initial_sql_lineage_unresolved",
                            connection.name.clone(),
                            "Initial SQL could not be reduced to a safe set of source tables/temp-table materializations",
                        ),
                    }
                }
            }
        }

        let mut object_by_id = HashMap::<(DatasourceId, String), u32>::new();
        for relation in &self.book.relations {
            let node = self.relations[relation.id.0 as usize];
            let datasource = self.datasources[relation.datasource.0 as usize];

            if relation.kind == RelationKind::LogicalTable {
                if let Ok(Some(object_id)) = self.book.xml.attr(relation.node, "id") {
                    object_by_id.insert((relation.datasource, object_id.into_owned()), node);
                }
            } else {
                self.edge(node, datasource, EdgeKind::SourceInDatasource);
            }

            if relation.kind == RelationKind::CustomSql
                && let Some(sql) = &relation.sql
            {
                let connection_id = self
                    .relation_connection_id(relation.datasource, relation.connection.as_deref());
                let connection_class = connection_id
                    .and_then(|id| self.book.connections.get(id.0 as usize))
                    .and_then(|connection| connection.class.as_deref());
                match read_tables_with_flavor(sql, sql_flavor(connection_class, sql)) {
                    Ok(reads) => {
                        for table in reads {
                            let table_node = self.sql_read_node(
                                relation.datasource,
                                connection_id,
                                &table,
                                &relation.name,
                            );
                            self.edge(table_node, node, EdgeKind::SqlRead);
                        }
                    }
                    Err(()) => self.gap(
                        "sql_lineage_unresolved",
                        relation.name.clone(),
                        "Custom SQL could not be reduced to a safe set of source tables",
                    ),
                }
            }

            if relation.kind == RelationKind::Join {
                for child in self
                    .book
                    .xml
                    .children(relation.node)
                    .filter(|child| self.book.xml.tag(*child) == "relation")
                {
                    if let Some(source_id) = self.relation_by_xml.get(&child) {
                        self.edge(
                            self.relations[source_id.0 as usize],
                            node,
                            EdgeKind::JoinInput,
                        );
                    } else {
                        self.gap(
                            "join_input_unresolved",
                            relation.name.clone(),
                            "Join contains a relation shape that is not indexed",
                        );
                    }
                }
            }

            if relation.kind == RelationKind::LogicalTable {
                for properties in self
                    .book
                    .xml
                    .children(relation.node)
                    .filter(|node| self.book.xml.tag(*node) == "properties")
                {
                    if self
                        .book
                        .xml
                        .attr_raw(properties, "context")
                        .is_some_and(|context| !context.is_empty())
                    {
                        continue;
                    }
                    for child in self
                        .book
                        .xml
                        .children(properties)
                        .filter(|node| self.book.xml.tag(*node) == "relation")
                    {
                        if let Some(source_id) = self.relation_by_xml.get(&child) {
                            self.edge(
                                self.relations[source_id.0 as usize],
                                node,
                                EdgeKind::SourceInLogical,
                            );
                        }
                    }
                }
            }
        }

        for datasource in &self.book.datasources {
            let mut extract_scopes = Vec::<u32>::new();
            for relation_node in self.book.xml.descendants(datasource.node) {
                while extract_scopes
                    .last()
                    .is_some_and(|end| relation_node.0 >= *end)
                {
                    extract_scopes.pop();
                }
                let tag = self.book.xml.tag(relation_node);
                if tag == "extract" {
                    extract_scopes.push(self.book.xml.subtree_end(relation_node));
                    continue;
                }
                if tag == "relation"
                    && extract_scopes.is_empty()
                    && !self.relation_by_xml.contains_key(&relation_node)
                {
                    self.gap(
                        "relation_shape_unsupported",
                        datasource.name.clone(),
                        format!(
                            "Relation type {:?} is not yet modeled",
                            self.book.xml.attr_raw(relation_node, "type")
                        ),
                    );
                }
            }
        }

        for relation in &self.book.relations {
            if relation.kind != RelationKind::Relationship {
                continue;
            }
            let relationship = self.relations[relation.id.0 as usize];
            for endpoint in self.book.xml.children(relation.node).filter(|node| {
                matches!(
                    self.book.xml.tag(*node),
                    "first-end-point" | "second-end-point"
                )
            }) {
                if let Ok(Some(object_id)) = self.book.xml.attr(endpoint, "object-id")
                    && let Some(&logical) =
                        object_by_id.get(&(relation.datasource, object_id.into_owned()))
                {
                    self.edge(logical, relationship, EdgeKind::RelationshipEnd);
                }
            }
        }
    }

    fn add_field_edges(&mut self) -> Result<()> {
        for field in &self.book.fields {
            let field_node = self.fields[field.id.0 as usize];
            self.edge(
                self.datasources[field.datasource.0 as usize],
                field_node,
                EdgeKind::FieldInDatasource,
            );
        }

        let mut aliases = HashMap::<(DatasourceId, String), Vec<u32>>::new();
        for relation in &self.book.relations {
            if matches!(
                relation.kind,
                RelationKind::PhysicalTable | RelationKind::CustomSql
            ) {
                aliases
                    .entry((relation.datasource, relation.name.clone()))
                    .or_default()
                    .push(self.relations[relation.id.0 as usize]);
                if let Some(table) = &relation.table {
                    aliases
                        .entry((relation.datasource, unquote(table).to_owned()))
                        .or_default()
                        .push(self.relations[relation.id.0 as usize]);
                }
            }
        }

        for datasource in &self.book.datasources {
            for record in self
                .book
                .xml
                .descendants(datasource.node)
                .filter(|node| self.book.xml.tag(*node) == "metadata-record")
            {
                if self.book.xml.attr_raw(record, "class") != Some("column") {
                    continue;
                }
                let local = self
                    .book
                    .xml
                    .direct_child(record, "local-name")
                    .and_then(|node| self.book.xml.text(node))
                    .map(str::trim);
                let parent = self
                    .book
                    .xml
                    .direct_child(record, "parent-name")
                    .and_then(|node| self.book.xml.text(node))
                    .map(str::trim);
                let (Some(local), Some(parent)) = (local, parent) else {
                    continue;
                };
                let Some(field) = self.book.field_id(datasource.id, local) else {
                    continue;
                };
                let key = (datasource.id, unquote(parent).to_owned());
                if let Some(origins) = aliases.get(&key)
                    && origins.len() == 1
                {
                    self.edge(
                        origins[0],
                        self.fields[field.0 as usize],
                        EdgeKind::FieldOrigin,
                    );
                }
            }
        }
        Ok(())
    }

    fn add_calculation_edges(&mut self) -> Result<()> {
        for calculation in &self.book.calculations {
            let calculation_node = self.calculations[calculation.id.0 as usize];
            let output_field = &self.book.fields[calculation.field.0 as usize];
            self.edge(
                calculation_node,
                self.fields[output_field.id.0 as usize],
                EdgeKind::CalculationOutput,
            );

            let analysis = match formula::analyze(&calculation.formula) {
                Ok(analysis) => analysis,
                Err(error) => {
                    self.gap(
                        "calculation_formula_unresolved",
                        output_field
                            .caption
                            .clone()
                            .unwrap_or_else(|| output_field.name.clone()),
                        error.message,
                    );
                    continue;
                }
            };
            for reference in analysis.references {
                let datasource = match reference.datasource.as_deref() {
                    Some(name) => {
                        let Some(datasource) = self.book.datasource_id(name) else {
                            self.gap(
                                "calculation_datasource_unresolved",
                                output_field
                                    .caption
                                    .clone()
                                    .unwrap_or_else(|| output_field.name.clone()),
                                format!("Cannot resolve datasource qualifier {name:?}"),
                            );
                            continue;
                        };
                        datasource
                    }
                    None => output_field.datasource,
                };
                if let Some(input) = self.book.field_id(datasource, &reference.field) {
                    self.edge(
                        self.fields[input.0 as usize],
                        calculation_node,
                        EdgeKind::CalculationInput,
                    );
                } else {
                    self.gap(
                        "calculation_reference_unresolved",
                        output_field
                            .caption
                            .clone()
                            .unwrap_or_else(|| output_field.name.clone()),
                        format!("Cannot resolve formula reference {}", reference.field),
                    );
                }
            }
        }
        Ok(())
    }

    fn add_worksheet_edges(&mut self) -> Result<()> {
        for worksheet in &self.book.worksheets {
            let worksheet_node = self.worksheets[worksheet.id.0 as usize];
            let mut used = BTreeSet::<FieldId>::new();

            for dependency in self
                .book
                .xml
                .descendants(worksheet.node)
                .filter(|node| self.book.xml.tag(*node) == "datasource-dependencies")
            {
                let Some(datasource_name) = self
                    .book
                    .xml
                    .attr(dependency, "datasource")?
                    .map(|value| value.into_owned())
                else {
                    continue;
                };
                let Some(datasource) = self.book.datasource_id(&datasource_name) else {
                    self.gap(
                        "worksheet_datasource_unresolved",
                        worksheet.name.clone(),
                        format!("Datasource dependency {datasource_name:?} is not indexed"),
                    );
                    continue;
                };

                for child in self.book.xml.children(dependency) {
                    let name = match self.book.xml.tag(child) {
                        "column" => self
                            .book
                            .xml
                            .attr(child, "name")?
                            .map(|value| value.into_owned()),
                        "column-instance" => self
                            .book
                            .xml
                            .attr(child, "column")?
                            .map(|value| value.into_owned()),
                        _ => None,
                    };
                    let Some(name) = name else {
                        continue;
                    };
                    if let Some(field) = self.book.field_id(datasource, &name) {
                        used.insert(field);
                    } else {
                        self.gap(
                            "worksheet_field_unresolved",
                            worksheet.name.clone(),
                            format!(
                                "Datasource dependency field {name:?} is not present in datasource {datasource_name:?}"
                            ),
                        );
                    }
                }
            }

            for field in used {
                self.edge(
                    self.fields[field.0 as usize],
                    worksheet_node,
                    EdgeKind::WorksheetUse,
                );
            }
        }
        Ok(())
    }

    fn add_dashboard_edges(&mut self) -> Result<()> {
        for dashboard in &self.book.dashboards {
            let dashboard_node = self.dashboards[dashboard.id.0 as usize];
            let mut worksheets = BTreeSet::<WorksheetId>::new();
            for zone in self
                .book
                .xml
                .descendants(dashboard.node)
                .filter(|node| self.book.xml.tag(*node) == "zone")
            {
                let kind = self.book.xml.attr_raw(zone, "type-v2");
                if !matches!(kind, None | Some("worksheet")) {
                    continue;
                }
                if let Some(name) = self
                    .book
                    .xml
                    .attr(zone, "name")?
                    .map(|value| value.into_owned())
                {
                    if let Some(worksheet) = self.book.worksheet_id(&name) {
                        worksheets.insert(worksheet);
                    } else {
                        self.gap(
                            "dashboard_sheet_unresolved",
                            dashboard.name.clone(),
                            format!("Zone references unknown worksheet {name:?}"),
                        );
                    }
                }
            }
            for worksheet in worksheets {
                self.edge(
                    self.worksheets[worksheet.0 as usize],
                    dashboard_node,
                    EdgeKind::SheetInDashboard,
                );
            }
        }
        Ok(())
    }

    fn sql_table(&mut self, datasource: DatasourceId, name: &str) -> u32 {
        let key = (datasource, name.to_owned());
        if let Some(&node) = self.sql_tables.get(&key) {
            return node;
        }
        let node = self.add(Kind::SqlTable, name);
        self.sql_tables.insert(key, node);
        node
    }

    fn temp_table(&mut self, connection: ConnectionId, name: &str) -> u32 {
        let canonical = canonical_temp_name(name).unwrap_or_else(|| name.to_ascii_lowercase());
        let key = (connection, canonical.clone());
        if let Some(&node) = self.temp_tables.get(&key) {
            return node;
        }
        let node = self.add(Kind::SqlTable, unquote(name));
        self.temp_tables.insert(key, node);
        node
    }

    fn relation_connection_id(
        &self,
        datasource: DatasourceId,
        connection_name: Option<&str>,
    ) -> Option<ConnectionId> {
        let name = connection_name?;
        self.connection_by_name
            .get(&(datasource, name.to_owned()))
            .copied()
    }

    fn sql_read_node(
        &mut self,
        datasource: DatasourceId,
        connection: Option<ConnectionId>,
        name: &str,
        context: &str,
    ) -> u32 {
        if is_global_temp_name(name) {
            self.gap(
                "global_temp_table_unsupported",
                context,
                format!(
                    "SQL Server global temporary table {name:?} is visible beyond one connection; TabKit does not guess its producer scope"
                ),
            );
            return self.sql_table(datasource, name);
        }

        if let Some(canonical) = canonical_temp_name(name) {
            if let Some(connection) = connection {
                let node = self.temp_table(connection, name);
                if !self.temp_producers.contains(&(connection, canonical)) {
                    self.gap(
                        "temp_table_producer_unresolved",
                        context,
                        format!(
                            "Temporary table {name:?} is read but no producer was resolved in Initial SQL for the same connection"
                        ),
                    );
                }
                return node;
            }

            self.gap(
                "temp_table_connection_unresolved",
                context,
                format!(
                    "Temporary table {name:?} is read but the owning connection cannot be resolved"
                ),
            );
        }

        self.sql_table(datasource, name)
    }

    fn add(&mut self, kind: Kind, name: &str) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            id,
            kind,
            name: name.to_owned(),
        });
        id
    }

    fn edge(&mut self, from: u32, to: u32, kind: EdgeKind) {
        self.edges.push(Edge { from, to, kind });
    }

    fn gap(&mut self, code: &'static str, context: impl Into<String>, message: impl Into<String>) {
        self.gaps.push(Gap {
            code,
            context: context.into(),
            message: message.into(),
        });
    }

    fn finish(mut self) -> Lineage {
        self.edges
            .sort_unstable_by_key(|edge| (edge.from, edge.to, edge.kind));
        self.edges.dedup();

        let outgoing = adjacency(self.nodes.len(), &self.edges, |edge| edge.from);
        let incoming = adjacency(self.nodes.len(), &self.edges, |edge| edge.to);
        let mut exact_names = FastHashMap::<u64, ExactNode>::default();
        exact_names.reserve(self.nodes.len());
        for node in &self.nodes {
            exact_names
                .entry(ascii_case_hash(&node.name))
                .and_modify(|entry| *entry = ExactNode::Ambiguous)
                .or_insert(ExactNode::One(node.id));
        }

        self.gaps.sort_unstable_by(|a, b| {
            (a.code, &a.context, &a.message).cmp(&(b.code, &b.context, &b.message))
        });
        self.gaps
            .dedup_by(|a, b| a.code == b.code && a.context == b.context && a.message == b.message);

        Lineage {
            nodes: self.nodes,
            edges: self.edges,
            gaps: self.gaps,
            exact_names,
            outgoing,
            incoming,
        }
    }
}

fn adjacency(node_count: usize, edges: &[Edge], node_of: impl Fn(&Edge) -> u32) -> Adjacency {
    let mut offsets = vec![0u32; node_count + 1];
    for edge in edges {
        offsets[node_of(edge) as usize + 1] += 1;
    }
    for index in 1..offsets.len() {
        offsets[index] += offsets[index - 1];
    }

    let mut edge_ids = vec![0u32; edges.len()];
    let mut cursor = offsets[..node_count].to_vec();
    for (edge_id, edge) in edges.iter().enumerate() {
        let node = node_of(edge) as usize;
        let slot = cursor[node] as usize;
        edge_ids[slot] = edge_id as u32;
        cursor[node] += 1;
    }

    Adjacency { offsets, edge_ids }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(value)
}

struct Reads {
    scopes: Vec<BTreeSet<String>>,
    names: BTreeSet<String>,
    unsupported: bool,
}

impl Visitor for Reads {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        let aliases = query
            .with
            .as_ref()
            .map(|with| {
                with.cte_tables
                    .iter()
                    .map(|cte| cte.alias.name.value.to_lowercase())
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

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<()> {
        match factor {
            TableFactor::Table { name, args, .. } => {
                if args.is_some() {
                    self.unsupported = true;
                } else {
                    let rendered = name.to_string();
                    let cte = name.0.len() == 1
                        && self.scopes.iter().any(|scope| {
                            name.0[0]
                                .as_ident()
                                .is_some_and(|ident| scope.contains(&ident.value.to_lowercase()))
                        });
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

#[derive(Clone, Copy)]
enum SqlFlavor {
    Generic,
    MsSql,
}

struct TempWrite {
    target: String,
    sources: BTreeSet<String>,
}

struct InitialSqlAnalysis {
    direct_reads: BTreeSet<String>,
    temp_writes: Vec<TempWrite>,
    partial: bool,
}

fn sql_flavor(connection_class: Option<&str>, sql: &str) -> SqlFlavor {
    let mssql_class = connection_class.is_some_and(|class| {
        matches!(
            class.to_ascii_lowercase().as_str(),
            "sqlserver" | "mssql" | "sql-server" | "sql server"
        )
    });
    if mssql_class || sql.contains('#') {
        SqlFlavor::MsSql
    } else {
        SqlFlavor::Generic
    }
}

fn parse_statements(sql: &str, flavor: SqlFlavor) -> std::result::Result<Vec<Statement>, ()> {
    match flavor {
        SqlFlavor::Generic => Parser::parse_sql(&GenericDialect {}, sql).map_err(|_| ()),
        SqlFlavor::MsSql => {
            if let Ok(statements) = Parser::parse_sql(&MsSqlDialect {}, sql) {
                return Ok(statements);
            }

            let mut statements = Vec::new();
            for chunk in mssql_statement_chunks(sql) {
                let statement = chunk.trim();
                if statement.is_empty() || is_mssql_lineage_neutral_setup(statement) {
                    continue;
                }
                statements.extend(Parser::parse_sql(&MsSqlDialect {}, statement).map_err(|_| ())?);
            }
            Ok(statements)
        }
    }
}

fn is_mssql_lineage_neutral_setup(statement: &str) -> bool {
    let normalized = statement.split_whitespace().collect::<Vec<_>>();
    if normalized.len() == 3
        && normalized[0].eq_ignore_ascii_case("set")
        && matches!(normalized[2].to_ascii_lowercase().as_str(), "on" | "off")
        && normalized[1]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return true;
    }

    let lower = statement.to_ascii_lowercase();
    lower.starts_with("if object_id(") && lower.contains('#') && lower.contains("drop table")
}

fn mssql_statement_chunks(sql: &str) -> Vec<&str> {
    let bytes = sql.as_bytes();
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut single_quote = false;
    let mut double_quote = false;
    let mut bracket = false;
    let mut line_comment = false;
    let mut block_comment = false;

    while index < bytes.len() {
        let current = bytes[index];
        let next = bytes.get(index + 1).copied();

        if line_comment {
            if current == b'\n' {
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if block_comment {
            if current == b'*' && next == Some(b'/') {
                block_comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if single_quote {
            if current == b'\'' {
                if next == Some(b'\'') {
                    index += 2;
                    continue;
                }
                single_quote = false;
            }
            index += 1;
            continue;
        }
        if double_quote {
            if current == b'"' {
                if next == Some(b'"') {
                    index += 2;
                    continue;
                }
                double_quote = false;
            }
            index += 1;
            continue;
        }
        if bracket {
            if current == b']' {
                if next == Some(b']') {
                    index += 2;
                    continue;
                }
                bracket = false;
            }
            index += 1;
            continue;
        }

        match (current, next) {
            (b'-', Some(b'-')) => {
                line_comment = true;
                index += 2;
            }
            (b'/', Some(b'*')) => {
                block_comment = true;
                index += 2;
            }
            (b'\'', _) => {
                single_quote = true;
                index += 1;
            }
            (b'"', _) => {
                double_quote = true;
                index += 1;
            }
            (b'[', _) => {
                bracket = true;
                index += 1;
            }
            (b';', _) => {
                chunks.push(&sql[start..index]);
                start = index + 1;
                index += 1;
            }
            _ => index += 1,
        }
    }

    if start <= sql.len() {
        chunks.push(&sql[start..]);
    }
    chunks
}

fn query_reads(query: &Query) -> std::result::Result<BTreeSet<String>, ()> {
    let mut reads = Reads {
        scopes: Vec::new(),
        names: BTreeSet::new(),
        unsupported: false,
    };
    let _ = query.visit(&mut reads);
    if reads.unsupported {
        Err(())
    } else {
        Ok(reads.names)
    }
}

fn read_tables_with_flavor(
    sql: &str,
    flavor: SqlFlavor,
) -> std::result::Result<BTreeSet<String>, ()> {
    let statements = parse_statements(sql, flavor)?;
    if statements.is_empty()
        || statements
            .iter()
            .any(|statement| !matches!(statement, Statement::Query(_)))
    {
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
fn read_tables(sql: &str) -> std::result::Result<BTreeSet<String>, ()> {
    read_tables_with_flavor(sql, SqlFlavor::Generic)
}

fn canonical_any_temp_name(value: &str) -> Option<String> {
    let value = unquote(value.trim());
    value.starts_with('#').then(|| value.to_ascii_lowercase())
}

fn is_global_temp_name(value: &str) -> bool {
    unquote(value.trim()).starts_with("##")
}

fn canonical_temp_name(value: &str) -> Option<String> {
    let value = unquote(value.trim());
    (value.starts_with('#') && !value.starts_with("##")).then(|| value.to_ascii_lowercase())
}

fn select_into_temp(query: &Query) -> Option<String> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    select
        .into
        .as_ref()
        .and_then(|into| match into.targets.as_slice() {
            [target] => canonical_any_temp_name(&target.to_string()),
            _ => None,
        })
}

fn analyze_initial_sql(
    sql: &str,
    connection_class: Option<&str>,
) -> std::result::Result<InitialSqlAnalysis, ()> {
    let flavor = sql_flavor(connection_class, sql);
    let statements = parse_statements(sql, flavor)?;
    if statements.is_empty() {
        return Err(());
    }

    let mut direct_reads = BTreeSet::new();
    let mut temp_writes = Vec::new();
    let mut partial = false;

    for statement in statements {
        match statement {
            Statement::Query(query) => {
                let reads = query_reads(&query)?;
                if let Some(target) = select_into_temp(&query) {
                    temp_writes.push(TempWrite {
                        target,
                        sources: reads,
                    });
                } else {
                    direct_reads.extend(reads);
                }
            }
            Statement::Insert(insert) => {
                let TableObject::TableName(table_name) = &insert.table else {
                    partial = true;
                    continue;
                };
                let Some(target) = canonical_any_temp_name(&table_name.to_string()) else {
                    partial = true;
                    continue;
                };
                let Some(source) = insert.source else {
                    partial = true;
                    continue;
                };
                temp_writes.push(TempWrite {
                    target,
                    sources: query_reads(&source)?,
                });
            }
            Statement::CreateTable(create) => {
                let Some(target) = canonical_any_temp_name(&create.name.to_string()) else {
                    partial = true;
                    continue;
                };
                if let Some(query) = create.query {
                    temp_writes.push(TempWrite {
                        target,
                        sources: query_reads(&query)?,
                    });
                }
            }
            Statement::Drop { names, .. }
                if names
                    .iter()
                    .all(|name| canonical_any_temp_name(&name.to_string()).is_some()) => {}
            Statement::Declare { .. } | Statement::Set(_) if matches!(flavor, SqlFlavor::MsSql) => {
            }
            _ => partial = true,
        }
    }

    if partial && direct_reads.is_empty() && temp_writes.is_empty() {
        return Err(());
    }

    Ok(InitialSqlAnalysis {
        direct_reads,
        temp_writes,
        partial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_names(sql: &str) -> Option<Vec<String>> {
        read_tables(sql)
            .ok()
            .map(|values| values.into_iter().collect::<Vec<_>>())
    }

    #[test]
    fn sql_reads_ignore_cte_aliases() {
        assert_eq!(
            read_names(
                "WITH c AS (SELECT id FROM public.orders) SELECT id FROM c JOIN audit.events e ON c.id=e.id",
            ),
            Some(vec!["audit.events".to_owned(), "public.orders".to_owned()])
        );
    }

    #[test]
    fn sql_reads_nested_ctes_and_derived_queries_without_alias_leakage() {
        assert_eq!(
            read_names(
                "WITH base AS (
                    SELECT id FROM warehouse.orders
                 ), enriched AS (
                    SELECT b.id FROM base b JOIN audit.events e ON b.id = e.id
                 )
                 SELECT * FROM enriched
                 JOIN (SELECT id FROM crm.customers) c ON enriched.id = c.id",
            ),
            Some(vec![
                "audit.events".to_owned(),
                "crm.customers".to_owned(),
                "warehouse.orders".to_owned(),
            ])
        );
    }

    #[test]
    fn sql_reads_preserve_qualified_and_quoted_source_names() {
        assert_eq!(
            read_names(r#"SELECT * FROM "Sales"."Order Facts" o JOIN raw.events e ON o.id=e.id"#),
            Some(vec![
                r#""Sales"."Order Facts""#.to_owned(),
                "raw.events".to_owned(),
            ])
        );
    }

    #[test]
    fn mssql_initial_sql_select_into_temp_tracks_sources_and_target() -> Result<()> {
        let analysis = analyze_initial_sql(
            "SELECT o.id, o.amount INTO #stage FROM dbo.orders o;",
            Some("sqlserver"),
        )
        .map_err(|_| Error::new("TEST", "MSSQL Initial SQL did not parse"))?;
        assert!(!analysis.partial);
        assert!(analysis.direct_reads.is_empty());
        assert_eq!(analysis.temp_writes.len(), 1);
        assert_eq!(analysis.temp_writes[0].target, "#stage");
        assert_eq!(
            analysis.temp_writes[0]
                .sources
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["dbo.orders".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn mssql_initial_sql_create_insert_temp_tracks_insert_source() -> Result<()> {
        let analysis = analyze_initial_sql(
            "CREATE TABLE #stage (id INT, amount DECIMAL(18,2));              INSERT INTO #stage (id, amount) SELECT id, amount FROM dbo.orders;",
            Some("sqlserver"),
        )
        .map_err(|_| Error::new("TEST", "MSSQL CREATE/INSERT temp SQL did not parse"))?;
        assert!(!analysis.partial);
        assert_eq!(analysis.temp_writes.len(), 1);
        assert_eq!(analysis.temp_writes[0].target, "#stage");
        assert!(analysis.temp_writes[0].sources.contains("dbo.orders"));
        Ok(())
    }

    #[test]
    fn mssql_initial_sql_accepts_common_temp_table_setup() -> Result<()> {
        let analysis = analyze_initial_sql(
            "SET NOCOUNT ON; DROP TABLE IF EXISTS #stage;              SELECT id, amount INTO #stage FROM dbo.orders;",
            Some("sqlserver"),
        )
        .map_err(|_| Error::new("TEST", "MSSQL temp-table setup did not parse"))?;
        assert!(!analysis.partial);
        assert_eq!(analysis.temp_writes.len(), 1);
        assert_eq!(analysis.temp_writes[0].target, "#stage");
        assert!(analysis.temp_writes[0].sources.contains("dbo.orders"));
        Ok(())
    }

    #[test]
    fn mssql_initial_sql_marks_unmodeled_temp_mutation_partial() -> Result<()> {
        let analysis = analyze_initial_sql(
            "SELECT id, amount INTO #stage FROM dbo.orders;              UPDATE #stage SET amount = amount * 2;",
            Some("sqlserver"),
        )
        .map_err(|_| Error::new("TEST", "MSSQL temp mutation batch did not parse"))?;
        assert!(analysis.partial);
        assert_eq!(analysis.temp_writes.len(), 1);
        assert_eq!(analysis.temp_writes[0].target, "#stage");
        assert!(analysis.temp_writes[0].sources.contains("dbo.orders"));
        Ok(())
    }

    #[test]
    fn mssql_global_temp_is_not_classified_as_connection_local() -> Result<()> {
        let analysis =
            analyze_initial_sql("SELECT id INTO ##stage FROM dbo.orders;", Some("sqlserver"))
                .map_err(|_| Error::new("TEST", "MSSQL global temp SQL did not parse"))?;
        assert_eq!(analysis.temp_writes.len(), 1);
        assert_eq!(analysis.temp_writes[0].target, "##stage");
        assert!(is_global_temp_name(&analysis.temp_writes[0].target));
        assert!(canonical_temp_name(&analysis.temp_writes[0].target).is_none());
        Ok(())
    }

    #[test]
    fn mssql_initial_sql_chains_temp_tables() -> Result<()> {
        let analysis = analyze_initial_sql(
            "SELECT * INTO #a FROM dbo.orders;              SELECT * INTO #b FROM #a;",
            Some("sqlserver"),
        )
        .map_err(|_| Error::new("TEST", "MSSQL chained temp SQL did not parse"))?;
        assert_eq!(analysis.temp_writes.len(), 2);
        assert_eq!(analysis.temp_writes[0].target, "#a");
        assert!(analysis.temp_writes[0].sources.contains("dbo.orders"));
        assert_eq!(analysis.temp_writes[1].target, "#b");
        assert!(analysis.temp_writes[1].sources.contains("#a"));
        Ok(())
    }

    #[test]
    fn mssql_custom_sql_reads_hash_temp_table() -> Result<()> {
        let reads = read_tables_with_flavor("SELECT * FROM #stage", SqlFlavor::MsSql)
            .map_err(|_| Error::new("TEST", "MSSQL Custom SQL did not parse"))?;
        assert_eq!(reads.into_iter().collect::<Vec<_>>(), vec!["#stage"]);
        Ok(())
    }

    #[test]
    fn sql_reads_reject_non_query_or_unsupported_table_factors() {
        assert!(read_tables("SET ROLE analyst").is_err());
        assert!(read_tables("SELECT * FROM generate_series(1, 3)").is_err());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    fn fixture() -> Result<Workbook> {
        Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <connection class="postgres" name="postgres.1" one-time-sql="SELECT * FROM bootstrap.tenant">
        <relation connection="postgres.1" name="Orders SQL" type="text">SELECT Sales, Profit FROM warehouse.orders</relation>
        <metadata-records>
          <metadata-record class="column">
            <local-name>[Sales]</local-name>
            <parent-name>[Orders SQL]</parent-name>
          </metadata-record>
          <metadata-record class="column">
            <local-name>[Profit]</local-name>
            <parent-name>[Orders SQL]</parent-name>
          </metadata-record>
        </metadata-records>
      </connection>
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      <column caption="Profit" datatype="real" name="[Profit]" role="measure" type="quantitative"/>
      <column caption="Profit Ratio" datatype="real" name="[Profit Ratio]" role="measure" type="quantitative">
        <calculation class="tableau" formula="[Profit] / [Sales]"/>
      </column>
    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Profit KPI">
      <table><view>
        <datasource-dependencies datasource="Orders">
          <column datatype="real" name="[Profit Ratio]" role="measure" type="quantitative"/>
        </datasource-dependencies>
      </view></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000001}"/>
    </worksheet>
  </worksheets>
  <dashboards>
    <dashboard name="Executive">
      <style/>
      <size maxheight="900" maxwidth="1440" minheight="900" minwidth="1440" sizing-mode="fixed"/>
      <zones>
        <zone h="100000" id="1" type-v2="layout-basic" w="100000" x="0" y="0">
          <zone h="100000" id="2" name="Profit KPI" w="100000" x="0" y="0"/>
        </zone>
      </zones>
      <simple-id uuid="{00000000-0000-0000-0000-000000000002}"/>
    </dashboard>
  </dashboards>
</workbook>"#
                .to_vec(),
        )
    }

    #[test]
    fn exact_lookup_and_bounded_trace_preserve_semantics() -> Result<()> {
        let workbook = fixture()?;
        let lineage = Lineage::build(&workbook)?;

        let matches = lineage.find_kind("sAlEs", Some(Kind::Field));
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Sales");

        let trace = lineage
            .trace_limited(matches[0].id, Direction::Downstream, 2)
            .ok_or_else(|| crate::Error::new("TEST", "bounded trace missing"))?;
        assert_eq!(trace.nodes.len(), 2);
        assert!(trace.truncated);

        let node_ids = trace
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<std::collections::HashSet<_>>();
        assert!(node_ids.contains(&matches[0].id));
        assert!(
            trace
                .edges
                .iter()
                .all(|edge| node_ids.contains(&edge.from) && node_ids.contains(&edge.to))
        );
        assert!(trace.edges.windows(2).all(|pair| {
            (pair[0].from, pair[0].to, pair[0].kind) <= (pair[1].from, pair[1].to, pair[1].kind)
        }));
        Ok(())
    }

    #[test]
    fn precise_sql_origins_do_not_cross_contaminate_fields_in_same_datasource() -> Result<()> {
        let workbook = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Mixed" version="26.2">
      <connection class="postgres" name="postgres.1">
        <relation connection="postgres.1" name="SQL A" type="text">SELECT a_value FROM raw.a</relation>
        <relation connection="postgres.1" name="SQL B" type="text">SELECT b_value FROM raw.b</relation>
        <metadata-records>
          <metadata-record class="column">
            <local-name>[A Value]</local-name>
            <parent-name>[SQL A]</parent-name>
          </metadata-record>
          <metadata-record class="column">
            <local-name>[B Value]</local-name>
            <parent-name>[SQL B]</parent-name>
          </metadata-record>
        </metadata-records>
      </connection>
      <column caption="A Value" datatype="real" name="[A Value]" role="measure" type="quantitative"/>
      <column caption="B Value" datatype="real" name="[B Value]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;
        let lineage = Lineage::build(&workbook)?;

        let sql_a = lineage
            .find_kind("raw.a", Some(Kind::SqlTable))
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error::new("TEST", "raw.a source missing"))?;
        let downstream = lineage
            .trace(sql_a.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "raw.a trace missing"))?;
        assert!(
            downstream
                .nodes
                .iter()
                .any(|node| node.kind == Kind::Field && node.name == "A Value")
        );
        assert!(
            !downstream
                .nodes
                .iter()
                .any(|node| node.kind == Kind::Field && node.name == "B Value"),
            "precise SQL A origin must not leak through datasource containment into SQL B fields"
        );

        let field_b = lineage
            .find_kind("B Value", Some(Kind::Field))
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error::new("TEST", "B Value field missing"))?;
        let upstream = lineage
            .trace(field_b.id, Direction::Upstream)
            .ok_or_else(|| crate::Error::new("TEST", "B Value trace missing"))?;
        assert!(
            !upstream
                .nodes
                .iter()
                .any(|node| node.kind == Kind::SqlTable && node.name == "raw.a"),
            "precise B field origin must not fan out to unrelated SQL A source"
        );
        Ok(())
    }

    #[test]
    fn reports_unresolved_custom_sql_instead_of_guessing_sources() -> Result<()> {
        let workbook = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <connection class="postgres" name="postgres.1">
        <relation connection="postgres.1" name="Generated SQL" type="text">SELECT * FROM generate_series(1, 3)</relation>
      </connection>
      <column caption="Value" datatype="integer" name="[Value]" role="dimension" type="quantitative"/>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;
        let lineage = Lineage::build(&workbook)?;
        assert!(
            lineage.gaps.iter().any(|gap| {
                gap.code == "sql_lineage_unresolved" && gap.context == "Generated SQL"
            })
        );
        assert!(
            lineage
                .nodes
                .iter()
                .any(|node| node.kind == Kind::CustomSql && node.name == "Generated SQL")
        );
        Ok(())
    }

    #[test]
    fn mssql_initial_sql_temp_table_flows_into_custom_sql() -> Result<()> {
        let workbook = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <connection class="sqlserver" name="sqlserver.1" one-time-sql="SELECT id, amount * 1.2 AS amount INTO #stage FROM dbo.orders">
        <relation connection="sqlserver.1" name="Stage SQL" type="text">SELECT * FROM #stage</relation>
        <metadata-records>
          <metadata-record class="column">
            <local-name>[id]</local-name>
            <parent-name>[Stage SQL]</parent-name>
          </metadata-record>
          <metadata-record class="column">
            <local-name>[amount]</local-name>
            <parent-name>[Stage SQL]</parent-name>
          </metadata-record>
        </metadata-records>
      </connection>
      <column caption="id" datatype="integer" name="[id]" role="dimension" type="quantitative"/>
      <column caption="amount" datatype="real" name="[amount]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;
        let lineage = Lineage::build(&workbook)?;

        assert!(!lineage.gaps.iter().any(|gap| {
            matches!(
                gap.code,
                "initial_sql_lineage_unresolved"
                    | "temp_table_producer_unresolved"
                    | "temp_table_connection_unresolved"
            )
        }));

        let source = lineage
            .find_kind("dbo.orders", Some(Kind::SqlTable))
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error::new("TEST", "dbo.orders source missing"))?;
        let downstream = lineage
            .trace(source.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "dbo.orders trace missing"))?;
        for (kind, name) in [
            (Kind::SqlTable, "#stage"),
            (Kind::CustomSql, "Stage SQL"),
            (Kind::Field, "amount"),
        ] {
            assert!(
                downstream
                    .nodes
                    .iter()
                    .any(|node| node.kind == kind && node.name.eq_ignore_ascii_case(name)),
                "MSSQL temp-table trace is missing {kind:?} {name:?}"
            );
        }

        let temp = lineage
            .find_kind("#stage", Some(Kind::SqlTable))
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error::new("TEST", "#stage temp table missing"))?;
        let upstream = lineage
            .trace(temp.id, Direction::Upstream)
            .ok_or_else(|| crate::Error::new("TEST", "#stage upstream trace missing"))?;
        assert!(
            upstream
                .nodes
                .iter()
                .any(|node| node.kind == Kind::InitialSql)
        );
        assert!(
            upstream
                .nodes
                .iter()
                .any(|node| node.kind == Kind::SqlTable && node.name == "dbo.orders")
        );
        Ok(())
    }

    #[test]
    fn mssql_hash_temp_tables_are_scoped_per_connection() -> Result<()> {
        let workbook = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Mixed" version="26.2">
      <connection class="sqlserver" name="sqlserver.1" one-time-sql="SELECT id INTO #stage FROM dbo.orders_a">
        <relation connection="sqlserver.1" name="SQL A" type="text">SELECT * FROM #stage</relation>
      </connection>
      <connection class="sqlserver" name="sqlserver.2" one-time-sql="SELECT id INTO #stage FROM dbo.orders_b">
        <relation connection="sqlserver.2" name="SQL B" type="text">SELECT * FROM #stage</relation>
      </connection>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;
        let lineage = Lineage::build(&workbook)?;

        assert_eq!(
            lineage
                .nodes
                .iter()
                .filter(|node| node.kind == Kind::SqlTable && node.name == "#stage")
                .count(),
            2
        );

        let source_a = lineage
            .find_kind("dbo.orders_a", Some(Kind::SqlTable))
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error::new("TEST", "dbo.orders_a source missing"))?;
        let downstream_a = lineage
            .trace(source_a.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "orders_a trace missing"))?;
        assert!(
            downstream_a
                .nodes
                .iter()
                .any(|node| node.kind == Kind::CustomSql && node.name == "SQL A")
        );
        assert!(
            !downstream_a
                .nodes
                .iter()
                .any(|node| node.kind == Kind::CustomSql && node.name == "SQL B"),
            "connection-scoped #stage from sqlserver.1 leaked into sqlserver.2"
        );
        Ok(())
    }

    #[test]
    fn mssql_partial_temp_mutation_emits_explicit_gap() -> Result<()> {
        let workbook = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <connection class="sqlserver" name="sqlserver.1" one-time-sql="SELECT id, amount INTO #stage FROM dbo.orders; UPDATE #stage SET amount = amount * 2">
        <relation connection="sqlserver.1" name="Stage SQL" type="text">SELECT * FROM #stage</relation>
      </connection>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;
        let lineage = Lineage::build(&workbook)?;
        assert!(lineage.gaps.iter().any(|gap| {
            gap.code == "initial_sql_lineage_partial" && gap.context == "sqlserver.1"
        }));
        assert!(!lineage.gaps.iter().any(|gap| {
            gap.code == "temp_table_producer_unresolved" && gap.context == "Stage SQL"
        }));
        Ok(())
    }

    #[test]
    fn mssql_global_temp_emits_scope_gap_instead_of_local_link() -> Result<()> {
        let workbook = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <connection class="sqlserver" name="sqlserver.1" one-time-sql="SELECT id INTO ##stage FROM dbo.orders">
        <relation connection="sqlserver.1" name="Global Stage SQL" type="text">SELECT * FROM ##stage</relation>
      </connection>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;
        let lineage = Lineage::build(&workbook)?;
        assert!(
            lineage
                .gaps
                .iter()
                .any(|gap| { gap.code == "global_temp_table_unsupported" })
        );
        assert_eq!(
            lineage
                .nodes
                .iter()
                .filter(|node| node.kind == Kind::SqlTable && node.name == "##stage")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn reports_unresolved_initial_sql_instead_of_dropping_it() -> Result<()> {
        let workbook = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <connection class="postgres" name="postgres.1" one-time-sql="SET ROLE analyst"/>
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;
        let lineage = Lineage::build(&workbook)?;
        assert!(lineage.gaps.iter().any(|gap| {
            gap.code == "initial_sql_lineage_unresolved" && gap.context == "postgres.1"
        }));
        assert!(
            lineage
                .nodes
                .iter()
                .any(|node| node.kind == Kind::InitialSql)
        );
        Ok(())
    }

    #[test]
    fn traces_source_field_calculation_sheet_dashboard() -> Result<()> {
        let workbook = fixture()?;
        let lineage = Lineage::build(&workbook)?;

        let sales = lineage
            .nodes
            .iter()
            .find(|node| node.kind == Kind::Field && node.name == "Sales")
            .ok_or_else(|| crate::Error::new("TEST", "Sales field missing"))?;
        let downstream = lineage
            .trace(sales.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "Sales trace missing"))?;

        assert!(
            downstream
                .nodes
                .iter()
                .any(|node| { node.kind == Kind::Calculation && node.name == "Profit Ratio" })
        );
        assert!(
            downstream
                .nodes
                .iter()
                .any(|node| node.kind == Kind::Worksheet && node.name == "Profit KPI")
        );
        assert!(
            downstream
                .nodes
                .iter()
                .any(|node| node.kind == Kind::Dashboard && node.name == "Executive")
        );

        let custom_sql = lineage
            .nodes
            .iter()
            .find(|node| node.kind == Kind::CustomSql && node.name == "Orders SQL")
            .ok_or_else(|| crate::Error::new("TEST", "Custom SQL missing"))?;
        let source_trace = lineage
            .trace(custom_sql.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "Custom SQL trace missing"))?;
        assert!(
            source_trace
                .nodes
                .iter()
                .any(|node| node.kind == Kind::Field && node.name == "Sales")
        );

        let sql_table = lineage
            .find_kind("warehouse.orders", Some(Kind::SqlTable))
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error::new("TEST", "SQL source table missing"))?;
        let sql_trace = lineage
            .trace(sql_table.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "SQL source trace missing"))?;
        for (kind, name) in [
            (Kind::CustomSql, "Orders SQL"),
            (Kind::Field, "Sales"),
            (Kind::Calculation, "Profit Ratio"),
            (Kind::Worksheet, "Profit KPI"),
            (Kind::Dashboard, "Executive"),
        ] {
            assert!(
                sql_trace
                    .nodes
                    .iter()
                    .any(|node| node.kind == kind && node.name == name),
                "SQL source trace is missing {kind:?} {name:?}"
            );
        }

        let initial_source = lineage
            .find_kind("bootstrap.tenant", Some(Kind::SqlTable))
            .into_iter()
            .next()
            .ok_or_else(|| crate::Error::new("TEST", "Initial SQL source table missing"))?;
        let initial_source_trace = lineage
            .trace(initial_source.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "Initial SQL source trace missing"))?;
        assert!(
            initial_source_trace
                .nodes
                .iter()
                .any(|node| node.kind == Kind::InitialSql)
        );

        let initial_sql = lineage
            .nodes
            .iter()
            .find(|node| node.kind == Kind::InitialSql)
            .ok_or_else(|| crate::Error::new("TEST", "Initial SQL missing"))?;
        let initial_trace = lineage
            .trace(initial_sql.id, Direction::Downstream)
            .ok_or_else(|| crate::Error::new("TEST", "Initial SQL trace missing"))?;
        assert!(
            initial_trace
                .nodes
                .iter()
                .any(|node| node.kind == Kind::Calculation && node.name == "Profit Ratio")
        );
        Ok(())
    }
}
