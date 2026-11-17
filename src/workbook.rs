//! Typed semantic index over a source-backed Tableau TWB document.
//!
//! The index is selective: it types only entities needed by inspection, lineage,
//! and supported edits while the complete [`Document`] remains authoritative for
//! structure and bytes that are not represented in the typed projection.
//!
//! All dense IDs in this module are local to one `Workbook` instance. Reparsing
//! a candidate rebuilds the index, so IDs must never be persisted or carried
//! across workbook snapshots.

use crate::{
    Error, Result, TABLEAU_PRODUCT_TARGET, TWB_VERSION, require,
    text::{FastHashMap, ascii_case_hash, contains_case_insensitive},
    xml::{Document, NodeId},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local datasource ID; invalid across reparses or other workbooks.
pub struct DatasourceId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local connection ID; invalid across reparses or other workbooks.
pub struct ConnectionId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local relation ID; invalid across reparses or other workbooks.
pub struct RelationId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local field ID; invalid across reparses or other workbooks.
pub struct FieldId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local calculation ID; invalid across reparses or other workbooks.
pub struct CalculationId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local parameter ID; invalid across reparses or other workbooks.
pub struct ParameterId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local worksheet ID; invalid across reparses or other workbooks.
pub struct WorksheetId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
/// Dense workbook-local dashboard ID; invalid across reparses or other workbooks.
pub struct DashboardId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
/// Tableau field datatype normalized for supported edits and inspection.
pub enum DataType {
    Unknown,
    Integer,
    Real,
    String,
    Datetime,
    Date,
    Boolean,
    Tuple,
    Spatial,
    Table,
}

impl DataType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Integer => "integer",
            Self::Real => "real",
            Self::String => "string",
            Self::Datetime => "datetime",
            Self::Date => "date",
            Self::Boolean => "boolean",
            Self::Tuple => "tuple",
            Self::Spatial => "spatial",
            Self::Table => "table",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "integer" => Self::Integer,
            "real" => Self::Real,
            "string" => Self::String,
            "datetime" => Self::Datetime,
            "date" => Self::Date,
            "boolean" => Self::Boolean,
            "tuple" => Self::Tuple,
            "spatial" => Self::Spatial,
            "table" => Self::Table,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
/// Tableau field role.
pub enum Role {
    Dimension,
    Measure,
    Unknown,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dimension => "dimension",
            Self::Measure => "measure",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "dimension" => Self::Dimension,
            "measure" => Self::Measure,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
/// Tableau field semantic type.
pub enum FieldType {
    Nominal,
    Ordinal,
    Quantitative,
    Unknown,
}

impl FieldType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::Ordinal => "ordinal",
            Self::Quantitative => "quantitative",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "nominal" => Self::Nominal,
            "ordinal" => Self::Ordinal,
            "quantitative" => Self::Quantitative,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Distinguishes source connections from extract-internal connections.
pub enum ConnectionScope {
    Source,
    Extract,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Relation category represented in the typed workbook index.
pub enum RelationKind {
    PhysicalTable,
    CustomSql,
    Join,
    LogicalTable,
    Relationship,
}

#[derive(Debug, Serialize)]
pub struct Datasource {
    pub id: DatasourceId,
    pub name: String,
    pub caption: Option<String>,
    pub inline: bool,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Debug, Serialize)]
pub struct Connection {
    pub id: ConnectionId,
    pub datasource: DatasourceId,
    pub name: String,
    pub class: Option<String>,
    pub server: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub initial_sql: Option<String>,
    pub scope: ConnectionScope,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Debug, Serialize)]
pub struct Relation {
    pub id: RelationId,
    pub datasource: DatasourceId,
    pub kind: RelationKind,
    pub name: String,
    pub connection: Option<String>,
    pub table: Option<String>,
    pub sql: Option<String>,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Debug, Serialize)]
pub struct Field {
    pub id: FieldId,
    pub datasource: DatasourceId,
    pub name: String,
    pub caption: Option<String>,
    pub datatype: DataType,
    pub role: Role,
    pub field_type: FieldType,
    pub calculation: Option<CalculationId>,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Debug, Serialize)]
pub struct Calculation {
    pub id: CalculationId,
    pub field: FieldId,
    pub formula: String,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterDomainKind {
    Any,
    List,
    Range,
    Unknown,
}

impl ParameterDomainKind {
    fn parse(value: &str) -> Self {
        match value {
            "any" => Self::Any,
            "list" => Self::List,
            "range" => Self::Range,
            _ => Self::Unknown,
        }
    }
}

/// Tableau parameter backed by a field in the synthetic `Parameters` datasource.
#[derive(Debug, Serialize)]
pub struct Parameter {
    pub id: ParameterId,
    pub field: FieldId,
    pub name: String,
    pub caption: Option<String>,
    pub datatype: DataType,
    pub domain: ParameterDomainKind,
    pub value: Option<String>,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Debug, Serialize)]
pub struct Worksheet {
    pub id: WorksheetId,
    pub name: String,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Debug, Serialize)]
pub struct Dashboard {
    pub id: DashboardId,
    pub name: String,
    #[serde(skip)]
    pub node: NodeId,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub version: String,
    pub xml_nodes: usize,
    pub datasources: usize,
    pub connections: usize,
    pub relations: usize,
    pub fields: usize,
    pub calculations: usize,
    pub parameters: usize,
    pub worksheets: usize,
    pub dashboards: usize,
}

#[derive(Clone, Copy)]
enum ExactField {
    One(FieldId),
    Ambiguous,
}

/// One immutable TWB snapshot: authoritative source XML plus derived semantic indexes.
pub struct Workbook {
    pub xml: Document,
    pub datasources: Vec<Datasource>,
    pub connections: Vec<Connection>,
    pub relations: Vec<Relation>,
    pub fields: Vec<Field>,
    pub calculations: Vec<Calculation>,
    pub parameters: Vec<Parameter>,
    pub worksheets: Vec<Worksheet>,
    pub dashboards: Vec<Dashboard>,
    datasource_by_name: HashMap<String, DatasourceId>,
    field_by_name: Vec<HashMap<String, FieldId>>,
    field_by_label_hash: FastHashMap<u64, ExactField>,
    parameter_by_name: HashMap<String, ParameterId>,
    worksheet_by_name: HashMap<String, WorksheetId>,
    dashboard_by_name: HashMap<String, DashboardId>,
    root_children: HashMap<String, NodeId>,
}

impl Workbook {
    /// Admits UTF-8 TWB bytes and builds the semantic index.
    ///
    /// The root must be `<workbook>` and `workbook/@version` must equal
    /// [`TWB_VERSION`]. Unknown Tableau constructs remain in
    /// the source-backed XML even when this index does not model them.
    pub fn open(bytes: Vec<u8>) -> Result<Self> {
        Self::open_document(Document::parse(bytes)?)
    }

    /// Takes ownership of TWB XML text and applies the same admission contract as [`Self::open`].
    ///
    /// Ownership avoids an additional source copy; it does not relax version or
    /// structural checks.
    pub fn open_string(source: String) -> Result<Self> {
        Self::open_document(Document::parse_string(source)?)
    }

    fn open_document(xml: Document) -> Result<Self> {
        let root = xml.root();
        require(
            xml.tag(root) == "workbook",
            "FORMAT",
            "XML root is not <workbook>",
        )?;

        let version = attr_string(&xml, root, "version")?.unwrap_or_default();
        require(
            version == TWB_VERSION,
            "VERSION",
            format!(
                "TapKit targets Tableau {TABLEAU_PRODUCT_TARGET} workbooks (TWB version {TWB_VERSION}); got {version:?}"
            ),
        )?;

        let mut root_children = HashMap::new();
        for child in xml.children(root) {
            root_children
                .entry(xml.tag(child).to_owned())
                .or_insert(child);
        }

        let mut datasources = Vec::new();
        let mut connections = Vec::new();
        let mut relations = Vec::new();
        let mut fields = Vec::new();
        let mut calculations = Vec::new();
        let mut parameters = Vec::new();
        let mut parameter_by_name = HashMap::new();
        let mut datasource_by_name = HashMap::new();
        let mut field_by_name = Vec::new();
        let mut field_by_label_hash = FastHashMap::<u64, ExactField>::default();
        field_by_label_hash.reserve(1024);

        if let Some(datasources_root) = root_children.get("datasources").copied() {
            for datasource_node in xml
                .children(datasources_root)
                .filter(|id| xml.tag(*id) == "datasource")
            {
                let id = DatasourceId(datasources.len() as u32);
                let name = attr_string(&xml, datasource_node, "name")?.unwrap_or_default();
                let caption = attr_string(&xml, datasource_node, "caption")?;
                let inline = attr_string(&xml, datasource_node, "inline")?
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                    || xml.children(datasource_node).next().is_some();

                if !name.is_empty() {
                    datasource_by_name.entry(name.clone()).or_insert(id);
                }
                datasources.push(Datasource {
                    id,
                    name: name.clone(),
                    caption,
                    inline,
                    node: datasource_node,
                });

                let mut lookup = HashMap::new();
                for column in xml
                    .children(datasource_node)
                    .filter(|node| xml.tag(*node) == "column")
                {
                    let field_id = FieldId(fields.len() as u32);
                    let internal_name = required_attr(&xml, column, "name")?;
                    let caption = attr_string(&xml, column, "caption")?;
                    let datatype = DataType::parse(
                        attr_string(&xml, column, "datatype")?
                            .as_deref()
                            .unwrap_or("unknown"),
                    );
                    let role = Role::parse(
                        attr_string(&xml, column, "role")?
                            .as_deref()
                            .unwrap_or("unknown"),
                    );
                    let field_type = FieldType::parse(
                        attr_string(&xml, column, "type")?
                            .as_deref()
                            .unwrap_or("unknown"),
                    );
                    let parameter_domain = attr_string(&xml, column, "param-domain-type")?;
                    let parameter_value = attr_string(&xml, column, "value")?;

                    let calculation = if parameter_domain.is_none() {
                        if let Some(calc_node) = xml
                            .children(column)
                            .find(|child| xml.tag(*child) == "calculation")
                        {
                            let formula =
                                attr_string(&xml, calc_node, "formula")?.unwrap_or_default();
                            let calc_id = CalculationId(calculations.len() as u32);
                            calculations.push(Calculation {
                                id: calc_id,
                                field: field_id,
                                formula,
                                node: calc_node,
                            });
                            Some(calc_id)
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    lookup.entry(internal_name.clone()).or_insert(field_id);
                    if let Some(caption) = &caption {
                        lookup.entry(caption.clone()).or_insert(field_id);
                    }
                    let label = caption.as_deref().unwrap_or(&internal_name);
                    field_by_label_hash
                        .entry(ascii_case_hash(label))
                        .and_modify(|entry| *entry = ExactField::Ambiguous)
                        .or_insert(ExactField::One(field_id));
                    fields.push(Field {
                        id: field_id,
                        datasource: id,
                        name: internal_name.clone(),
                        caption: caption.clone(),
                        datatype,
                        role,
                        field_type,
                        calculation,
                        node: column,
                    });
                    if name == "Parameters"
                        && let Some(domain) = parameter_domain.as_deref()
                    {
                        let parameter_id = ParameterId(parameters.len() as u32);
                        parameter_by_name
                            .entry(internal_name.clone())
                            .or_insert(parameter_id);
                        if let Some(caption) = &caption {
                            parameter_by_name
                                .entry(caption.clone())
                                .or_insert(parameter_id);
                        }
                        parameters.push(Parameter {
                            id: parameter_id,
                            field: field_id,
                            name: internal_name.clone(),
                            caption: caption.clone(),
                            datatype,
                            domain: ParameterDomainKind::parse(domain),
                            value: parameter_value,
                            node: column,
                        });
                    }
                }
                field_by_name.push(lookup);

                let mut extract_scopes = Vec::<u32>::new();
                let mut object_graph_scopes = Vec::<u32>::new();
                for node in xml.descendants(datasource_node) {
                    while extract_scopes.last().is_some_and(|end| node.0 >= *end) {
                        extract_scopes.pop();
                    }
                    while object_graph_scopes.last().is_some_and(|end| node.0 >= *end) {
                        object_graph_scopes.pop();
                    }

                    let tag = xml.tag(node);
                    if tag == "extract" {
                        extract_scopes.push(xml.subtree_end(node));
                    }
                    if tag == "object-graph" {
                        object_graph_scopes.push(xml.subtree_end(node));
                    }
                    let in_extract = !extract_scopes.is_empty();
                    let in_object_graph = !object_graph_scopes.is_empty();

                    match tag {
                        "connection" => {
                            let scope = if in_extract {
                                ConnectionScope::Extract
                            } else {
                                ConnectionScope::Source
                            };
                            let parent_name = xml
                                .parent(node)
                                .filter(|parent| xml.tag(*parent) == "named-connection")
                                .and_then(|parent| {
                                    attr_string(&xml, parent, "name").ok().flatten()
                                });
                            let class = attr_string(&xml, node, "class")?;
                            let name = parent_name
                                .or(attr_string(&xml, node, "name")?)
                                .or_else(|| class.clone())
                                .unwrap_or_else(|| "connection".into());
                            let connection_id = ConnectionId(connections.len() as u32);
                            connections.push(Connection {
                                id: connection_id,
                                datasource: id,
                                name,
                                class,
                                server: attr_string(&xml, node, "server")?,
                                database: attr_string(&xml, node, "dbname")?,
                                schema: attr_string(&xml, node, "schema")?,
                                initial_sql: attr_string(&xml, node, "one-time-sql")?
                                    .filter(|sql| !sql.is_empty()),
                                scope,
                                node,
                            });
                        }
                        "relation" if !in_extract => {
                            let kind = match attr_string(&xml, node, "type")?.as_deref() {
                                Some("table") => Some(RelationKind::PhysicalTable),
                                Some("text") => Some(RelationKind::CustomSql),
                                Some("join") => Some(RelationKind::Join),
                                _ => None,
                            };
                            if let Some(kind) = kind {
                                let table = attr_string(&xml, node, "table")?;
                                let name = attr_string(&xml, node, "name")?
                                    .or_else(|| table.clone())
                                    .unwrap_or_else(|| match kind {
                                        RelationKind::CustomSql => "Custom SQL".into(),
                                        RelationKind::Join => "Join".into(),
                                        _ => "Relation".into(),
                                    });
                                let sql = (kind == RelationKind::CustomSql).then(|| {
                                    xml.text(node).map(str::trim).unwrap_or("").to_owned()
                                });
                                let relation_id = RelationId(relations.len() as u32);
                                relations.push(Relation {
                                    id: relation_id,
                                    datasource: id,
                                    kind,
                                    name,
                                    connection: attr_string(&xml, node, "connection")?,
                                    table,
                                    sql,
                                    node,
                                });
                            }
                        }
                        "object" if in_object_graph => {
                            let name = attr_string(&xml, node, "caption")?
                                .or(attr_string(&xml, node, "id")?)
                                .unwrap_or_else(|| "Logical table".into());
                            let relation_id = RelationId(relations.len() as u32);
                            relations.push(Relation {
                                id: relation_id,
                                datasource: id,
                                kind: RelationKind::LogicalTable,
                                name,
                                connection: None,
                                table: None,
                                sql: None,
                                node,
                            });
                        }
                        "relationship" => {
                            let relation_id = RelationId(relations.len() as u32);
                            relations.push(Relation {
                                id: relation_id,
                                datasource: id,
                                kind: RelationKind::Relationship,
                                name: format!("Relationship {}", relation_id.0),
                                connection: None,
                                table: None,
                                sql: None,
                                node,
                            });
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut worksheets = Vec::new();
        let mut worksheet_by_name = HashMap::new();
        if let Some(root) = root_children.get("worksheets").copied() {
            for node in xml
                .children(root)
                .filter(|node| xml.tag(*node) == "worksheet")
            {
                let id = WorksheetId(worksheets.len() as u32);
                let name = attr_string(&xml, node, "name")?.unwrap_or_default();
                worksheet_by_name.entry(name.clone()).or_insert(id);
                worksheets.push(Worksheet { id, name, node });
            }
        }

        let mut dashboards = Vec::new();
        let mut dashboard_by_name = HashMap::new();
        if let Some(root) = root_children.get("dashboards").copied() {
            for node in xml
                .children(root)
                .filter(|node| xml.tag(*node) == "dashboard")
            {
                let id = DashboardId(dashboards.len() as u32);
                let name = attr_string(&xml, node, "name")?.unwrap_or_default();
                dashboard_by_name.entry(name.clone()).or_insert(id);
                dashboards.push(Dashboard { id, name, node });
            }
        }

        Ok(Self {
            xml,
            datasources,
            connections,
            relations,
            fields,
            calculations,
            parameters,
            worksheets,
            dashboards,
            datasource_by_name,
            field_by_name,
            field_by_label_hash,
            parameter_by_name,
            worksheet_by_name,
            dashboard_by_name,
            root_children,
        })
    }

    /// Summarizes the indexed subset of this workbook snapshot.
    ///
    /// Counts describe TapKit's semantic index, not the full set of XML elements
    /// permitted by Tableau's schema.
    pub fn summary(&self) -> Summary {
        Summary {
            version: TWB_VERSION.into(),
            xml_nodes: self.xml.node_count(),
            datasources: self.datasources.len(),
            connections: self.connections.len(),
            relations: self.relations.len(),
            fields: self.fields.len(),
            calculations: self.calculations.len(),
            parameters: self.parameters.len(),
            worksheets: self.worksheets.len(),
            dashboards: self.dashboards.len(),
        }
    }

    pub fn datasource_id(&self, name: &str) -> Option<DatasourceId> {
        self.datasource_by_name.get(name).copied()
    }

    pub fn datasource(&self, id: DatasourceId) -> Option<&Datasource> {
        self.datasources.get(id.0 as usize)
    }

    pub fn field_id(&self, datasource: DatasourceId, name: &str) -> Option<FieldId> {
        self.field_by_name
            .get(datasource.0 as usize)
            .and_then(|lookup| lookup.get(name))
            .copied()
    }

    pub fn field(&self, id: FieldId) -> Option<&Field> {
        self.fields.get(id.0 as usize)
    }

    pub fn parameter_id(&self, name: &str) -> Option<ParameterId> {
        self.parameter_by_name.get(name).copied()
    }

    pub fn parameter(&self, id: ParameterId) -> Option<&Parameter> {
        self.parameters.get(id.0 as usize)
    }

    pub fn worksheet_id(&self, name: &str) -> Option<WorksheetId> {
        self.worksheet_by_name.get(name).copied()
    }

    pub fn dashboard_id(&self, name: &str) -> Option<DashboardId> {
        self.dashboard_by_name.get(name).copied()
    }

    pub fn root_child(&self, tag: &str) -> Option<NodeId> {
        self.root_children.get(tag).copied()
    }
}

/// Inspects the indexed semantic subset without truncating matches.
///
/// Exact case-insensitive name/caption matches suppress substring fallback. Use
/// [`inspect_limited`] on agent-facing or otherwise bounded paths.
pub fn inspect(book: &Workbook, kind: &str, query: Option<&str>) -> Result<serde_json::Value> {
    inspect_limited(book, kind, query, usize::MAX)
}

/// Inspects indexed objects with exact-first lookup and an explicit output bound.
///
/// `kind` must be one of the semantic object kinds supported by this index.
/// `limit` bounds returned items but `total` still reports the number of matches;
/// the result sets `truncated` when additional matches were omitted.
pub fn inspect_limited(
    book: &Workbook,
    kind: &str,
    query: Option<&str>,
    limit: usize,
) -> Result<serde_json::Value> {
    require(limit > 0, "ARGUMENT", "Inspect limit must be positive")?;

    let (items, total) = match kind {
        "datasource" => select_values(
            &book.datasources,
            query,
            limit,
            |datasource| {
                datasource
                    .caption
                    .as_deref()
                    .filter(|caption| !caption.is_empty())
                    .unwrap_or(&datasource.name)
            },
            |datasource| Ok(serde_json::to_value(datasource)?),
        )?,
        "connection" => select_values(
            &book.connections,
            query,
            limit,
            |connection| &connection.name,
            |connection| Ok(serde_json::to_value(connection)?),
        )?,
        "relation" => select_values(
            &book.relations,
            query,
            limit,
            |relation| &relation.name,
            |relation| Ok(serde_json::to_value(relation)?),
        )?,
        "field" => select_fields(book, query, limit)?,
        "calculation" => select_calculations(book, query, limit)?,
        "parameter" => select_values(
            &book.parameters,
            query,
            limit,
            |parameter| parameter.caption.as_deref().unwrap_or(&parameter.name),
            |parameter| {
                Ok(serde_json::json!({
                    "id": parameter.id,
                    "name": parameter.name,
                    "caption": parameter.caption,
                    "datatype": parameter.datatype,
                    "domain": parameter.domain,
                    "value": parameter.value
                }))
            },
        )?,
        "worksheet" => select_values(
            &book.worksheets,
            query,
            limit,
            |worksheet| &worksheet.name,
            |worksheet| Ok(serde_json::to_value(worksheet)?),
        )?,
        "dashboard" => select_values(
            &book.dashboards,
            query,
            limit,
            |dashboard| &dashboard.name,
            |dashboard| Ok(serde_json::to_value(dashboard)?),
        )?,
        _ => {
            return Err(Error::new(
                "ARGUMENT",
                format!(
                    "Unknown inspect kind {kind:?}; expected datasource, connection, relation, field, calculation, parameter, worksheet, or dashboard"
                ),
            ));
        }
    };

    Ok(serde_json::json!({
        "kind": kind,
        "query": query,
        "count": items.len(),
        "total": total,
        "truncated": total > items.len(),
        "items": items
    }))
}

fn field_value(book: &Workbook, field: &Field) -> serde_json::Value {
    let datasource = book
        .datasource(field.datasource)
        .map(|datasource| datasource.name.as_str())
        .unwrap_or("");
    serde_json::json!({
        "id": field.id,
        "datasource": datasource,
        "name": field.name,
        "caption": field.caption,
        "datatype": field.datatype,
        "role": field.role,
        "type": field.field_type,
        "calculation": field.calculation
    })
}

fn calculation_value(book: &Workbook, calculation: &Calculation) -> Result<serde_json::Value> {
    let field = book
        .field(calculation.field)
        .ok_or_else(|| Error::new("INTERNAL", "Calculation output field is missing"))?;
    let datasource = book
        .datasource(field.datasource)
        .map(|datasource| datasource.name.as_str())
        .unwrap_or("");
    Ok(serde_json::json!({
        "id": calculation.id,
        "datasource": datasource,
        "field": field.id,
        "name": field.name,
        "caption": field.caption,
        "datatype": field.datatype,
        "role": field.role,
        "type": field.field_type,
        "formula": calculation.formula
    }))
}

fn unique_exact_field<'a>(book: &'a Workbook, query: &str) -> Result<Option<&'a Field>> {
    let Some(ExactField::One(field_id)) = book.field_by_label_hash.get(&ascii_case_hash(query))
    else {
        return Ok(None);
    };
    let field = book
        .field(*field_id)
        .ok_or_else(|| Error::new("INTERNAL", "Field exact index is invalid"))?;
    let label = field.caption.as_deref().unwrap_or(&field.name);
    Ok(label.eq_ignore_ascii_case(query).then_some(field))
}

fn select_fields(
    book: &Workbook,
    query: Option<&str>,
    limit: usize,
) -> Result<(Vec<serde_json::Value>, usize)> {
    if let Some(query) = query
        && let Some(field) = unique_exact_field(book, query)?
    {
        return Ok((
            (limit > 0)
                .then(|| field_value(book, field))
                .into_iter()
                .collect(),
            1,
        ));
    }

    select_values(
        &book.fields,
        query,
        limit,
        |field| field.caption.as_deref().unwrap_or(&field.name),
        |field| Ok(field_value(book, field)),
    )
}

fn select_calculations(
    book: &Workbook,
    query: Option<&str>,
    limit: usize,
) -> Result<(Vec<serde_json::Value>, usize)> {
    if let Some(query) = query
        && let Some(field) = unique_exact_field(book, query)?
        && let Some(calculation_id) = field.calculation
    {
        let calculation = book
            .calculations
            .get(calculation_id.0 as usize)
            .ok_or_else(|| Error::new("INTERNAL", "Calculation index is invalid"))?;
        return Ok((
            (limit > 0)
                .then(|| calculation_value(book, calculation))
                .transpose()?
                .into_iter()
                .collect(),
            1,
        ));
    }

    let Some(query) = query else {
        let mut selected = Vec::with_capacity(book.calculations.len().min(limit));
        for calculation in book.calculations.iter().take(limit) {
            selected.push(calculation_value(book, calculation)?);
        }
        return Ok((selected, book.calculations.len()));
    };

    let mut exact = Vec::with_capacity(limit.min(1));
    let mut partial = Vec::with_capacity(limit.min(1));
    let mut exact_total = 0usize;
    let mut partial_total = 0usize;

    for calculation in &book.calculations {
        let name = calculation_name(book, calculation);
        if name.eq_ignore_ascii_case(query) {
            exact_total += 1;
            if exact.len() < limit {
                exact.push(calculation_value(book, calculation)?);
            }
        } else if contains_case_insensitive(name, query) {
            partial_total += 1;
            if partial.len() < limit {
                partial.push(calculation_value(book, calculation)?);
            }
        }
    }

    if exact_total > 0 {
        Ok((exact, exact_total))
    } else {
        Ok((partial, partial_total))
    }
}

fn calculation_name<'a>(book: &'a Workbook, calculation: &Calculation) -> &'a str {
    let field = &book.fields[calculation.field.0 as usize];
    field.caption.as_deref().unwrap_or(&field.name)
}

fn select_values<T>(
    items: &[T],
    query: Option<&str>,
    limit: usize,
    name: impl for<'a> Fn(&'a T) -> &'a str,
    value: impl Fn(&T) -> Result<serde_json::Value>,
) -> Result<(Vec<serde_json::Value>, usize)> {
    let Some(query) = query else {
        let mut selected = Vec::with_capacity(items.len().min(limit));
        for item in items.iter().take(limit) {
            selected.push(value(item)?);
        }
        return Ok((selected, items.len()));
    };

    let mut exact = Vec::with_capacity(limit.min(1));
    let mut partial = Vec::with_capacity(limit.min(1));
    let mut exact_total = 0usize;
    let mut partial_total = 0usize;

    for item in items {
        let item_name = name(item);
        if item_name.eq_ignore_ascii_case(query) {
            exact_total += 1;
            if exact.len() < limit {
                exact.push(value(item)?);
            }
        } else if contains_case_insensitive(item_name, query) {
            partial_total += 1;
            if partial.len() < limit {
                partial.push(value(item)?);
            }
        }
    }

    if exact_total > 0 {
        Ok((exact, exact_total))
    } else {
        Ok((partial, partial_total))
    }
}

fn attr_string(xml: &Document, node: NodeId, name: &str) -> Result<Option<String>> {
    Ok(xml.attr(node, name)?.map(|value| value.into_owned()))
}

fn required_attr(xml: &Document, node: NodeId, name: &str) -> Result<String> {
    attr_string(xml, node, name)?.ok_or_else(|| {
        Error::new(
            "WORKBOOK_SHAPE",
            format!("<{}> lacks {name}", xml.tag(node)),
        )
    })
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn accepts_26_2_and_rejects_26_3() -> Result<()> {
        let accepted =
            Workbook::open(br#"<workbook version="26.2" source-build="20262.test"/>"#.to_vec())?;
        assert_eq!(accepted.summary().version, "26.2");

        let rejected = match Workbook::open(
            br#"<workbook version="26.3" source-build="20263.test"/>"#.to_vec(),
        ) {
            Ok(_) => {
                return Err(Error::new(
                    "TEST",
                    "26.3 was unexpectedly accepted by the 2026.2.4 target gate",
                ));
            }
            Err(error) => error,
        };
        assert_eq!(rejected.code, "VERSION");
        Ok(())
    }
}

#[cfg(test)]
mod inspect_tests {
    use super::*;

    fn fixture() -> Result<Workbook> {
        Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      <column caption="Sales Tax" datatype="real" name="[Sales Tax]" role="measure" type="quantitative">
        <calculation class="tableau" formula="[Sales] * 0.2"/>
      </column>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )
    }

    #[test]
    fn ambiguous_exact_field_labels_fall_back_without_losing_matches() -> Result<()> {
        let book = Workbook::open(
            br#"<workbook version="26.2" source-build="20262.test">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
    </datasource>
    <datasource inline="true" name="Forecast" version="26.2">
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
</workbook>"#
                .to_vec(),
        )?;

        let exact = inspect_limited(&book, "field", Some("SALES"), 10)?;
        assert_eq!(exact["total"], 2);
        assert_eq!(exact["count"], 2);
        assert_eq!(exact["items"][0]["caption"], "Sales");
        assert_eq!(exact["items"][1]["caption"], "Sales");
        Ok(())
    }

    #[test]
    fn exact_field_and_calculation_inspect_precede_substring_matches() -> Result<()> {
        let book = fixture()?;

        let exact = inspect_limited(&book, "field", Some("sAlEs"), 10)?;
        assert_eq!(exact["total"], 1);
        assert_eq!(exact["items"][0]["caption"], "Sales");

        let partial = inspect_limited(&book, "field", Some("sale"), 10)?;
        assert_eq!(partial["total"], 2);

        let calculation = inspect_limited(&book, "calculation", Some("SALES TAX"), 10)?;
        assert_eq!(calculation["total"], 1);
        assert_eq!(calculation["items"][0]["caption"], "Sales Tax");
        Ok(())
    }
}
