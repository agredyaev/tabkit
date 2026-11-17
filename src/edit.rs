//! Typed, source-preserving workbook edits.
//!
//! An edit batch is evaluated against one immutable [`Workbook`] base. TapKit
//! validates semantic preconditions first, lowers accepted operations to
//! non-overlapping expected-source patches, applies them as one candidate, then
//! reparses and checks operation-specific postconditions.
//!
//! The base workbook is never mutated. Raw XML mutation is deliberately not part
//! of this API; unsupported Tableau constructs fail closed rather than being
//! approximated.

use crate::{
    Error, Result, formula,
    patch::{self, Patch},
    require,
    workbook::{DataType, DatasourceId, FieldType, Role, Workbook, WorksheetId},
    xml::{NodeId, Span},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Write as _,
};

const CANVAS: u32 = 100_000;

/// Atomic semantic edit plan evaluated against one workbook base.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub edits: Vec<Edit>,
}

/// Operations for which TapKit implements both admission checks and postconditions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    CreateCalculation(CalculationSpec),
    CreateParameter(ParameterSpec),
    CreateWorksheet(WorksheetSpec),
    CreateFilterAction(FilterActionSpec),
    CreateParameterAction(ParameterActionSpec),
    CreateDashboard(DashboardSpec),
}

/// Contract for a new calculation in an existing inline datasource.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalculationSpec {
    pub datasource: String,
    pub caption: String,
    pub formula: String,
    pub datatype: DataType,
    pub role: Role,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub name: Option<String>,
}

/// Tableau parameter authored in the synthetic `Parameters` datasource.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParameterSpec {
    pub caption: String,
    pub datatype: DataType,
    pub value: serde_json::Value,
    pub domain: ParameterDomain,
    pub name: Option<String>,
}

/// Admitted value domain for a newly authored parameter.
///
/// List domains are bounded and must contain the current value. Range domains are
/// accepted only for numeric datatypes and require an ordered interval with a
/// positive step.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParameterDomain {
    Any,
    List {
        values: Vec<serde_json::Value>,
    },
    Range {
        min: serde_json::Value,
        max: serde_json::Value,
        step: serde_json::Value,
    },
}

/// Minimal worksheet families whose Tableau 2026.2 structure is deterministic.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorksheetKind {
    Bar,
    Line,
    Kpi,
    TextTable,
}

/// Aggregations supported by the bounded worksheet compiler.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Sum,
    Average,
    Min,
    Max,
    Count,
    CountDistinct,
}

impl Default for Aggregation {
    fn default() -> Self {
        Self::Sum
    }
}

/// Narrow worksheet compiler contract over fields already present in one datasource.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorksheetSpec {
    pub name: String,
    pub datasource: String,
    pub kind: WorksheetKind,
    pub dimension: Option<String>,
    pub secondary_dimension: Option<String>,
    pub measure: String,
    #[serde(default)]
    pub aggregation: Aggregation,
}

/// Tableau activation modes intentionally exposed by the MVP action compiler.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionTrigger {
    #[default]
    Select,
    Hover,
}

/// Dashboard-wide filter action using Tableau's `tsc:tsl-filter` contract.
///
/// The source worksheet must already be placed on `dashboard`. It is excluded
/// from its own action; other compatible worksheets on that dashboard are the
/// action targets. This does not synthesize persisted `Apply to Worksheets` state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterActionSpec {
    pub caption: String,
    pub dashboard: String,
    pub source_sheet: String,
    #[serde(default)]
    pub run_on: ActionTrigger,
    #[serde(default = "default_true")]
    pub auto_clear: bool,
}

/// Parameter action from one worksheet field instance to an existing parameter.
///
/// The source worksheet must already be placed on `dashboard`. Source and target
/// datatypes must match. `source_instance` is required only when the worksheet
/// exposes multiple Tableau instances for the requested field.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParameterActionSpec {
    pub caption: String,
    pub dashboard: String,
    pub source_sheet: String,
    pub source_field: String,
    pub source_instance: Option<String>,
    pub target_parameter: String,
    #[serde(default)]
    pub run_on: ActionTrigger,
}

/// Fixed-size dashboard specification built only from already indexed worksheets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DashboardSpec {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub padding: u32,
    pub background: Option<String>,
    #[serde(default)]
    pub view_styles: Vec<ViewStyleSpec>,
    pub layout: Layout,
}

/// Recursive layout tree normalized onto TapKit's fixed 100,000-unit canvas.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Layout {
    Row {
        #[serde(default)]
        gap: u32,
        #[serde(default)]
        padding: u32,
        children: Vec<Item>,
    },
    Column {
        #[serde(default)]
        gap: u32,
        #[serde(default)]
        padding: u32,
        children: Vec<Item>,
    },
    Sheet {
        sheet: String,
        show_title: Option<bool>,
        fit: Option<FitMode>,
    },
    Filter {
        source_sheet: String,
        field: String,
        instance: Option<String>,
        #[serde(default)]
        control: FilterControl,
        values: Option<FilterValues>,
        title: Option<String>,
    },
    Text {
        text: String,
        font_family: Option<String>,
        font_size: Option<u32>,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        color: Option<String>,
        align: Option<TextAlign>,
    },
    Spacer,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Item {
    #[serde(default = "one")]
    pub weight: u32,
    pub fixed_size: Option<u32>,
    #[serde(default)]
    pub style: ZoneStyle,
    #[serde(flatten)]
    pub layout: Layout,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoneStyle {
    pub background: Option<String>,
    pub border_color: Option<String>,
    #[serde(default)]
    pub border_width: u32,
    #[serde(default)]
    pub corner_radius: u32,
    #[serde(default)]
    pub padding: u32,
    #[serde(default)]
    pub margin: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Standard,
    EntireView,
    FitWidth,
    FitHeight,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterControl {
    #[default]
    Auto,
    SingleDropdown,
    MultipleDropdown,
    SingleList,
    MultipleList,
    Slider,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterValues {
    AllDatabase,
    Relevant,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewStyleSpec {
    pub sheet: String,
    pub font_family: Option<String>,
    pub font_size: Option<u32>,
    pub color: Option<String>,
    pub axis: Option<LineStyle>,
    pub grid_lines: Option<LineStyle>,
    pub zero_line: Option<LineStyle>,
    pub line_width: Option<f32>,
    pub show_labels: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineStyle {
    #[serde(default = "default_true")]
    pub visible: bool,
    pub color: Option<String>,
    pub width: Option<u32>,
}

/// Deterministic receipt for a reparsed candidate.
#[derive(Debug, Serialize)]
pub struct Receipt {
    pub sha256: String,
    pub calculations: Vec<String>,
    pub parameters: Vec<String>,
    pub worksheets: Vec<String>,
    pub actions: Vec<String>,
    pub dashboards: Vec<String>,
}

/// Detached candidate whose XML has been reparsed and whose edit postconditions hold.
pub struct Candidate {
    pub workbook: Workbook,
    pub receipt: Receipt,
}

impl Candidate {
    /// Borrows the exact TWB source represented by `workbook` and `receipt.sha256`.
    pub fn text(&self) -> &str {
        self.workbook.xml.source()
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone, Copy)]
struct Flow {
    gap: u32,
    padding: u32,
    horizontal: bool,
}

struct ZoneRenderer<'a> {
    dashboard_width: u32,
    dashboard_height: u32,
    filters: &'a [FilterBinding],
    filter_cursor: usize,
    next_id: u32,
}

/// Produces a verified candidate without mutating `book`.
///
/// # Invariants
///
/// * every edit is validated against the same base plus explicitly planned
///   same-batch calculations;
/// * generated patches are non-overlapping and satisfy expected-source
///   preconditions;
/// * the candidate remains within `max_bytes`;
/// * the resulting XML is reparsed, and every created calculation, parameter,
///   worksheet, action, and dashboard is checked before success is returned.
///
/// Failure is atomic from the caller's perspective: no partially edited
/// [`Workbook`] is returned and the input workbook is unchanged.
pub fn apply(book: &Workbook, batch: &Batch, max_bytes: u64) -> Result<Candidate> {
    require(
        !batch.edits.is_empty(),
        "EMPTY_BATCH",
        "At least one edit is required",
    )?;

    let mut calculation_keys = HashSet::new();
    let mut parameter_captions = HashSet::new();
    let mut worksheet_names = HashSet::new();
    let mut action_captions = HashSet::new();
    let mut dashboard_names = HashSet::new();
    let mut dashboard_style_sheets = HashSet::new();

    for edit in &batch.edits {
        match edit {
            Edit::CreateCalculation(spec) => {
                require(
                    calculation_keys.insert((spec.datasource.clone(), spec.caption.clone())),
                    "DUPLICATE_EDIT",
                    format!(
                        "Calculation {:?} is created more than once in datasource {:?}",
                        spec.caption, spec.datasource
                    ),
                )?;
            }
            Edit::CreateParameter(spec) => {
                require(
                    parameter_captions.insert(spec.caption.clone()),
                    "DUPLICATE_EDIT",
                    format!("Parameter {:?} is created more than once", spec.caption),
                )?;
            }
            Edit::CreateWorksheet(spec) => {
                require(
                    worksheet_names.insert(spec.name.clone()),
                    "DUPLICATE_EDIT",
                    format!("Worksheet {:?} is created more than once", spec.name),
                )?;
            }
            Edit::CreateFilterAction(spec) => {
                require(
                    action_captions.insert(spec.caption.clone()),
                    "DUPLICATE_EDIT",
                    format!("Action {:?} is created more than once", spec.caption),
                )?;
            }
            Edit::CreateParameterAction(spec) => {
                require(
                    action_captions.insert(spec.caption.clone()),
                    "DUPLICATE_EDIT",
                    format!("Action {:?} is created more than once", spec.caption),
                )?;
            }
            Edit::CreateDashboard(spec) => {
                require(
                    dashboard_names.insert(spec.name.clone()),
                    "DUPLICATE_EDIT",
                    format!("Dashboard {:?} is created more than once", spec.name),
                )?;
                for style in &spec.view_styles {
                    require(
                        dashboard_style_sheets.insert(style.sheet.clone()),
                        "DUPLICATE_STYLE",
                        format!(
                            "Worksheet {:?} is styled by more than one dashboard in the same batch",
                            style.sheet
                        ),
                    )?;
                }
            }
        }
    }

    let planned_calculations = planned_calculation_names(book, batch)?;
    let mut dashboard_zone_cursor = if dashboard_names.is_empty() {
        None
    } else {
        Some(next_dashboard_zone_id(book)?)
    };

    let mut patches = Vec::new();
    let mut calculations = Vec::new();
    let mut calculation_checks = Vec::new();
    let mut parameters = Vec::new();
    let mut worksheets = Vec::new();
    let mut actions = Vec::new();
    let mut dashboards = Vec::new();

    let parameter_specs = batch
        .edits
        .iter()
        .filter_map(|edit| match edit {
            Edit::CreateParameter(spec) => Some(spec),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !parameter_specs.is_empty() {
        let (patch, created) = parameter_batch_patch(book, &parameter_specs)?;
        patches.push(patch);
        parameters = created;
    }

    let mut worksheet_xml_batch = String::new();
    let mut filter_action_xml_batch = String::new();
    let mut parameter_action_xml_batch = String::new();
    let mut parameter_dashboard_requests = BTreeMap::<String, BTreeSet<String>>::new();
    let mut dashboard_xml = String::new();
    let mut window_xml = String::new();
    let mut dashboard_other_patches = Vec::new();
    let mut filter_indexes = HashMap::<WorksheetId, WorksheetFilterIndex<'_>>::new();

    for edit in &batch.edits {
        match edit {
            Edit::CreateCalculation(spec) => {
                let (patch, name) = calculation_patch(book, spec, &planned_calculations)?;
                patches.push(patch);
                calculation_checks.push((spec.datasource.clone(), name.clone()));
                calculations.push(name);
            }
            Edit::CreateParameter(_) => {}
            Edit::CreateWorksheet(spec) => {
                worksheet_xml_batch.push_str(&worksheet_xml(book, spec)?);
                worksheets.push(spec.name.clone());
            }
            Edit::CreateFilterAction(spec) => {
                filter_action_xml_batch.push_str(&filter_action_xml(book, spec)?);
                actions.push(spec.caption.clone());
            }
            Edit::CreateParameterAction(spec) => {
                let (xml, parameter_name) = parameter_action_xml(book, spec)?;
                parameter_action_xml_batch.push_str(&xml);
                parameter_dashboard_requests
                    .entry(spec.dashboard.clone())
                    .or_default()
                    .insert(parameter_name);
                actions.push(spec.caption.clone());
            }
            Edit::CreateDashboard(spec) => {
                let parts = dashboard_parts(
                    book,
                    spec,
                    dashboard_zone_cursor.as_mut().ok_or_else(|| {
                        Error::new("INTERNAL", "Dashboard zone cursor is missing")
                    })?,
                    &mut filter_indexes,
                )?;
                dashboard_xml.push_str(&parts.dashboard);
                window_xml.push_str(&parts.window);
                dashboard_other_patches.extend(parts.other_patches);
                dashboards.push(spec.name.clone());
            }
        }
    }

    if !worksheet_xml_batch.is_empty() {
        patches.push(worksheet_batch_patch(book, worksheet_xml_batch)?);
    }
    patches.extend(action_patches(
        book,
        filter_action_xml_batch,
        parameter_action_xml_batch,
    )?);
    patches.extend(parameter_dashboard_dependency_patches(
        book,
        &parameter_dashboard_requests,
    )?);

    if !dashboards.is_empty() {
        patches.push(dashboard_batch_patch(book, dashboard_xml)?);
        patches.push(dashboard_window_batch_patch(book, window_xml)?);
        patches.extend(dashboard_other_patches);
    }

    let text = patch::apply(book.xml.source(), &patches, max_bytes)?;
    let reopened = Workbook::open_string(text)?;

    for (datasource, name) in &calculation_checks {
        let datasource_id = reopened.datasource_id(datasource).ok_or_else(|| {
            Error::new(
                "POSTCONDITION",
                format!("Datasource {datasource:?} disappeared after edit"),
            )
        })?;
        let field_id = reopened.field_id(datasource_id, name).ok_or_else(|| {
            Error::new(
                "POSTCONDITION",
                format!("Generated calculation field {name:?} is not indexed after edit"),
            )
        })?;
        require(
            reopened
                .field(field_id)
                .is_some_and(|field| field.calculation.is_some()),
            "POSTCONDITION",
            format!("Generated field {name:?} is not indexed as a calculation"),
        )?;
    }
    for name in &parameters {
        require(
            reopened.parameter_id(name).is_some(),
            "POSTCONDITION",
            format!("Generated parameter {name:?} is not indexed after edit"),
        )?;
    }
    for name in &worksheets {
        require(
            reopened.worksheet_id(name).is_some(),
            "POSTCONDITION",
            format!("Generated worksheet {name:?} is not indexed after edit"),
        )?;
    }
    for caption in &actions {
        require(
            action_caption_exists(&reopened, caption)?,
            "POSTCONDITION",
            format!("Generated action {caption:?} is not present after edit"),
        )?;
    }
    for name in &dashboards {
        require(
            reopened.dashboard_id(name).is_some(),
            "POSTCONDITION",
            format!("Generated dashboard {name:?} is not indexed after edit"),
        )?;
    }

    let sha256 = reopened.xml.sha256.clone();
    Ok(Candidate {
        workbook: reopened,
        receipt: Receipt {
            sha256,
            calculations,
            parameters,
            worksheets,
            actions,
            dashboards,
        },
    })
}

fn planned_calculation_names(
    book: &Workbook,
    batch: &Batch,
) -> Result<HashMap<DatasourceId, HashSet<String>>> {
    let mut planned = HashMap::<DatasourceId, HashSet<String>>::new();
    for edit in &batch.edits {
        let Edit::CreateCalculation(spec) = edit else {
            continue;
        };
        let (datasource_id, internal_name) = validate_calculation_spec(book, spec)?;
        require(
            planned
                .entry(datasource_id)
                .or_default()
                .insert(internal_name.clone()),
            "CALCULATION_EXISTS",
            format!(
                "Calculation internal name {internal_name:?} is created more than once in datasource {:?}",
                spec.datasource
            ),
        )?;
    }
    Ok(planned)
}

fn validate_calculation_spec(
    book: &Workbook,
    spec: &CalculationSpec,
) -> Result<(DatasourceId, String)> {
    require(
        !spec.caption.trim().is_empty(),
        "CALCULATION",
        "Calculation caption cannot be empty",
    )?;
    require(
        spec.datatype != DataType::Unknown
            && spec.role != Role::Unknown
            && spec.field_type != FieldType::Unknown,
        "CALCULATION",
        "Authored calculations require explicit datatype, role, and field type",
    )?;

    let datasource_id = book.datasource_id(&spec.datasource).ok_or_else(|| {
        Error::new(
            "DATASOURCE_NOT_FOUND",
            format!(
                "Datasource {:?} does not exist in this workbook",
                spec.datasource
            ),
        )
    })?;
    let datasource = book
        .datasource(datasource_id)
        .ok_or_else(|| Error::new("INTERNAL", "Datasource id is invalid"))?;
    require(
        datasource.inline,
        "CALCULATION",
        "Calculation creation requires an inline datasource",
    )?;

    let internal_name = spec
        .name
        .clone()
        .unwrap_or_else(|| calculation_name(&book.xml.sha256, spec));
    require(
        internal_name.starts_with('[') && internal_name.ends_with(']'),
        "CALCULATION",
        "Calculation internal name must be bracketed",
    )?;
    require(
        book.field_id(datasource_id, &internal_name).is_none()
            && book.field_id(datasource_id, &spec.caption).is_none(),
        "CALCULATION_EXISTS",
        "A field with this name or caption already exists",
    )?;

    Ok((datasource_id, internal_name))
}

fn calculation_patch(
    book: &Workbook,
    spec: &CalculationSpec,
    planned_calculations: &HashMap<DatasourceId, HashSet<String>>,
) -> Result<(Patch, String)> {
    let (datasource_id, internal_name) = validate_calculation_spec(book, spec)?;
    let datasource = book
        .datasource(datasource_id)
        .ok_or_else(|| Error::new("INTERNAL", "Datasource id is invalid"))?;

    let analysis = formula::analyze(&spec.formula)?;
    for reference in analysis.references {
        let target_datasource = match reference.datasource.as_deref() {
            Some(name) => book.datasource_id(name).ok_or_else(|| {
                Error::new(
                    "DATASOURCE_NOT_FOUND",
                    format!("Formula qualifier {name:?} cannot be resolved"),
                )
            })?,
            None => datasource_id,
        };
        require(
            book.field_id(target_datasource, &reference.field).is_some()
                || planned_calculations
                    .get(&target_datasource)
                    .is_some_and(|fields| fields.contains(reference.field.as_str())),
            "FIELD_NOT_FOUND",
            format!("Formula reference {} cannot be resolved", reference.field),
        )?;
    }

    let at = datasource_column_insert_at(book, datasource.node)?;
    let xml = format!(
        "\n      <column caption=\"{}\" datatype=\"{}\" name=\"{}\" role=\"{}\" type=\"{}\">\n        <calculation class=\"tableau\" formula=\"{}\"/>\n      </column>",
        escape_attr(&spec.caption),
        spec.datatype.as_str(),
        escape_attr(&internal_name),
        spec.role.as_str(),
        spec.field_type.as_str(),
        escape_attr(&spec.formula),
    );

    Ok((
        Patch {
            span: Span::empty(at)?,
            expected: String::new(),
            replacement: xml,
            reason: format!("create calculation {}", spec.caption),
        },
        internal_name,
    ))
}

fn parameter_batch_patch(
    book: &Workbook,
    specs: &[&ParameterSpec],
) -> Result<(Patch, Vec<String>)> {
    let mut captions = HashSet::new();
    let mut names = HashSet::new();
    let mut columns = String::new();
    let mut created = Vec::with_capacity(specs.len());

    for spec in specs {
        require(
            !spec.caption.trim().is_empty(),
            "PARAMETER",
            "Parameter caption cannot be empty",
        )?;
        require(
            captions.insert(spec.caption.clone()),
            "DUPLICATE_EDIT",
            format!("Parameter {:?} is created more than once", spec.caption),
        )?;
        require(
            book.parameter_id(&spec.caption).is_none(),
            "PARAMETER_EXISTS",
            format!("Parameter {:?} already exists", spec.caption),
        )?;

        let name = spec
            .name
            .clone()
            .unwrap_or_else(|| parameter_name(&book.xml.sha256, &spec.caption));
        require(
            name.starts_with('[') && name.ends_with(']'),
            "PARAMETER",
            "Parameter internal name must be bracketed",
        )?;
        require(
            names.insert(name.clone()),
            "DUPLICATE_EDIT",
            format!("Parameter internal name {name:?} is created more than once"),
        )?;
        require(
            book.parameter_id(&name).is_none(),
            "PARAMETER_EXISTS",
            format!("Parameter internal name {name:?} already exists"),
        )?;

        let value = parameter_literal(spec.datatype, &spec.value)?;
        let field_type = match spec.datatype {
            DataType::String | DataType::Boolean => "nominal",
            DataType::Integer | DataType::Real => "quantitative",
            _ => {
                return Err(Error::new(
                    "PARAMETER",
                    "MVP parameter authoring supports string, integer, real, and boolean datatypes",
                ));
            }
        };
        let domain = parameter_domain_xml(spec.datatype, &spec.value, &spec.domain)?;
        write!(
            columns,
            "\n      <column caption=\"{}\" datatype=\"{}\" name=\"{}\" param-domain-type=\"{}\" role=\"measure\" type=\"{}\" value=\"{}\">\n        <calculation class=\"tableau\" formula=\"{}\"/>{}\n      </column>",
            escape_attr(&spec.caption),
            spec.datatype.as_str(),
            escape_attr(&name),
            parameter_domain_name(&spec.domain),
            field_type,
            escape_attr(&value),
            escape_attr(&value),
            domain,
        )
        .map_err(|_| Error::new("INTERNAL", "Failed to render parameter"))?;
        created.push(name);
    }

    let (at, replacement) = if let Some(datasource_id) = book.datasource_id("Parameters") {
        let datasource = book
            .datasource(datasource_id)
            .ok_or_else(|| Error::new("INTERNAL", "Parameters datasource id is invalid"))?;
        require(
            datasource.inline,
            "PARAMETER",
            "Parameters datasource must be inline",
        )?;
        (datasource_column_insert_at(book, datasource.node)?, columns)
    } else {
        let datasources = book.root_child("datasources").ok_or_else(|| {
            Error::new(
                "WORKBOOK_SHAPE",
                "Parameter creation requires an existing <datasources> container",
            )
        })?;
        let at = book.xml.closing_tag_start(datasources)?;
        (
            at,
            format!(
                "\n    <datasource hasconnection=\"false\" inline=\"true\" name=\"Parameters\" version=\"{}\">\n      <aliases enabled=\"yes\"/>{}\n    </datasource>",
                crate::TWB_VERSION,
                columns
            ),
        )
    };

    Ok((
        Patch {
            span: Span::empty(at)?,
            expected: String::new(),
            replacement,
            reason: "create parameter batch".into(),
        },
        created,
    ))
}

fn parameter_domain_name(domain: &ParameterDomain) -> &'static str {
    match domain {
        ParameterDomain::Any => "any",
        ParameterDomain::List { .. } => "list",
        ParameterDomain::Range { .. } => "range",
    }
}

fn parameter_domain_xml(
    datatype: DataType,
    current: &serde_json::Value,
    domain: &ParameterDomain,
) -> Result<String> {
    match domain {
        ParameterDomain::Any => Ok(String::new()),
        ParameterDomain::List { values } => {
            require(
                !values.is_empty() && values.len() <= 1000,
                "PARAMETER",
                "List parameter requires 1..1000 values",
            )?;
            require(
                values.iter().any(|value| value == current),
                "PARAMETER",
                "Current parameter value must be present in its list domain",
            )?;
            let mut members = String::new();
            for value in values {
                let literal = parameter_literal(datatype, value)?;
                write!(members, "<member value=\"{}\"/>", escape_attr(&literal))
                    .map_err(|_| Error::new("INTERNAL", "Failed to render parameter member"))?;
            }
            Ok(format!("\n        <members>{members}</members>"))
        }
        ParameterDomain::Range { min, max, step } => {
            require(
                matches!(datatype, DataType::Integer | DataType::Real),
                "PARAMETER",
                "Range parameters require integer or real datatype",
            )?;
            let min_n = parameter_number(datatype, min)?;
            let max_n = parameter_number(datatype, max)?;
            let step_n = parameter_number(datatype, step)?;
            let current_n = parameter_number(datatype, current)?;
            require(
                min_n <= max_n && current_n >= min_n && current_n <= max_n && step_n > 0.0,
                "PARAMETER",
                "Range parameter requires min <= current <= max and a positive step",
            )?;
            Ok(format!(
                "\n        <range granularity=\"{}\" max=\"{}\" min=\"{}\"/>",
                escape_attr(&parameter_literal(datatype, step)?),
                escape_attr(&parameter_literal(datatype, max)?),
                escape_attr(&parameter_literal(datatype, min)?),
            ))
        }
    }
}

fn parameter_number(datatype: DataType, value: &serde_json::Value) -> Result<f64> {
    match datatype {
        DataType::Integer => value
            .as_i64()
            .map(|value| value as f64)
            .or_else(|| value.as_u64().map(|value| value as f64))
            .ok_or_else(|| Error::new("PARAMETER", "Integer parameter value must be an integer")),
        DataType::Real => value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| Error::new("PARAMETER", "Real parameter value must be finite")),
        _ => Err(Error::new("PARAMETER", "Numeric parameter value expected")),
    }
}

fn parameter_literal(datatype: DataType, value: &serde_json::Value) -> Result<String> {
    match datatype {
        DataType::String => {
            let value = value.as_str().ok_or_else(|| {
                Error::new("PARAMETER", "String parameter value must be a JSON string")
            })?;
            require(
                !value.contains(['"', '\n', '\r', '\0']),
                "PARAMETER",
                "MVP string parameter values cannot contain quotes, newlines, or NUL",
            )?;
            Ok(format!("\"{value}\""))
        }
        DataType::Integer => {
            if let Some(value) = value.as_i64() {
                Ok(value.to_string())
            } else if let Some(value) = value.as_u64() {
                Ok(value.to_string())
            } else {
                Err(Error::new(
                    "PARAMETER",
                    "Integer parameter value must be an integer",
                ))
            }
        }
        DataType::Real => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| value.to_string())
            .ok_or_else(|| Error::new("PARAMETER", "Real parameter value must be finite")),
        DataType::Boolean => value
            .as_bool()
            .map(|value| value.to_string())
            .ok_or_else(|| {
                Error::new("PARAMETER", "Boolean parameter value must be true or false")
            }),
        _ => Err(Error::new(
            "PARAMETER",
            "MVP parameter authoring supports string, integer, real, and boolean datatypes",
        )),
    }
}

fn parameter_name(workbook_hash: &str, caption: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(workbook_hash.as_bytes());
    hash.update(b"\0parameter\0");
    hash.update(caption.as_bytes());
    let digest = format!("{:x}", hash.finalize());
    format!("[Parameter_{}]", &digest[..16])
}

fn worksheet_xml(book: &Workbook, spec: &WorksheetSpec) -> Result<String> {
    require(
        !spec.name.trim().is_empty(),
        "WORKSHEET",
        "Worksheet name cannot be empty",
    )?;
    require(
        book.worksheet_id(&spec.name).is_none(),
        "WORKSHEET_EXISTS",
        format!("Worksheet {:?} already exists", spec.name),
    )?;
    require(
        spec.datasource != "Parameters",
        "WORKSHEET",
        "Parameters datasource cannot be used as a visualization datasource",
    )?;

    let datasource_id = book.datasource_id(&spec.datasource).ok_or_else(|| {
        Error::new(
            "DATASOURCE_NOT_FOUND",
            format!(
                "Datasource {:?} does not exist in this workbook",
                spec.datasource
            ),
        )
    })?;
    let datasource = book
        .datasource(datasource_id)
        .ok_or_else(|| Error::new("INTERNAL", "Datasource id is invalid"))?;
    let measure = resolve_field(book, datasource_id, &spec.measure)?;
    require(
        matches!(measure.datatype, DataType::Integer | DataType::Real),
        "WORKSHEET",
        "MVP worksheet measures must be integer or real fields",
    )?;

    let dimension = spec
        .dimension
        .as_deref()
        .map(|name| resolve_field(book, datasource_id, name))
        .transpose()?;
    let secondary = spec
        .secondary_dimension
        .as_deref()
        .map(|name| resolve_field(book, datasource_id, name))
        .transpose()?;

    match spec.kind {
        WorksheetKind::Bar => require(
            dimension.is_some() && secondary.is_none(),
            "WORKSHEET",
            "Bar worksheet requires dimension and does not accept secondary_dimension",
        )?,
        WorksheetKind::Line => {
            let dimension = dimension.ok_or_else(|| {
                Error::new("WORKSHEET", "Line worksheet requires a date dimension")
            })?;
            require(
                matches!(dimension.datatype, DataType::Date | DataType::Datetime)
                    && secondary.is_none(),
                "WORKSHEET",
                "Line worksheet requires one date/datetime dimension and no secondary_dimension",
            )?;
        }
        WorksheetKind::Kpi => require(
            dimension.is_none() && secondary.is_none(),
            "WORKSHEET",
            "KPI worksheet accepts only a measure",
        )?,
        WorksheetKind::TextTable => require(
            dimension.is_some(),
            "WORKSHEET",
            "Text table requires at least one dimension",
        )?,
    }

    let mut column_nodes = Vec::new();
    if let Some(field) = dimension {
        column_nodes.push(field.node);
    }
    if let Some(field) = secondary {
        if !column_nodes.contains(&field.node) {
            column_nodes.push(field.node);
        }
    }
    if !column_nodes.contains(&measure.node) {
        column_nodes.push(measure.node);
    }
    let mut dependencies = String::new();
    for node in column_nodes {
        dependencies.push_str(&book.xml.source()[book.xml.span(node).range()]);
    }

    let measure_instance = aggregate_instance(measure, spec.aggregation);
    let measure_ref = qualified_instance(&spec.datasource, &measure_instance.name);
    let mut instances = format!(
        "<column-instance column=\"{}\" derivation=\"{}\" name=\"{}\" pivot=\"key\" type=\"quantitative\"/>",
        escape_attr(&measure.name),
        measure_instance.derivation,
        escape_attr(&measure_instance.name),
    );

    let mut dimension_ref = None;
    if let Some(field) = dimension {
        let instance = if spec.kind == WorksheetKind::Line {
            date_month_instance(field)
        } else {
            discrete_instance(field)?
        };
        dimension_ref = Some(qualified_instance(&spec.datasource, &instance.name));
        write!(
            instances,
            "<column-instance column=\"{}\" derivation=\"{}\" name=\"{}\" pivot=\"key\" type=\"{}\"/>",
            escape_attr(&field.name),
            instance.derivation,
            escape_attr(&instance.name),
            instance.field_type,
        )
        .map_err(|_| Error::new("INTERNAL", "Failed to render worksheet dimension"))?;
    }

    let mut secondary_ref = None;
    if let Some(field) = secondary {
        let instance = discrete_instance(field)?;
        secondary_ref = Some(qualified_instance(&spec.datasource, &instance.name));
        write!(
            instances,
            "<column-instance column=\"{}\" derivation=\"{}\" name=\"{}\" pivot=\"key\" type=\"{}\"/>",
            escape_attr(&field.name),
            instance.derivation,
            escape_attr(&instance.name),
            instance.field_type,
        )
        .map_err(|_| Error::new("INTERNAL", "Failed to render worksheet secondary dimension"))?;
    }

    let (rows, cols, mark, encoding) = match spec.kind {
        WorksheetKind::Bar => (
            measure_ref.clone(),
            dimension_ref.unwrap_or_default(),
            "Automatic",
            String::new(),
        ),
        WorksheetKind::Line => (
            measure_ref.clone(),
            dimension_ref.unwrap_or_default(),
            "Automatic",
            String::new(),
        ),
        WorksheetKind::Kpi => (
            String::new(),
            String::new(),
            "Automatic",
            format!(
                "<encodings><text column=\"{}\"/></encodings>",
                escape_attr(&measure_ref)
            ),
        ),
        WorksheetKind::TextTable => (
            dimension_ref.unwrap_or_default(),
            secondary_ref.unwrap_or_default(),
            "Automatic",
            format!(
                "<encodings><text column=\"{}\"/></encodings>",
                escape_attr(&measure_ref)
            ),
        ),
    };

    let caption = datasource.caption.as_deref().unwrap_or(&datasource.name);
    let uuid = deterministic_uuid(&book.xml.sha256, &format!("worksheet:{}", spec.name));
    Ok(format!(
        "\n    <worksheet name=\"{}\">\n      <table>\n        <view>\n          <datasources><datasource caption=\"{}\" name=\"{}\"/></datasources>\n          <datasource-dependencies datasource=\"{}\">{}{}</datasource-dependencies>\n          <aggregation value=\"true\"/>\n        </view>\n        <style/>\n        <panes><pane selection-relaxation-option=\"selection-relaxation-allow\"><view><breakdown value=\"auto\"/></view><mark class=\"{}\"/>{}</pane></panes>\n        <rows>{}</rows><cols>{}</cols>\n      </table>\n      <simple-id uuid=\"{}\"/>\n    </worksheet>",
        escape_attr(&spec.name),
        escape_attr(caption),
        escape_attr(&datasource.name),
        escape_attr(&datasource.name),
        dependencies,
        instances,
        mark,
        encoding,
        escape_text(&rows),
        escape_text(&cols),
        uuid,
    ))
}

struct InstanceSpec {
    name: String,
    derivation: &'static str,
    field_type: &'static str,
}

fn resolve_field<'a>(
    book: &'a Workbook,
    datasource: DatasourceId,
    name: &str,
) -> Result<&'a crate::workbook::Field> {
    let id = book
        .field_id(datasource, name)
        .or_else(|| {
            (!name.starts_with('['))
                .then(|| bracket_name(name))
                .and_then(|name| book.field_id(datasource, &name))
        })
        .ok_or_else(|| {
            Error::new(
                "FIELD_NOT_FOUND",
                format!("Field {name:?} does not exist in the datasource"),
            )
        })?;
    book.field(id)
        .ok_or_else(|| Error::new("INTERNAL", "Field id is invalid"))
}

fn field_token(field: &crate::workbook::Field) -> &str {
    field
        .name
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&field.name)
}

fn aggregate_instance(field: &crate::workbook::Field, aggregation: Aggregation) -> InstanceSpec {
    InstanceSpec {
        name: format!("[{}:{}:qk]", aggregation.prefix(), field_token(field)),
        derivation: aggregation.derivation(),
        field_type: "quantitative",
    }
}

fn discrete_instance(field: &crate::workbook::Field) -> Result<InstanceSpec> {
    require(
        field.field_type != FieldType::Unknown,
        "WORKSHEET",
        "Dimension field type is unknown",
    )?;
    Ok(InstanceSpec {
        name: format!("[none:{}:nk]", field_token(field)),
        derivation: "None",
        field_type: field.field_type.as_str(),
    })
}

fn date_month_instance(field: &crate::workbook::Field) -> InstanceSpec {
    InstanceSpec {
        name: format!("[tmn:{}:qk]", field_token(field)),
        derivation: "Month-Trunc",
        field_type: "quantitative",
    }
}

fn qualified_instance(datasource: &str, instance: &str) -> String {
    format!("{}.{}", bracket_name(datasource), instance)
}

fn worksheet_batch_patch(book: &Workbook, worksheets: String) -> Result<Patch> {
    let root = book.root_child("worksheets").ok_or_else(|| {
        Error::new(
            "WORKBOOK_SHAPE",
            "Worksheet creation requires an existing <worksheets> container",
        )
    })?;
    Ok(Patch {
        span: Span::empty(book.xml.closing_tag_start(root)?)?,
        expected: String::new(),
        replacement: worksheets,
        reason: "create worksheet batch".into(),
    })
}

fn filter_action_xml(book: &Workbook, spec: &FilterActionSpec) -> Result<String> {
    validate_action_source(book, &spec.caption, &spec.dashboard, &spec.source_sheet)?;
    let exclude = format!(
        "<param name=\"exclude\" value=\"{}\"/>",
        escape_attr(&spec.source_sheet)
    );
    Ok(format!(
        "\n    <action caption=\"{}\" name=\"{}\"><activation auto-clear=\"{}\" type=\"{}\"/><source dashboard=\"{}\" type=\"sheet\" worksheet=\"{}\"/><command command=\"tsc:tsl-filter\">{}<param name=\"special-fields\" value=\"all\"/><param name=\"target\" value=\"{}\"/></command></action>",
        escape_attr(&spec.caption),
        action_name(&book.xml.sha256, "filter", &spec.caption),
        spec.auto_clear,
        spec.run_on.as_str(),
        escape_attr(&spec.dashboard),
        escape_attr(&spec.source_sheet),
        exclude,
        escape_attr(&spec.dashboard),
    ))
}

fn parameter_action_xml(book: &Workbook, spec: &ParameterActionSpec) -> Result<(String, String)> {
    validate_action_source(book, &spec.caption, &spec.dashboard, &spec.source_sheet)?;
    let (source, source_datatype) = resolve_worksheet_field_instance(
        book,
        &spec.source_sheet,
        &spec.source_field,
        spec.source_instance.as_deref(),
    )?;
    let parameter_id = book.parameter_id(&spec.target_parameter).ok_or_else(|| {
        Error::new(
            "PARAMETER_NOT_FOUND",
            format!(
                "Parameter {:?} does not exist in this workbook",
                spec.target_parameter
            ),
        )
    })?;
    let parameter = book
        .parameter(parameter_id)
        .ok_or_else(|| Error::new("INTERNAL", "Parameter id is invalid"))?;
    let field = book
        .field(parameter.field)
        .ok_or_else(|| Error::new("INTERNAL", "Parameter field id is invalid"))?;
    require(
        source_datatype == parameter.datatype,
        "ACTION",
        format!(
            "Parameter action source field datatype {:?} does not match target parameter datatype {:?}",
            source_datatype, parameter.datatype
        ),
    )?;
    let target = format!("[Parameters].{}", field.name);

    Ok((
        format!(
            "\n    <edit-parameter-action caption=\"{}\" name=\"{}\"><activation type=\"{}\"/><source dashboard=\"{}\" type=\"sheet\" worksheet=\"{}\"/><agg-type type=\"{}\"/><clear-option type=\"do-nothing\" value=\"s:LROOT:\"/><params><param name=\"source-field\" value=\"{}\"/><param name=\"target-parameter\" value=\"{}\"/></params></edit-parameter-action>",
            escape_attr(&spec.caption),
            action_name(&book.xml.sha256, "parameter", &spec.caption),
            spec.run_on.as_str(),
            escape_attr(&spec.dashboard),
            escape_attr(&spec.source_sheet),
            "attr",
            escape_attr(&source),
            escape_attr(&target),
        ),
        field.name.clone(),
    ))
}

fn validate_action_source(
    book: &Workbook,
    caption: &str,
    dashboard: &str,
    source_sheet: &str,
) -> Result<()> {
    require(
        !caption.trim().is_empty(),
        "ACTION",
        "Action caption cannot be empty",
    )?;
    require(
        !action_caption_exists(book, caption)?,
        "ACTION_EXISTS",
        format!("Action caption {caption:?} already exists"),
    )?;
    let dashboard_id = book.dashboard_id(dashboard).ok_or_else(|| {
        Error::new(
            "DASHBOARD_NOT_FOUND",
            format!("Dashboard {dashboard:?} does not exist in this workbook"),
        )
    })?;
    require(
        book.worksheet_id(source_sheet).is_some(),
        "WORKSHEET_NOT_FOUND",
        format!("Worksheet {source_sheet:?} does not exist in this workbook"),
    )?;
    let dashboard = book
        .dashboards
        .get(dashboard_id.0 as usize)
        .ok_or_else(|| Error::new("INTERNAL", "Dashboard id is invalid"))?;
    let on_dashboard = book.xml.descendants(dashboard.node).any(|node| {
        book.xml.tag(node) == "zone" && book.xml.attr_raw(node, "name") == Some(source_sheet)
    });
    require(
        on_dashboard,
        "ACTION",
        format!("Source worksheet {source_sheet:?} is not placed on dashboard {dashboard:?}"),
    )
}

fn action_caption_exists(book: &Workbook, caption: &str) -> Result<bool> {
    let Some(actions) = book.root_child("actions") else {
        return Ok(false);
    };
    for node in book.xml.children(actions) {
        if matches!(
            book.xml.tag(node),
            "action" | "edit-parameter-action" | "edit-group-action" | "nav-action"
        ) && book
            .xml
            .attr(node, "caption")?
            .is_some_and(|value| value == caption)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn action_name(workbook_hash: &str, kind: &str, caption: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(workbook_hash.as_bytes());
    hash.update(b"\0action\0");
    hash.update(kind.as_bytes());
    hash.update(b"\0");
    hash.update(caption.as_bytes());
    let digest = format!("{:X}", hash.finalize());
    format!("[Action1_{}]", &digest[..32])
}

fn resolve_worksheet_field_instance(
    book: &Workbook,
    worksheet_name: &str,
    field_name: &str,
    requested_instance: Option<&str>,
) -> Result<(String, DataType)> {
    let worksheet_id = book.worksheet_id(worksheet_name).ok_or_else(|| {
        Error::new(
            "WORKSHEET_NOT_FOUND",
            format!("Worksheet {worksheet_name:?} does not exist in this workbook"),
        )
    })?;
    let worksheet = book
        .worksheets
        .get(worksheet_id.0 as usize)
        .ok_or_else(|| Error::new("INTERNAL", "Worksheet id is invalid"))?;

    let mut matches = Vec::new();
    for dependency in book
        .xml
        .descendants(worksheet.node)
        .filter(|node| book.xml.tag(*node) == "datasource-dependencies")
    {
        let Some(datasource_name) = book.xml.attr(dependency, "datasource")? else {
            continue;
        };
        let Some(datasource_id) = book.datasource_id(datasource_name.as_ref()) else {
            continue;
        };
        let Some(field_id) = book.field_id(datasource_id, field_name).or_else(|| {
            (!field_name.starts_with('['))
                .then(|| bracket_name(field_name))
                .and_then(|name| book.field_id(datasource_id, &name))
        }) else {
            continue;
        };
        let field = book
            .field(field_id)
            .ok_or_else(|| Error::new("INTERNAL", "Field id is invalid"))?;
        for child in book.xml.children(dependency) {
            if book.xml.tag(child) != "column-instance"
                || book.xml.attr(child, "column")?.as_deref() != Some(field.name.as_str())
            {
                continue;
            }
            let Some(instance) = book.xml.attr(child, "name")? else {
                continue;
            };
            if requested_instance.is_none_or(|wanted| wanted == instance) {
                matches.push((
                    format!("{}.{}", bracket_name(datasource_name.as_ref()), instance),
                    field.datatype,
                ));
            }
        }
    }

    require(
        !matches.is_empty(),
        "ACTION_SOURCE_FIELD",
        format!(
            "Worksheet {worksheet_name:?} does not expose a column-instance for field {field_name:?}"
        ),
    )?;
    require(
        matches.len() == 1,
        "ACTION_SOURCE_AMBIGUOUS",
        format!(
            "Worksheet {worksheet_name:?} exposes multiple instances for field {field_name:?}; pass source_instance"
        ),
    )?;
    Ok(matches.remove(0))
}

fn action_patches(
    book: &Workbook,
    filter_actions: String,
    parameter_actions: String,
) -> Result<Vec<Patch>> {
    if filter_actions.is_empty() && parameter_actions.is_empty() {
        return Ok(Vec::new());
    }

    if let Some(actions) = book.root_child("actions") {
        let mut patches = Vec::new();
        if !filter_actions.is_empty() {
            let at = book
                .xml
                .children(actions)
                .find(|node| book.xml.tag(*node) != "action")
                .map(|node| book.xml.span(node).start as usize)
                .unwrap_or(book.xml.closing_tag_start(actions)?);
            patches.push(Patch {
                span: Span::empty(at)?,
                expected: String::new(),
                replacement: filter_actions,
                reason: "create filter actions".into(),
            });
        }
        if !parameter_actions.is_empty() {
            patches.push(Patch {
                span: Span::empty(book.xml.closing_tag_start(actions)?)?,
                expected: String::new(),
                replacement: parameter_actions,
                reason: "create parameter actions".into(),
            });
        }
        return Ok(patches);
    }

    let worksheets = book.root_child("worksheets").ok_or_else(|| {
        Error::new(
            "WORKBOOK_SHAPE",
            "Action creation requires an existing <worksheets> container",
        )
    })?;
    Ok(vec![Patch {
        span: Span::empty(book.xml.span(worksheets).start as usize)?,
        expected: String::new(),
        replacement: format!("\n  <actions>{filter_actions}{parameter_actions}\n  </actions>\n  "),
        reason: "create actions container".into(),
    }])
}

fn parameter_dashboard_dependency_patches(
    book: &Workbook,
    requests: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<Patch>> {
    let mut patches = Vec::new();
    require(
        requests.is_empty() || book.datasource_id("Parameters").is_some(),
        "PARAMETER_NOT_FOUND",
        "Parameters datasource does not exist",
    )?;

    for (dashboard_name, parameters) in requests {
        let dashboard_id = book.dashboard_id(dashboard_name).ok_or_else(|| {
            Error::new(
                "DASHBOARD_NOT_FOUND",
                format!("Dashboard {dashboard_name:?} does not exist in this workbook"),
            )
        })?;
        let dashboard = book
            .dashboards
            .get(dashboard_id.0 as usize)
            .ok_or_else(|| Error::new("INTERNAL", "Dashboard id is invalid"))?;
        let zones = book
            .xml
            .direct_child(dashboard.node, "zones")
            .ok_or_else(|| Error::new("WORKBOOK_SHAPE", "Dashboard has no zones container"))?;

        let mut requested_columns = Vec::new();
        for parameter_name in parameters {
            let parameter_id = book.parameter_id(parameter_name).ok_or_else(|| {
                Error::new(
                    "PARAMETER_NOT_FOUND",
                    format!("Parameter {parameter_name:?} does not exist"),
                )
            })?;
            let parameter = book
                .parameter(parameter_id)
                .ok_or_else(|| Error::new("INTERNAL", "Parameter id is invalid"))?;
            let field = book
                .field(parameter.field)
                .ok_or_else(|| Error::new("INTERNAL", "Parameter field id is invalid"))?;
            requested_columns.push((field.name.as_str(), field.node));
        }

        let datasources = book.xml.direct_child(dashboard.node, "datasources");
        let existing_dep = book.xml.children(dashboard.node).find(|node| {
            book.xml.tag(*node) == "datasource-dependencies"
                && book.xml.attr_raw(*node, "datasource") == Some("Parameters")
        });

        let declares_parameters = datasources.is_some_and(|container| {
            book.xml.children(container).any(|node| {
                book.xml.tag(node) == "datasource"
                    && book.xml.attr_raw(node, "name") == Some("Parameters")
            })
        });

        if !declares_parameters {
            if let Some(container) = datasources {
                patches.push(Patch {
                    span: Span::empty(book.xml.closing_tag_start(container)?)?,
                    expected: String::new(),
                    replacement: "<datasource name=\"Parameters\"/>".into(),
                    reason: format!("declare Parameters datasource on dashboard {dashboard_name}"),
                });
            } else if let Some(dependency) = existing_dep {
                patches.push(Patch {
                    span: Span::empty(book.xml.span(dependency).start as usize)?,
                    expected: String::new(),
                    replacement: "<datasources><datasource name=\"Parameters\"/></datasources>"
                        .into(),
                    reason: format!("declare Parameters datasource on dashboard {dashboard_name}"),
                });
            }
        }

        if let Some(dependency) = existing_dep {
            let existing_names = book
                .xml
                .children(dependency)
                .filter(|node| book.xml.tag(*node) == "column")
                .filter_map(|node| book.xml.attr_raw(node, "name"))
                .collect::<HashSet<_>>();
            let mut missing = String::new();
            for (name, node) in requested_columns {
                if !existing_names.contains(name) {
                    missing.push_str(&book.xml.source()[book.xml.span(node).range()]);
                }
            }
            if !missing.is_empty() {
                patches.push(Patch {
                    span: Span::empty(book.xml.closing_tag_start(dependency)?)?,
                    expected: String::new(),
                    replacement: missing,
                    reason: format!("add parameter dependencies to dashboard {dashboard_name}"),
                });
            }
            continue;
        }

        let mut columns = String::new();
        for (_, node) in requested_columns {
            columns.push_str(&book.xml.source()[book.xml.span(node).range()]);
        }
        let dependency_xml = format!(
            "<datasource-dependencies datasource=\"Parameters\">{columns}</datasource-dependencies>"
        );
        let replacement = if datasources.is_none() {
            format!("<datasources><datasource name=\"Parameters\"/></datasources>{dependency_xml}")
        } else {
            dependency_xml
        };
        patches.push(Patch {
            span: Span::empty(book.xml.span(zones).start as usize)?,
            expected: String::new(),
            replacement,
            reason: format!("add parameter dependencies to dashboard {dashboard_name}"),
        });
    }
    Ok(patches)
}

impl Aggregation {
    fn derivation(self) -> &'static str {
        match self {
            Self::Sum => "Sum",
            Self::Average => "Average",
            Self::Min => "Min",
            Self::Max => "Max",
            Self::Count => "Count",
            Self::CountDistinct => "CountD",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Average => "avg",
            Self::Min => "min",
            Self::Max => "max",
            Self::Count => "cnt",
            Self::CountDistinct => "ctd",
        }
    }
}

impl ActionTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Select => "on-select",
            Self::Hover => "on-hover",
        }
    }
}

#[derive(Clone)]
struct FilterBinding {
    source_sheet: String,
    requested_field: String,
    datasource: String,
    qualified: String,
    column_node: NodeId,
    instance_node: NodeId,
    mode: Option<&'static str>,
    values: Option<&'static str>,
    title: Option<String>,
}

struct FilterDependencyIndex<'a> {
    datasource: Cow<'a, str>,
    datasource_id: DatasourceId,
    columns: HashMap<Cow<'a, str>, NodeId>,
    instances: HashMap<Cow<'a, str>, Vec<NodeId>>,
}

struct WorksheetFilterIndex<'a> {
    dependencies: Vec<FilterDependencyIndex<'a>>,
    filter_columns: HashSet<Cow<'a, str>>,
    filter_instances: HashSet<String>,
}

struct FilterRequest<'a> {
    source_sheet: &'a str,
    field: &'a str,
    instance: Option<&'a str>,
    control: FilterControl,
    values: Option<FilterValues>,
    title: Option<&'a str>,
}

struct DashboardParts {
    dashboard: String,
    window: String,
    other_patches: Vec<Patch>,
}

fn dashboard_parts<'a>(
    book: &'a Workbook,
    spec: &DashboardSpec,
    next_id: &mut u32,
    filter_indexes: &mut HashMap<WorksheetId, WorksheetFilterIndex<'a>>,
) -> Result<DashboardParts> {
    require(
        !spec.name.trim().is_empty(),
        "DASHBOARD",
        "Dashboard name cannot be empty",
    )?;
    require(
        book.dashboard_id(&spec.name).is_none(),
        "DASHBOARD_EXISTS",
        "Dashboard name already exists",
    )?;
    require(
        spec.width > 0 && spec.height > 0,
        "DASHBOARD",
        "Dashboard width and height must be positive",
    )?;
    validate_color(spec.background.as_deref())?;
    validate_layout(book, &spec.layout)?;
    for view_style in &spec.view_styles {
        validate_view_style(book, view_style)?;
    }

    let filters = collect_filter_bindings(book, &spec.layout, filter_indexes)?;
    let padding_x = pixels_to_norm(spec.padding, spec.width);
    let padding_y = pixels_to_norm(spec.padding, spec.height);
    require(
        padding_x.saturating_mul(2) < CANVAS && padding_y.saturating_mul(2) < CANVAS,
        "LAYOUT",
        "Dashboard padding consumes the entire canvas",
    )?;

    let root_id = take_id(next_id)?;
    let mut renderer = ZoneRenderer {
        dashboard_width: spec.width,
        dashboard_height: spec.height,
        filters: &filters,
        filter_cursor: 0,
        next_id: *next_id,
    };
    let content = renderer.zone(
        &spec.layout,
        Rect {
            x: padding_x,
            y: padding_y,
            w: CANVAS - padding_x * 2,
            h: CANVAS - padding_y * 2,
        },
        &ZoneStyle::default(),
        None,
    )?;
    require(
        renderer.filter_cursor == filters.len(),
        "INTERNAL",
        "Dashboard filter bindings were not consumed exactly once",
    )?;
    *next_id = renderer.next_id;

    let uuid = deterministic_uuid(&book.xml.sha256, &spec.name);
    let dependencies = dashboard_dependencies(book, &filters);
    let root_style = root_style(spec.background.as_deref());
    let dashboard = format!(
        "\n    <dashboard enable-sort-zone-taborder=\"true\" name=\"{}\">\n      <style/>\n      <size maxheight=\"{}\" maxwidth=\"{}\" minheight=\"{}\" minwidth=\"{}\" sizing-mode=\"fixed\"/>{}\n      <zones>\n        <zone h=\"{CANVAS}\" id=\"{}\" type-v2=\"layout-basic\" w=\"{CANVAS}\" x=\"0\" y=\"0\">\n{}{}\n        </zone>\n      </zones>\n      <simple-id uuid=\"{}\"/>\n    </dashboard>",
        escape_attr(&spec.name),
        spec.height,
        spec.width,
        spec.height,
        spec.width,
        dependencies,
        root_id,
        indent(&content, 10),
        root_style,
        uuid,
    );

    let window = dashboard_window_xml(book, spec)?;
    let mut other_patches = Vec::new();
    for view_style in &spec.view_styles {
        other_patches.extend(view_style_patches(book, view_style)?);
    }
    Ok(DashboardParts {
        dashboard,
        window,
        other_patches,
    })
}

fn next_dashboard_zone_id(book: &Workbook) -> Result<u32> {
    let mut max_id = 9u32;
    for dashboard in &book.dashboards {
        for node in book.xml.descendants(dashboard.node) {
            if book.xml.tag(node) != "zone" {
                continue;
            }
            if let Some(value) = book.xml.attr_raw(node, "id")
                && let Ok(id) = value.parse::<u32>()
            {
                max_id = max_id.max(id);
            }
        }
    }
    max_id
        .checked_add(1)
        .ok_or_else(|| Error::new("LAYOUT", "Zone id overflow"))
}

fn dashboard_window_xml(book: &Workbook, spec: &DashboardSpec) -> Result<String> {
    let mut sheets = Vec::<(&str, Option<FitMode>)>::new();
    collect_sheet_viewpoints(&spec.layout, &mut sheets);
    sheets.sort_by(|a, b| a.0.cmp(b.0));
    sheets.dedup_by(|a, b| a.0 == b.0);

    let mut viewpoints = String::new();
    for (sheet, fit) in sheets {
        let zoom = match fit.unwrap_or(FitMode::EntireView) {
            FitMode::Standard => "",
            FitMode::EntireView => "<zoom type=\"entire-view\"/>",
            FitMode::FitWidth => "<zoom type=\"fit-width\"/>",
            FitMode::FitHeight => "<zoom type=\"fit-height\"/>",
        };
        write!(
            viewpoints,
            "<viewpoint name=\"{}\">{}</viewpoint>",
            escape_attr(sheet),
            zoom
        )
        .map_err(|_| Error::new("INTERNAL", "Failed to render dashboard viewpoint"))?;
    }

    let uuid = deterministic_uuid(&book.xml.sha256, &format!("window:{}", spec.name));
    Ok(format!(
        "\n    <window class=\"dashboard\" name=\"{}\"><viewpoints>{}</viewpoints><active id=\"-1\"/><simple-id uuid=\"{}\"/></window>",
        escape_attr(&spec.name),
        viewpoints,
        uuid
    ))
}

fn dashboard_batch_patch(book: &Workbook, dashboards: String) -> Result<Patch> {
    let at = dashboard_insert_at(book)?;
    let replacement = if book.root_child("dashboards").is_some() {
        dashboards
    } else {
        format!("\n  <dashboards>{dashboards}\n  </dashboards>")
    };
    Ok(Patch {
        span: Span::empty(at)?,
        expected: String::new(),
        replacement,
        reason: "create dashboard batch".into(),
    })
}

fn dashboard_window_batch_patch(book: &Workbook, windows: String) -> Result<Patch> {
    let at = window_insert_at(book)?;
    let replacement = if book.root_child("windows").is_some() {
        windows
    } else {
        format!("\n  <windows>{windows}\n  </windows>")
    };
    Ok(Patch {
        span: Span::empty(at)?,
        expected: String::new(),
        replacement,
        reason: "register dashboard window batch".into(),
    })
}

fn collect_sheet_viewpoints<'a>(layout: &'a Layout, out: &mut Vec<(&'a str, Option<FitMode>)>) {
    match layout {
        Layout::Row { children, .. } | Layout::Column { children, .. } => {
            for child in children {
                collect_sheet_viewpoints(&child.layout, out);
            }
        }
        Layout::Sheet { sheet, fit, .. } => out.push((sheet, *fit)),
        Layout::Filter { source_sheet, .. } => out.push((source_sheet, None)),
        Layout::Text { .. } | Layout::Spacer => {}
    }
}

fn window_insert_at(book: &Workbook) -> Result<usize> {
    if let Some(windows) = book.root_child("windows") {
        return book.xml.closing_tag_start(windows);
    }
    if let Some(dashboards) = book.root_child("dashboards") {
        return Ok(book.xml.span(dashboards).end as usize);
    }
    let root = book.xml.root();
    for child in book.xml.children(root) {
        if matches!(
            book.xml.tag(child),
            "datagraph"
                | "thumbnails"
                | "shapes"
                | "referenced-extensions"
                | "explain-data"
                | "data-orientation"
                | "tab-agent-config"
                | "accelerator-details"
                | "workbook-optimizer"
        ) {
            return Ok(book.xml.span(child).start as usize);
        }
    }
    book.xml.closing_tag_start(root)
}

fn collect_filter_bindings<'a>(
    book: &'a Workbook,
    layout: &Layout,
    indexes: &mut HashMap<WorksheetId, WorksheetFilterIndex<'a>>,
) -> Result<Vec<FilterBinding>> {
    let mut bindings = Vec::new();
    collect_filters(book, layout, indexes, &mut bindings)?;
    Ok(bindings)
}

fn collect_filters<'a>(
    book: &'a Workbook,
    layout: &Layout,
    indexes: &mut HashMap<WorksheetId, WorksheetFilterIndex<'a>>,
    out: &mut Vec<FilterBinding>,
) -> Result<()> {
    match layout {
        Layout::Row { children, .. } | Layout::Column { children, .. } => {
            for child in children {
                collect_filters(book, &child.layout, indexes, out)?;
            }
        }
        Layout::Filter {
            source_sheet,
            field,
            instance,
            control,
            values,
            title,
        } => out.push(resolve_filter_binding(
            book,
            indexes,
            FilterRequest {
                source_sheet,
                field,
                instance: instance.as_deref(),
                control: *control,
                values: *values,
                title: title.as_deref(),
            },
        )?),
        Layout::Sheet { .. } | Layout::Text { .. } | Layout::Spacer => {}
    }
    Ok(())
}

fn build_worksheet_filter_index<'a>(
    book: &'a Workbook,
    worksheet_id: WorksheetId,
) -> Result<WorksheetFilterIndex<'a>> {
    let worksheet = book
        .worksheets
        .get(worksheet_id.0 as usize)
        .ok_or_else(|| Error::new("INTERNAL", "Worksheet id is invalid"))?;

    let mut dependency_nodes = Vec::new();
    let mut filter_columns = HashSet::new();
    let mut filter_instances = HashSet::new();

    for node in book.xml.descendants(worksheet.node) {
        match book.xml.tag(node) {
            "datasource-dependencies" => dependency_nodes.push(node),
            "filter" => {
                if let Some(column) = book.xml.attr(node, "column")? {
                    filter_instances.insert(filter_instance_suffix(column.as_ref()).to_owned());
                    filter_columns.insert(column);
                }
            }
            _ => {}
        }
    }

    let mut dependencies = Vec::with_capacity(dependency_nodes.len());
    for dependency in dependency_nodes {
        let Some(datasource) = book.xml.attr(dependency, "datasource")? else {
            continue;
        };
        let Some(datasource_id) = book.datasource_id(datasource.as_ref()) else {
            continue;
        };

        let mut columns = HashMap::new();
        let mut instances = HashMap::<Cow<'a, str>, Vec<NodeId>>::new();
        for child in book.xml.children(dependency) {
            match book.xml.tag(child) {
                "column" => {
                    if let Some(name) = book.xml.attr(child, "name")? {
                        columns.insert(name, child);
                    }
                }
                "column-instance" => {
                    if let Some(column) = book.xml.attr(child, "column")? {
                        instances.entry(column).or_default().push(child);
                    }
                }
                _ => {}
            }
        }

        dependencies.push(FilterDependencyIndex {
            datasource,
            datasource_id,
            columns,
            instances,
        });
    }

    Ok(WorksheetFilterIndex {
        dependencies,
        filter_columns,
        filter_instances,
    })
}

fn filter_instance_suffix(column: &str) -> &str {
    column
        .rfind("].[")
        .map(|at| &column[at + 2..])
        .unwrap_or(column)
}

fn resolve_filter_binding<'a>(
    book: &'a Workbook,
    indexes: &mut HashMap<WorksheetId, WorksheetFilterIndex<'a>>,
    request: FilterRequest<'_>,
) -> Result<FilterBinding> {
    let FilterRequest {
        source_sheet,
        field,
        instance: requested_instance,
        control,
        values,
        title,
    } = request;
    let worksheet_id = book.worksheet_id(source_sheet).ok_or_else(|| {
        Error::new(
            "WORKSHEET_NOT_FOUND",
            format!("Worksheet {source_sheet:?} does not exist in this workbook"),
        )
    })?;

    let index = match indexes.entry(worksheet_id) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(build_worksheet_filter_index(book, worksheet_id)?)
        }
    };

    for dependency in &index.dependencies {
        let field_id = book.field_id(dependency.datasource_id, field).or_else(|| {
            (!field.starts_with('['))
                .then(|| bracket_name(field))
                .and_then(|name| book.field_id(dependency.datasource_id, &name))
        });
        let Some(field_id) = field_id else {
            continue;
        };
        let field_meta = book
            .field(field_id)
            .ok_or_else(|| Error::new("INTERNAL", "Filter field id is invalid"))?;

        let mut matches = dependency
            .instances
            .get(field_meta.name.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|node| {
                requested_instance.is_none()
                    || book.xml.attr_raw(*node, "name") == requested_instance
            });
        let instance_node = matches.next().ok_or_else(|| {
            Error::new(
                "FILTER_INSTANCE_NOT_FOUND",
                format!(
                    "Worksheet {source_sheet:?} does not expose a column-instance for field {field:?}; pass an exact instance or add the field/filter to the worksheet first"
                ),
            )
        })?;
        require(
            matches.next().is_none(),
            "FILTER_INSTANCE_AMBIGUOUS",
            format!(
                "Worksheet {source_sheet:?} exposes multiple column-instances for field {field:?}; pass the exact instance"
            ),
        )?;

        let instance_name = book
            .xml
            .attr(instance_node, "name")?
            .ok_or_else(|| Error::new("WORKBOOK_SHAPE", "column-instance lacks name"))?;
        let qualified = format!(
            "{}.{}",
            bracket_name(dependency.datasource.as_ref()),
            instance_name
        );

        require(
            index.filter_columns.contains(qualified.as_str())
                || index.filter_instances.contains(instance_name.as_ref()),
            "FILTER_NOT_ON_SHEET",
            format!(
                "Worksheet {source_sheet:?} does not have {field:?} on the Filters shelf; dashboard filter controls only expose existing worksheet filters"
            ),
        )?;

        let column_node = dependency
            .columns
            .get(field_meta.name.as_str())
            .copied()
            .ok_or_else(|| {
                Error::new(
                    "FILTER_FIELD_NOT_FOUND",
                    format!("Datasource dependency does not declare field {field:?}"),
                )
            })?;
        return Ok(FilterBinding {
            source_sheet: source_sheet.to_owned(),
            requested_field: field.to_owned(),
            datasource: dependency.datasource.to_string(),
            qualified,
            column_node,
            instance_node,
            mode: control.mode(),
            values: values.map(FilterValues::as_str),
            title: title.map(str::to_owned),
        });
    }

    Err(Error::new(
        "FILTER_FIELD_NOT_FOUND",
        format!("Cannot resolve filter field {field:?} in worksheet {source_sheet:?}"),
    ))
}

fn dashboard_dependencies(book: &Workbook, filters: &[FilterBinding]) -> String {
    use std::collections::BTreeMap;

    let mut grouped: BTreeMap<&str, Vec<&FilterBinding>> = BTreeMap::new();
    for filter in filters {
        grouped.entry(&filter.datasource).or_default().push(filter);
    }

    let mut xml = String::new();
    for (datasource, bindings) in grouped {
        let mut seen_columns = HashSet::<NodeId>::new();
        let mut seen_instances = HashSet::<NodeId>::new();
        let mut children = String::new();
        for binding in bindings {
            if seen_columns.insert(binding.column_node) {
                children.push_str(&book.xml.source()[book.xml.span(binding.column_node).range()]);
            }
            if seen_instances.insert(binding.instance_node) {
                children.push_str(&book.xml.source()[book.xml.span(binding.instance_node).range()]);
            }
        }
        xml.push_str(&format!(
            "\n      <datasource-dependencies datasource=\"{}\">{}</datasource-dependencies>",
            escape_attr(datasource),
            children
        ));
    }
    xml
}

fn bracket_name(value: &str) -> String {
    format!("[{}]", value.replace(']', "]]"))
}

impl FilterControl {
    fn mode(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::SingleDropdown => Some("dropdown"),
            Self::MultipleDropdown => Some("checkdropdown"),
            Self::SingleList => Some("radiolist"),
            Self::MultipleList => Some("checklist"),
            Self::Slider => Some("slider"),
        }
    }
}

impl FilterValues {
    fn as_str(self) -> &'static str {
        match self {
            Self::AllDatabase => "database",
            Self::Relevant => "relevant",
        }
    }
}

fn validate_view_style(book: &Workbook, style: &ViewStyleSpec) -> Result<()> {
    require(
        book.worksheet_id(&style.sheet).is_some(),
        "WORKSHEET_NOT_FOUND",
        format!("Worksheet {:?} does not exist", style.sheet),
    )?;
    if let Some(size) = style.font_size {
        require((6..=96).contains(&size), "STYLE", "Font size must be 6..96")?;
    }
    validate_color(style.color.as_deref())?;
    for line in [&style.axis, &style.grid_lines, &style.zero_line]
        .into_iter()
        .flatten()
    {
        validate_color(line.color.as_deref())?;
        if let Some(width) = line.width {
            require(width <= 10, "STYLE", "Line width must be <= 10")?;
        }
    }
    if let Some(width) = style.line_width {
        require(
            (0.0..=4.0).contains(&width),
            "STYLE",
            "Tableau mark line width must be 0..4",
        )?;
    }
    Ok(())
}

fn view_style_patches(book: &Workbook, style: &ViewStyleSpec) -> Result<Vec<Patch>> {
    let worksheet_id = book.worksheet_id(&style.sheet).ok_or_else(|| {
        Error::new(
            "WORKSHEET_NOT_FOUND",
            format!(
                "Worksheet {:?} does not exist in this workbook",
                style.sheet
            ),
        )
    })?;
    let worksheet = &book.worksheets[worksheet_id.0 as usize];
    let mut patches = Vec::new();

    let mut table_rules = Vec::new();
    let mut worksheet_formats = Vec::new();
    if let Some(font) = &style.font_family {
        worksheet_formats.push(("font-family", font.clone()));
    }
    if let Some(size) = style.font_size {
        worksheet_formats.push(("font-size", size.to_string()));
    }
    if let Some(color) = &style.color {
        worksheet_formats.push(("color", color.clone()));
    }
    if !worksheet_formats.is_empty() {
        table_rules.push(render_style_rule("worksheet", &worksheet_formats));
    }
    if let Some(line) = &style.axis {
        table_rules.push(render_line_rule("axis", line));
    }
    if let Some(line) = &style.grid_lines {
        table_rules.push(render_line_rule("gridline", line));
    }
    if let Some(line) = &style.zero_line {
        table_rules.push(render_line_rule("zeroline", line));
    }
    if !table_rules.is_empty() {
        patches.push(style_insert_patch(
            book,
            worksheet.node,
            &table_rules.join(""),
            "worksheet table style",
        )?);
    }

    if style.line_width.is_some() || style.show_labels.is_some() {
        for pane in book
            .xml
            .descendants(worksheet.node)
            .filter(|node| book.xml.tag(*node) == "pane")
        {
            let mut formats = Vec::<(&str, String)>::new();
            if let Some(width) = style.line_width {
                formats.push(("size", width.to_string()));
            }
            if let Some(show) = style.show_labels {
                formats.push(("mark-labels-show", show.to_string()));
            }
            patches.push(style_insert_patch(
                book,
                pane,
                &render_style_rule("mark", &formats),
                "pane mark style",
            )?);
        }
    }
    Ok(patches)
}

fn render_line_rule(element: &str, line: &LineStyle) -> String {
    let mut formats = vec![(
        "line-visibility",
        if line.visible { "on" } else { "off" }.to_owned(),
    )];
    if let Some(color) = &line.color {
        formats.push(("stroke-color", color.clone()));
    }
    if let Some(width) = line.width {
        formats.push(("stroke-size", width.to_string()));
    }
    render_style_rule(element, &formats)
}

fn render_style_rule(element: &str, formats: &[(&str, String)]) -> String {
    let formats = formats
        .iter()
        .map(|(attr, value)| {
            format!(
                "<format attr=\"{}\" value=\"{}\"/>",
                escape_attr(attr),
                escape_attr(value)
            )
        })
        .collect::<String>();
    format!("<style-rule element=\"{element}\">{formats}</style-rule>")
}

fn style_insert_patch(
    book: &Workbook,
    owner: crate::xml::NodeId,
    rules: &str,
    reason: &str,
) -> Result<Patch> {
    let style_owner = if book.xml.tag(owner) == "worksheet" {
        book.xml
            .direct_child(owner, "table")
            .ok_or_else(|| Error::new("WORKBOOK_SHAPE", "Worksheet has no table"))?
    } else {
        owner
    };

    if let Some(style) = book.xml.direct_child(style_owner, "style") {
        if book.xml.span(style).range().is_empty() {
            return Err(Error::new("WORKBOOK_SHAPE", "Invalid style span"));
        }
        let source = &book.xml.source()[book.xml.span(style).range()];
        if source.trim_end().ends_with("/>") {
            return Ok(Patch {
                span: book.xml.span(style),
                expected: source.to_owned(),
                replacement: format!("<style>{rules}</style>"),
                reason: reason.to_owned(),
            });
        }
        let at = book.xml.closing_tag_start(style)?;
        return Ok(Patch {
            span: Span::empty(at)?,
            expected: String::new(),
            replacement: rules.to_owned(),
            reason: reason.to_owned(),
        });
    }

    let insert_at = if book.xml.tag(style_owner) == "pane" {
        book.xml.closing_tag_start(style_owner)?
    } else if let Some(view) = book.xml.direct_child(style_owner, "view") {
        book.xml.span(view).end as usize
    } else {
        book.xml.closing_tag_start(style_owner)?
    };
    Ok(Patch {
        span: Span::empty(insert_at)?,
        expected: String::new(),
        replacement: format!("<style>{rules}</style>"),
        reason: reason.to_owned(),
    })
}

fn datasource_column_insert_at(book: &Workbook, datasource: crate::xml::NodeId) -> Result<usize> {
    let after_columns = [
        "column-instance",
        "group",
        "mapped-images",
        "drill-paths",
        "folders-common",
        "folders-parameters",
        "actions",
        "calculated-members",
        "extract",
        "layout",
        "style",
        "semantic-values",
        "date-options",
        "default-date-format",
        "default-sorts",
        "field-sort-info",
        "datasource-dependencies",
        "explainability",
        "filter",
        "analytic-model",
        "object-graph",
        "default-calendar-type",
        "datasource-tree",
    ];

    for child in book.xml.children(datasource) {
        if after_columns.contains(&book.xml.tag(child)) {
            return Ok(book.xml.span(child).start as usize);
        }
    }

    book.xml.closing_tag_start(datasource)
}

fn dashboard_insert_at(book: &Workbook) -> Result<usize> {
    if let Some(dashboards) = book.root_child("dashboards") {
        return book.xml.closing_tag_start(dashboards);
    }

    let later = [
        "windows",
        "datagraph",
        "thumbnails",
        "shapes",
        "referenced-extensions",
        "explain-data",
        "data-orientation",
        "tab-agent-config",
        "accelerator-details",
        "workbook-optimizer",
    ];
    let root = book.xml.root();
    for child in book.xml.children(root) {
        if later.contains(&book.xml.tag(child)) {
            return Ok(book.xml.span(child).start as usize);
        }
    }
    book.xml.closing_tag_start(root)
}

fn validate_layout(book: &Workbook, layout: &Layout) -> Result<()> {
    match layout {
        Layout::Row {
            children, padding, ..
        }
        | Layout::Column {
            children, padding, ..
        } => {
            require(!children.is_empty(), "LAYOUT", "Container cannot be empty")?;
            require(
                *padding <= 500,
                "LAYOUT",
                "Container padding must be <= 500 px",
            )?;
            for child in children {
                require(child.weight > 0, "LAYOUT", "Child weight must be positive")?;
                if let Some(size) = child.fixed_size {
                    require(size > 0, "LAYOUT", "Fixed size must be positive")?;
                }
                validate_style(&child.style)?;
                validate_layout(book, &child.layout)?;
            }
        }
        Layout::Sheet { sheet, .. } => require(
            book.worksheet_id(sheet).is_some(),
            "WORKSHEET_NOT_FOUND",
            format!("Worksheet {sheet:?} does not exist"),
        )?,
        Layout::Filter {
            source_sheet,
            field,
            ..
        } => {
            require(
                book.worksheet_id(source_sheet).is_some(),
                "WORKSHEET_NOT_FOUND",
                format!("Worksheet {source_sheet:?} does not exist"),
            )?;
            require(
                !field.trim().is_empty(),
                "FILTER",
                "Filter field cannot be empty",
            )?;
        }
        Layout::Text {
            text,
            font_family,
            font_size,
            color,
            ..
        } => {
            require(!text.trim().is_empty(), "LAYOUT", "Text cannot be empty")?;
            if let Some(font) = font_family {
                require(
                    !font.trim().is_empty(),
                    "STYLE",
                    "Font family cannot be empty",
                )?;
            }
            if let Some(size) = font_size {
                require(
                    (6..=96).contains(size),
                    "LAYOUT",
                    "Text font size must be 6..96",
                )?;
            }
            validate_color(color.as_deref())?;
        }
        Layout::Spacer => {}
    }
    Ok(())
}

fn validate_style(style: &ZoneStyle) -> Result<()> {
    validate_color(style.background.as_deref())?;
    validate_color(style.border_color.as_deref())?;
    require(
        style.border_width <= 20,
        "STYLE",
        "Border width must be <= 20",
    )?;
    require(
        style.corner_radius <= 100,
        "STYLE",
        "Corner radius must be <= 100",
    )?;
    require(style.padding <= 500, "STYLE", "Zone padding must be <= 500")?;
    require(style.margin <= 500, "STYLE", "Zone margin must be <= 500")
}

fn validate_color(color: Option<&str>) -> Result<()> {
    if let Some(color) = color {
        require(
            color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit()),
            "STYLE",
            "Colors must use #RRGGBB",
        )?;
    }
    Ok(())
}

impl ZoneRenderer<'_> {
    fn zone(
        &mut self,
        layout: &Layout,
        rect: Rect,
        style: &ZoneStyle,
        fixed_size: Option<u32>,
    ) -> Result<String> {
        let fixed = fixed_zone_attrs(fixed_size);
        match layout {
            Layout::Sheet {
                sheet, show_title, ..
            } => {
                let id = self.take_id()?;
                let title = show_title
                    .map(|show| format!(" show-title=\"{show}\""))
                    .unwrap_or_default();
                Ok(format!(
                    "<zone{fixed} h=\"{}\" id=\"{id}\" name=\"{}\"{title} w=\"{}\" x=\"{}\" y=\"{}\">{}</zone>",
                    rect.h,
                    escape_attr(sheet),
                    rect.w,
                    rect.x,
                    rect.y,
                    zone_style(style),
                ))
            }
            Layout::Filter {
                source_sheet,
                field,
                ..
            } => {
                let id = self.take_id()?;
                let binding = self
                    .filters
                    .get(self.filter_cursor)
                    .ok_or_else(|| Error::new("FILTER", "Resolved filter binding is missing"))?;
                self.filter_cursor += 1;
                require(
                    binding.source_sheet == *source_sheet && binding.requested_field == *field,
                    "INTERNAL",
                    "Dashboard filter binding order diverged from layout traversal",
                )?;
                let mode = binding
                    .mode
                    .map(|mode| format!(" mode=\"{}\"", escape_attr(mode)))
                    .unwrap_or_default();
                let values = binding
                    .values
                    .map(|values| format!(" values=\"{}\"", escape_attr(values)))
                    .unwrap_or_default();
                let title = binding
                .title
                .as_ref()
                .map(|title| {
                    format!(
                        " custom-title=\"true\"><formatted-text><run>{}</run></formatted-text>{}",
                        escape_text(title),
                        zone_style(style)
                    )
                })
                .unwrap_or_else(|| format!(">{}", zone_style(style)));
                Ok(format!(
                    "<zone{fixed} h=\"{}\" id=\"{id}\"{mode} name=\"{}\" param=\"{}\" type-v2=\"filter\"{values} w=\"{}\" x=\"{}\" y=\"{}\"{title}</zone>",
                    rect.h,
                    escape_attr(source_sheet),
                    escape_attr(&binding.qualified),
                    rect.w,
                    rect.x,
                    rect.y,
                ))
            }
            Layout::Text {
                text,
                font_family,
                font_size,
                bold,
                italic,
                color,
                align,
            } => {
                let id = self.take_id()?;
                let size = font_size.unwrap_or(12);
                let alignment = match align.unwrap_or(TextAlign::Left) {
                    TextAlign::Left => "0",
                    TextAlign::Center => "1",
                    TextAlign::Right => "2",
                };
                let font_color = color
                    .as_ref()
                    .map(|value| format!(" fontcolor=\"{}\"", escape_attr(value)))
                    .unwrap_or_default();
                let font_name = font_family
                    .as_ref()
                    .map(|value| format!(" fontname=\"{}\"", escape_attr(value)))
                    .unwrap_or_default();
                Ok(format!(
                    "<zone{fixed} h=\"{}\" id=\"{id}\" type-v2=\"text\" w=\"{}\" x=\"{}\" y=\"{}\">\n  <formatted-text><run bold=\"{}\" italic=\"{}\" fontalignment=\"{alignment}\"{font_color}{font_name} fontsize=\"{size}\">{}</run></formatted-text>{}\n</zone>",
                    rect.h,
                    rect.w,
                    rect.x,
                    rect.y,
                    bold,
                    italic,
                    escape_text(text),
                    zone_style(style),
                ))
            }
            Layout::Spacer => {
                let id = self.take_id()?;
                Ok(format!(
                    "<zone{fixed} h=\"{}\" id=\"{id}\" type-v2=\"empty\" w=\"{}\" x=\"{}\" y=\"{}\">{}</zone>",
                    rect.h,
                    rect.w,
                    rect.x,
                    rect.y,
                    zone_style(style),
                ))
            }
            Layout::Row {
                gap,
                padding,
                children,
            } => self.container(
                children,
                Flow {
                    gap: *gap,
                    padding: *padding,
                    horizontal: true,
                },
                rect,
                style,
                fixed_size,
            ),
            Layout::Column {
                gap,
                padding,
                children,
            } => self.container(
                children,
                Flow {
                    gap: *gap,
                    padding: *padding,
                    horizontal: false,
                },
                rect,
                style,
                fixed_size,
            ),
        }
    }

    fn container(
        &mut self,
        children: &[Item],
        flow: Flow,
        rect: Rect,
        style: &ZoneStyle,
        fixed_size: Option<u32>,
    ) -> Result<String> {
        let id = self.take_id()?;
        let pad_x = pixels_to_norm(flow.padding, self.dashboard_width);
        let pad_y = pixels_to_norm(flow.padding, self.dashboard_height);
        require(
            pad_x.saturating_mul(2) < rect.w && pad_y.saturating_mul(2) < rect.h,
            "LAYOUT",
            "Container padding consumes all available space",
        )?;
        let inner = Rect {
            x: rect.x + pad_x,
            y: rect.y + pad_y,
            w: rect.w - pad_x * 2,
            h: rect.h - pad_y * 2,
        };

        let gap = if flow.horizontal {
            pixels_to_norm(flow.gap, self.dashboard_width)
        } else {
            pixels_to_norm(flow.gap, self.dashboard_height)
        };
        let total_gap = gap.saturating_mul(children.len().saturating_sub(1) as u32);
        let axis = if flow.horizontal { inner.w } else { inner.h };
        require(
            total_gap < axis,
            "LAYOUT",
            "Container gaps consume all available space",
        )?;
        let available = axis - total_gap;

        let fixed_total = children.iter().try_fold(0u32, |total, child| {
            let size = match child.fixed_size {
                Some(pixels) if flow.horizontal => pixels_to_norm(pixels, self.dashboard_width),
                Some(pixels) => pixels_to_norm(pixels, self.dashboard_height),
                None => 0,
            };
            total
                .checked_add(size)
                .ok_or_else(|| Error::new("LAYOUT", "Fixed sizes overflow"))
        })?;
        require(
            fixed_total <= available,
            "LAYOUT",
            "Fixed child sizes exceed container space",
        )?;
        let flexible_available = available - fixed_total;
        let flex_weight: u64 = children
            .iter()
            .filter(|child| child.fixed_size.is_none())
            .map(|child| u64::from(child.weight))
            .sum();

        let mut used_axis = 0u32;
        let mut flexible_used = 0u32;
        let flexible_count = children
            .iter()
            .filter(|child| child.fixed_size.is_none())
            .count();
        let mut flexible_index = 0usize;
        let mut parts = Vec::with_capacity(children.len());

        for (index, child) in children.iter().enumerate() {
            let size = if let Some(pixels) = child.fixed_size {
                if flow.horizontal {
                    pixels_to_norm(pixels, self.dashboard_width)
                } else {
                    pixels_to_norm(pixels, self.dashboard_height)
                }
            } else {
                flexible_index += 1;
                if flexible_index == flexible_count {
                    flexible_available - flexible_used
                } else {
                    let size = ((u64::from(flexible_available) * u64::from(child.weight))
                        / flex_weight.max(1)) as u32;
                    flexible_used = flexible_used.saturating_add(size);
                    size
                }
            };

            let gap_before = gap.saturating_mul(index as u32);
            let child_rect = if flow.horizontal {
                Rect {
                    x: inner.x + used_axis + gap_before,
                    y: inner.y,
                    w: size,
                    h: inner.h,
                }
            } else {
                Rect {
                    x: inner.x,
                    y: inner.y + used_axis + gap_before,
                    w: inner.w,
                    h: size,
                }
            };
            parts.push(self.zone(&child.layout, child_rect, &child.style, child.fixed_size)?);
            used_axis = used_axis
                .checked_add(size)
                .ok_or_else(|| Error::new("LAYOUT", "Layout coordinate overflow"))?;
        }

        let param = if flow.horizontal { "horz" } else { "vert" };
        Ok(format!(
            "<zone{} h=\"{}\" id=\"{id}\" param=\"{param}\" type-v2=\"layout-flow\" w=\"{}\" x=\"{}\" y=\"{}\">\n{}{}\n</zone>",
            fixed_zone_attrs(fixed_size),
            rect.h,
            rect.w,
            rect.x,
            rect.y,
            indent(&parts.join("\n"), 2),
            zone_style(style),
        ))
    }

    fn take_id(&mut self) -> Result<u32> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::new("LAYOUT", "Zone id overflow"))?;
        Ok(id)
    }
}

fn fixed_zone_attrs(fixed_size: Option<u32>) -> String {
    fixed_size
        .map(|size| format!(" fixed-size=\"{size}\" is-fixed=\"true\""))
        .unwrap_or_default()
}

fn zone_style(style: &ZoneStyle) -> String {
    let mut formats = Vec::new();
    if let Some(background) = &style.background {
        formats.push(format!(
            "<format attr=\"background-color\" value=\"{}\"/>",
            escape_attr(background)
        ));
    }
    if let Some(border) = &style.border_color {
        formats.push(format!(
            "<format attr=\"border-color\" value=\"{}\"/>",
            escape_attr(border)
        ));
    }
    if style.border_width > 0 {
        formats.push("<format attr=\"border-style\" value=\"solid\"/>".into());
        formats.push(format!(
            "<format attr=\"border-width\" value=\"{}\"/>",
            style.border_width
        ));
    }
    if style.corner_radius > 0 {
        formats.push(format!(
            "<format attr=\"corner-radius\" value=\"{}\"/>",
            style.corner_radius
        ));
    }
    if style.padding > 0 {
        formats.push(format!(
            "<format attr=\"padding\" value=\"{}\"/>",
            style.padding
        ));
    }
    if style.margin > 0 {
        formats.push(format!(
            "<format attr=\"margin\" value=\"{}\"/>",
            style.margin
        ));
    }
    if formats.is_empty() {
        String::new()
    } else {
        format!("\n  <zone-style>{}</zone-style>", formats.join(""))
    }
}

fn root_style(background: Option<&str>) -> String {
    background
        .map(|value| {
            format!(
                "\n          <zone-style><format attr=\"background-color\" value=\"{}\"/></zone-style>",
                escape_attr(value)
            )
        })
        .unwrap_or_default()
}

fn take_id(next_id: &mut u32) -> Result<u32> {
    let id = *next_id;
    *next_id = next_id
        .checked_add(1)
        .ok_or_else(|| Error::new("LAYOUT", "Zone id overflow"))?;
    Ok(id)
}

fn calculation_name(workbook_hash: &str, spec: &CalculationSpec) -> String {
    let mut digest = Sha256::new();
    digest.update(workbook_hash.as_bytes());
    digest.update(b"\0");
    digest.update(spec.datasource.as_bytes());
    digest.update(b"\0");
    digest.update(spec.caption.as_bytes());
    digest.update(b"\0");
    digest.update(spec.formula.as_bytes());
    let hex = format!("{:X}", digest.finalize());
    format!("[Calculation_{}]", &hex[..18])
}

fn deterministic_uuid(workbook_hash: &str, name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(workbook_hash.as_bytes());
    digest.update(b"\0");
    digest.update(name.as_bytes());
    let hex = format!("{:X}", digest.finalize());
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32],
    )
}

fn pixels_to_norm(pixels: u32, dimension: u32) -> u32 {
    ((u64::from(pixels) * u64::from(CANVAS)) / u64::from(dimension)) as u32
}

fn escape_attr(value: &str) -> String {
    escape(value)
}

fn escape_text(value: &str) -> String {
    escape(value)
}

fn escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
    output
}

fn indent(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    let line_count = value
        .as_bytes()
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + 1;
    let mut output = String::with_capacity(value.len() + prefix.len() * line_count);
    for (index, line) in value.lines().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str(&prefix);
        output.push_str(line);
    }
    output
}

fn one() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workbook() -> Result<Workbook> {
        Workbook::open(
            br#"<workbook version="26.2" source-build="0.0.0">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <column datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      <column datatype="real" name="[Profit]" role="measure" type="quantitative"/>
    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Trend"><simple-id uuid="{00000000-0000-0000-0000-000000000001}"/></worksheet>
    <worksheet name="Region"><simple-id uuid="{00000000-0000-0000-0000-000000000002}"/></worksheet>
  </worksheets>
</workbook>"#
                .to_vec(),
        )
    }

    #[test]
    fn creates_calculation_and_dashboard_in_one_batch() -> Result<()> {
        let book = workbook()?;
        let batch = Batch {
            edits: vec![
                Edit::CreateCalculation(CalculationSpec {
                    datasource: "Orders".into(),
                    caption: "Profit Ratio".into(),
                    formula: "[Profit] / [Sales]".into(),
                    datatype: DataType::Real,
                    role: Role::Measure,
                    field_type: FieldType::Quantitative,
                    name: None,
                }),
                Edit::CreateDashboard(DashboardSpec {
                    name: "Overview".into(),
                    width: 1440,
                    height: 900,
                    padding: 24,
                    background: Some("#F7F7F7".into()),
                    view_styles: Vec::new(),
                    layout: Layout::Row {
                        gap: 16,
                        padding: 0,
                        children: vec![
                            Item {
                                weight: 2,
                                fixed_size: None,
                                style: ZoneStyle::default(),
                                layout: Layout::Sheet {
                                    sheet: "Trend".into(),
                                    show_title: None,
                                    fit: None,
                                },
                            },
                            Item {
                                weight: 1,
                                fixed_size: None,
                                style: ZoneStyle::default(),
                                layout: Layout::Sheet {
                                    sheet: "Region".into(),
                                    show_title: None,
                                    fit: None,
                                },
                            },
                        ],
                    },
                }),
            ],
        };

        let candidate = apply(&book, &batch, 8 << 20)?;
        assert_eq!(
            candidate.receipt.sha256,
            format!("{:x}", Sha256::digest(candidate.text().as_bytes()))
        );
        let reopened = &candidate.workbook;
        assert_eq!(reopened.calculations.len(), 1);
        assert!(reopened.dashboard_id("Overview").is_some());
        Ok(())
    }

    #[test]
    fn creates_forward_same_batch_calculation_dependency() -> Result<()> {
        let book = workbook()?;
        let batch = Batch {
            edits: vec![
                Edit::CreateCalculation(CalculationSpec {
                    datasource: "Orders".into(),
                    caption: "Adjusted Profit".into(),
                    formula: "[Profit Ratio] * [Sales]".into(),
                    datatype: DataType::Real,
                    role: Role::Measure,
                    field_type: FieldType::Quantitative,
                    name: Some("[Adjusted Profit]".into()),
                }),
                Edit::CreateCalculation(CalculationSpec {
                    datasource: "Orders".into(),
                    caption: "Profit Ratio".into(),
                    formula: "[Profit] / [Sales]".into(),
                    datatype: DataType::Real,
                    role: Role::Measure,
                    field_type: FieldType::Quantitative,
                    name: Some("[Profit Ratio]".into()),
                }),
            ],
        };

        let candidate = apply(&book, &batch, 8 << 20)?;
        let reopened = &candidate.workbook;
        let datasource = reopened
            .datasource_id("Orders")
            .ok_or_else(|| Error::new("TEST", "Orders datasource disappeared"))?;
        let ratio = reopened
            .field_id(datasource, "[Profit Ratio]")
            .ok_or_else(|| Error::new("TEST", "Profit Ratio was not created"))?;
        let adjusted = reopened
            .field_id(datasource, "[Adjusted Profit]")
            .ok_or_else(|| Error::new("TEST", "Adjusted Profit was not created"))?;
        assert!(
            reopened
                .field(ratio)
                .is_some_and(|field| field.calculation.is_some())
        );
        assert!(
            reopened
                .field(adjusted)
                .is_some_and(|field| field.calculation.is_some())
        );
        assert_eq!(reopened.calculations.len(), 2);
        Ok(())
    }

    #[test]
    fn rejects_duplicate_same_batch_internal_calculation_name() -> Result<()> {
        let book = workbook()?;
        let batch = Batch {
            edits: vec![
                Edit::CreateCalculation(CalculationSpec {
                    datasource: "Orders".into(),
                    caption: "Ratio A".into(),
                    formula: "[Profit] / [Sales]".into(),
                    datatype: DataType::Real,
                    role: Role::Measure,
                    field_type: FieldType::Quantitative,
                    name: Some("[Shared Calc]".into()),
                }),
                Edit::CreateCalculation(CalculationSpec {
                    datasource: "Orders".into(),
                    caption: "Ratio B".into(),
                    formula: "[Sales] / [Profit]".into(),
                    datatype: DataType::Real,
                    role: Role::Measure,
                    field_type: FieldType::Quantitative,
                    name: Some("[Shared Calc]".into()),
                }),
            ],
        };

        let error = match apply(&book, &batch, 8 << 20) {
            Ok(_) => return Err(Error::new("TEST", "duplicate internal name was accepted")),
            Err(error) => error,
        };
        assert_eq!(error.code, "CALCULATION_EXISTS");
        Ok(())
    }

    #[test]
    fn rejects_unplanned_calculation_reference() -> Result<()> {
        let book = workbook()?;
        let batch = Batch {
            edits: vec![Edit::CreateCalculation(CalculationSpec {
                datasource: "Orders".into(),
                caption: "Broken".into(),
                formula: "[Missing Calc] + [Sales]".into(),
                datatype: DataType::Real,
                role: Role::Measure,
                field_type: FieldType::Quantitative,
                name: Some("[Broken]".into()),
            })],
        };

        let error = match apply(&book, &batch, 8 << 20) {
            Ok(_) => return Err(Error::new("TEST", "unknown reference was accepted")),
            Err(error) => error,
        };
        assert_eq!(error.code, "FIELD_NOT_FOUND");
        Ok(())
    }

    #[test]
    fn creates_multiple_dashboards_in_single_root_containers() -> Result<()> {
        let book = workbook()?;
        let dashboard = |name: &str, sheet: &str| {
            Edit::CreateDashboard(DashboardSpec {
                name: name.into(),
                width: 1200,
                height: 800,
                padding: 16,
                background: None,
                view_styles: Vec::new(),
                layout: Layout::Sheet {
                    sheet: sheet.into(),
                    show_title: None,
                    fit: None,
                },
            })
        };
        let batch = Batch {
            edits: vec![
                dashboard("Overview", "Trend"),
                dashboard("Regional", "Region"),
            ],
        };

        let candidate = apply(&book, &batch, 8 << 20)?;
        let reopened = &candidate.workbook;
        assert_eq!(reopened.dashboards.len(), 2);
        assert!(reopened.dashboard_id("Overview").is_some());
        assert!(reopened.dashboard_id("Regional").is_some());
        assert_eq!(candidate.text().matches("<dashboards>").count(), 1);
        assert_eq!(candidate.text().matches("</dashboards>").count(), 1);
        assert_eq!(candidate.text().matches("<windows>").count(), 1);
        assert_eq!(candidate.text().matches("</windows>").count(), 1);
        assert_eq!(
            candidate
                .text()
                .matches("<window class=\"dashboard\"")
                .count(),
            2
        );
        assert!(
            candidate.text().find("<dashboards>").unwrap_or(usize::MAX)
                < candidate.text().find("<windows>").unwrap_or(0)
        );

        let mut zone_ids = HashSet::new();
        for dashboard in &reopened.dashboards {
            for zone in reopened
                .xml
                .descendants(dashboard.node)
                .filter(|node| reopened.xml.tag(*node) == "zone")
            {
                let id = reopened
                    .xml
                    .attr_raw(zone, "id")
                    .ok_or_else(|| Error::new("TEST", "dashboard zone is missing id"))?;
                require(
                    zone_ids.insert(id.to_owned()),
                    "TEST",
                    format!("dashboard zone id {id:?} was reused across the batch"),
                )?;
            }
        }
        Ok(())
    }

    #[test]
    fn keeps_repeated_filter_cards_distinct_in_layout_order() -> Result<()> {
        let book = dashboard_workbook()?;
        let filter = |control: FilterControl, title: &str| Item {
            weight: 1,
            fixed_size: None,
            style: ZoneStyle::default(),
            layout: Layout::Filter {
                source_sheet: "Trend".into(),
                field: "Region".into(),
                instance: Some("[none:Region:nk]".into()),
                control,
                values: None,
                title: Some(title.into()),
            },
        };
        let batch = Batch {
            edits: vec![Edit::CreateDashboard(DashboardSpec {
                name: "Filters".into(),
                width: 1000,
                height: 700,
                padding: 12,
                background: None,
                view_styles: Vec::new(),
                layout: Layout::Row {
                    gap: 8,
                    padding: 0,
                    children: vec![
                        filter(FilterControl::SingleDropdown, "Primary Region"),
                        filter(FilterControl::MultipleDropdown, "Secondary Region"),
                    ],
                },
            })],
        };

        let candidate = apply(&book, &batch, 8 << 20)?;
        assert_eq!(candidate.text().matches("type-v2=\"filter\"").count(), 2);
        assert_eq!(candidate.text().matches("mode=\"dropdown\"").count(), 1);
        assert_eq!(
            candidate.text().matches("mode=\"checkdropdown\"").count(),
            1
        );
        assert!(candidate.text().contains("<run>Primary Region</run>"));
        assert!(candidate.text().contains("<run>Secondary Region</run>"));
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_filter_instance_without_exact_name() -> Result<()> {
        let source = dashboard_workbook()?
            .xml
            .source()
            .replace(
                r#"<column-instance column="[Region]" derivation="None" name="[none:Region:nk]" pivot="key" type="nominal"/>"#,
                r#"<column-instance column="[Region]" derivation="None" name="[none:Region:nk]" pivot="key" type="nominal"/>
            <column-instance column="[Region]" derivation="Attribute" name="[attr:Region:nk]" pivot="key" type="nominal"/>"#,
            );
        let book = Workbook::open(source.into_bytes())?;
        let batch = Batch {
            edits: vec![Edit::CreateDashboard(DashboardSpec {
                name: "Ambiguous Filter".into(),
                width: 1000,
                height: 700,
                padding: 12,
                background: None,
                view_styles: Vec::new(),
                layout: Layout::Filter {
                    source_sheet: "Trend".into(),
                    field: "Region".into(),
                    instance: None,
                    control: FilterControl::Auto,
                    values: None,
                    title: None,
                },
            })],
        };

        let error = match apply(&book, &batch, 8 << 20) {
            Ok(_) => return Err(Error::new("TEST", "ambiguous filter instance was accepted")),
            Err(error) => error,
        };
        assert_eq!(error.code, "FILTER_INSTANCE_AMBIGUOUS");
        Ok(())
    }

    #[test]
    fn rejects_cross_dashboard_style_patch_overlap_before_lowering() -> Result<()> {
        let book = dashboard_workbook()?;
        let style = || ViewStyleSpec {
            sheet: "Trend".into(),
            font_family: Some("Tableau Book".into()),
            font_size: Some(11),
            color: None,
            axis: None,
            grid_lines: None,
            zero_line: None,
            line_width: None,
            show_labels: None,
        };
        let dashboard = |name: &str| {
            Edit::CreateDashboard(DashboardSpec {
                name: name.into(),
                width: 1000,
                height: 700,
                padding: 12,
                background: None,
                view_styles: vec![style()],
                layout: Layout::Sheet {
                    sheet: "Trend".into(),
                    show_title: None,
                    fit: None,
                },
            })
        };
        let batch = Batch {
            edits: vec![dashboard("A"), dashboard("B")],
        };

        let error = match apply(&book, &batch, 8 << 20) {
            Ok(_) => return Err(Error::new("TEST", "overlapping view styles were accepted")),
            Err(error) => error,
        };
        assert_eq!(error.code, "DUPLICATE_STYLE");
        Ok(())
    }

    #[test]
    fn parses_agent_facing_json_batch() -> Result<()> {
        let json = br##"{
          "edits": [
            {
              "op": "create_calculation",
              "datasource": "Orders",
              "caption": "Profit Ratio",
              "formula": "[Profit] / [Sales]",
              "datatype": "real",
              "role": "measure",
              "type": "quantitative",
              "name": null
            },
            {
              "op": "create_dashboard",
              "name": "Overview",
              "width": 1440,
              "height": 900,
              "padding": 24,
              "background": "#F7F7F7",
              "layout": {
                "type": "row",
                "gap": 16,
                "children": [
                  {
                    "weight": 2,
                    "style": {
                      "background": "#FFFFFF",
                      "corner_radius": 8,
                      "padding": 8
                    },
                    "type": "sheet",
                    "sheet": "Trend"
                  },
                  {
                    "type": "sheet",
                    "sheet": "Region"
                  }
                ]
              }
            }
          ]
        }"##;

        let batch: Batch = serde_json::from_slice(json)?;
        let candidate = apply(&workbook()?, &batch, 8 << 20)?;
        assert!(candidate.text().contains(r#"attr="padding" value="8""#));
        assert!(candidate.text().contains(r#"name="Overview""#));
        Ok(())
    }

    fn dashboard_workbook() -> Result<Workbook> {
        Workbook::open(
            br#"<workbook version="26.2" source-build="0.0.0">
  <datasources>
    <datasource inline="true" name="Orders" version="26.2">
      <column datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      <column datatype="string" name="[Region]" role="dimension" type="nominal"/>
    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Trend">
      <table>
        <view>
          <datasource-dependencies datasource="Orders">
            <column datatype="real" name="[Sales]" role="measure" type="quantitative"/>
            <column datatype="string" name="[Region]" role="dimension" type="nominal"/>
            <column-instance column="[Sales]" derivation="Sum" name="[sum:Sales:qk]" pivot="key" type="quantitative"/>
            <column-instance column="[Region]" derivation="None" name="[none:Region:nk]" pivot="key" type="nominal"/>
          </datasource-dependencies>
          <filter class="categorical" column="[Orders].[none:Region:nk]"/>
        </view>
        <style/>
        <panes>
          <pane>
            <view><breakdown value="auto"/></view>
            <mark class="Line"/>
            <style/>
          </pane>
        </panes>
        <rows/>
        <cols/>
      </table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000001}"/>
    </worksheet>
    <worksheet name="Region">
      <table><view/><panes><pane><view/><mark class="Bar"/></pane></panes><rows/><cols/></table>
      <simple-id uuid="{00000000-0000-0000-0000-000000000002}"/>
    </worksheet>
  </worksheets>
</workbook>"#
                .to_vec(),
        )
    }

    fn worksheet_authoring_workbook() -> Result<Workbook> {
        Workbook::open(
            br#"<workbook version="26.2" source-build="0.0.0">
  <datasources>
    <datasource caption="Orders" inline="true" name="Orders" version="26.2">
      <column caption="Sales" datatype="real" name="[Sales]" role="measure" type="quantitative"/>
      <column caption="Region" datatype="string" name="[Region]" role="dimension" type="nominal"/>
      <column caption="Category" datatype="string" name="[Category]" role="dimension" type="nominal"/>
      <column caption="Order Date" datatype="date" name="[Order Date]" role="dimension" type="ordinal"/>
    </datasource>
  </datasources>
  <worksheets>
    <worksheet name="Seed"><simple-id uuid="{00000000-0000-0000-0000-000000000001}"/></worksheet>
  </worksheets>
</workbook>"#
                .to_vec(),
        )
    }

    #[test]
    fn creates_and_indexes_list_and_range_parameters() -> Result<()> {
        let book = worksheet_authoring_workbook()?;
        let batch = Batch {
            edits: vec![
                Edit::CreateParameter(ParameterSpec {
                    caption: "View Mode".into(),
                    datatype: DataType::String,
                    value: serde_json::json!("Bar"),
                    domain: ParameterDomain::List {
                        values: vec![serde_json::json!("Bar"), serde_json::json!("Line")],
                    },
                    name: Some("[View Mode]".into()),
                }),
                Edit::CreateParameter(ParameterSpec {
                    caption: "Top N".into(),
                    datatype: DataType::Integer,
                    value: serde_json::json!(5),
                    domain: ParameterDomain::Range {
                        min: serde_json::json!(1),
                        max: serde_json::json!(20),
                        step: serde_json::json!(1),
                    },
                    name: Some("[Top N]".into()),
                }),
            ],
        };

        let candidate = apply(&book, &batch, 8 << 20)?;
        assert!(candidate.workbook.parameter_id("View Mode").is_some());
        assert!(candidate.workbook.parameter_id("[Top N]").is_some());
        assert_eq!(candidate.workbook.parameters.len(), 2);
        let inspected = crate::workbook::inspect_limited(
            &candidate.workbook,
            "parameter",
            Some("view mode"),
            10,
        )?;
        assert_eq!(inspected["count"], 1);
        assert_eq!(inspected["items"][0]["caption"], "View Mode");
        assert_eq!(
            candidate.workbook.calculations.len(),
            book.calculations.len(),
            "parameter value calculations must not be exposed as authored calculations"
        );
        assert!(candidate.text().contains(r#"param-domain-type="list""#));
        assert!(candidate.text().contains(r#"param-domain-type="range""#));
        assert!(
            candidate
                .text()
                .contains(r#"<member value="&quot;Bar&quot;"/>"#)
        );
        assert!(
            candidate
                .text()
                .contains(r#"<range granularity="1" max="20" min="1"/>"#)
        );
        Ok(())
    }

    #[test]
    fn creates_minimal_bar_line_kpi_and_text_table_worksheets() -> Result<()> {
        let book = worksheet_authoring_workbook()?;
        let batch = Batch {
            edits: vec![
                Edit::CreateWorksheet(WorksheetSpec {
                    name: "Sales by Region".into(),
                    datasource: "Orders".into(),
                    kind: WorksheetKind::Bar,
                    dimension: Some("Region".into()),
                    secondary_dimension: None,
                    measure: "Sales".into(),
                    aggregation: Aggregation::Sum,
                }),
                Edit::CreateWorksheet(WorksheetSpec {
                    name: "Sales Trend".into(),
                    datasource: "Orders".into(),
                    kind: WorksheetKind::Line,
                    dimension: Some("Order Date".into()),
                    secondary_dimension: None,
                    measure: "Sales".into(),
                    aggregation: Aggregation::Sum,
                }),
                Edit::CreateWorksheet(WorksheetSpec {
                    name: "Total Sales".into(),
                    datasource: "Orders".into(),
                    kind: WorksheetKind::Kpi,
                    dimension: None,
                    secondary_dimension: None,
                    measure: "Sales".into(),
                    aggregation: Aggregation::Sum,
                }),
                Edit::CreateWorksheet(WorksheetSpec {
                    name: "Sales Matrix".into(),
                    datasource: "Orders".into(),
                    kind: WorksheetKind::TextTable,
                    dimension: Some("Region".into()),
                    secondary_dimension: Some("Category".into()),
                    measure: "Sales".into(),
                    aggregation: Aggregation::Sum,
                }),
            ],
        };

        let candidate = apply(&book, &batch, 8 << 20)?;
        for name in [
            "Sales by Region",
            "Sales Trend",
            "Total Sales",
            "Sales Matrix",
        ] {
            assert!(
                candidate.workbook.worksheet_id(name).is_some(),
                "worksheet {name:?} was not indexed after authoring"
            );
        }
        assert!(candidate.text().contains(r#"derivation="Month-Trunc""#));
        assert!(candidate.text().contains(r#"name="[tmn:Order Date:qk]""#));
        assert!(
            candidate
                .text()
                .contains(r#"<text column="[Orders].[sum:Sales:qk]"/>"#)
        );
        assert_eq!(
            candidate
                .text()
                .matches(r#"<mark class="Automatic"/>"#)
                .count(),
            4
        );
        Ok(())
    }

    #[test]
    fn creates_filter_and_parameter_actions_in_sequential_edits() -> Result<()> {
        let base = dashboard_workbook()?;
        let dashboard = apply(
            &base,
            &Batch {
                edits: vec![Edit::CreateDashboard(DashboardSpec {
                    name: "Interactive".into(),
                    width: 1200,
                    height: 800,
                    padding: 16,
                    background: None,
                    view_styles: Vec::new(),
                    layout: Layout::Row {
                        gap: 12,
                        padding: 0,
                        children: vec![
                            Item {
                                weight: 1,
                                fixed_size: None,
                                style: ZoneStyle::default(),
                                layout: Layout::Sheet {
                                    sheet: "Trend".into(),
                                    show_title: None,
                                    fit: None,
                                },
                            },
                            Item {
                                weight: 1,
                                fixed_size: None,
                                style: ZoneStyle::default(),
                                layout: Layout::Sheet {
                                    sheet: "Region".into(),
                                    show_title: None,
                                    fit: None,
                                },
                            },
                        ],
                    },
                })],
            },
            8 << 20,
        )?;

        let filtered = apply(
            &dashboard.workbook,
            &Batch {
                edits: vec![Edit::CreateFilterAction(FilterActionSpec {
                    caption: "Filter other views".into(),
                    dashboard: "Interactive".into(),
                    source_sheet: "Trend".into(),
                    run_on: ActionTrigger::Select,
                    auto_clear: true,
                })],
            },
            8 << 20,
        )?;
        assert!(filtered.text().contains(r#"command="tsc:tsl-filter""#));
        assert!(filtered.text().contains(r#"name="exclude" value="Trend""#));

        let parameterized = apply(
            &filtered.workbook,
            &Batch {
                edits: vec![Edit::CreateParameter(ParameterSpec {
                    caption: "Selected Region".into(),
                    datatype: DataType::String,
                    value: serde_json::json!("All"),
                    domain: ParameterDomain::Any,
                    name: Some("[Selected Region]".into()),
                })],
            },
            8 << 20,
        )?;

        let action = apply(
            &parameterized.workbook,
            &Batch {
                edits: vec![Edit::CreateParameterAction(ParameterActionSpec {
                    caption: "Set selected region".into(),
                    dashboard: "Interactive".into(),
                    source_sheet: "Trend".into(),
                    source_field: "Region".into(),
                    source_instance: Some("[none:Region:nk]".into()),
                    target_parameter: "Selected Region".into(),
                    run_on: ActionTrigger::Select,
                })],
            },
            8 << 20,
        )?;

        assert!(action.text().contains("<edit-parameter-action"));
        assert!(action.text().contains(r#"<agg-type type="attr"/>"#));
        assert!(
            action
                .text()
                .contains(r#"name="target-parameter" value="[Parameters].[Selected Region]""#)
        );
        assert!(action.text().contains(r#"<datasource name="Parameters"/>"#));
        assert!(
            action
                .text()
                .contains(r#"<datasource-dependencies datasource="Parameters">"#)
        );
        assert!(action_caption_exists(
            &action.workbook,
            "Filter other views"
        )?);
        assert!(action_caption_exists(
            &action.workbook,
            "Set selected region"
        )?);
        Ok(())
    }

    #[test]
    fn parameter_action_rejects_source_target_datatype_mismatch() -> Result<()> {
        let base = dashboard_workbook()?;
        let dashboard = apply(
            &base,
            &Batch {
                edits: vec![Edit::CreateDashboard(DashboardSpec {
                    name: "Interactive".into(),
                    width: 1000,
                    height: 700,
                    padding: 0,
                    background: None,
                    view_styles: Vec::new(),
                    layout: Layout::Sheet {
                        sheet: "Trend".into(),
                        show_title: None,
                        fit: None,
                    },
                })],
            },
            8 << 20,
        )?;
        let parameterized = apply(
            &dashboard.workbook,
            &Batch {
                edits: vec![Edit::CreateParameter(ParameterSpec {
                    caption: "Numeric Target".into(),
                    datatype: DataType::Integer,
                    value: serde_json::json!(1),
                    domain: ParameterDomain::Any,
                    name: Some("[Numeric Target]".into()),
                })],
            },
            8 << 20,
        )?;
        let error = match apply(
            &parameterized.workbook,
            &Batch {
                edits: vec![Edit::CreateParameterAction(ParameterActionSpec {
                    caption: "Invalid action".into(),
                    dashboard: "Interactive".into(),
                    source_sheet: "Trend".into(),
                    source_field: "Region".into(),
                    source_instance: Some("[none:Region:nk]".into()),
                    target_parameter: "Numeric Target".into(),
                    run_on: ActionTrigger::Select,
                })],
            },
            8 << 20,
        ) {
            Ok(_) => {
                return Err(Error::new(
                    "TEST",
                    "datatype-mismatched action was accepted",
                ));
            }
            Err(error) => error,
        };
        assert_eq!(error.code, "ACTION");
        Ok(())
    }

    #[test]
    fn creates_fixed_filter_sidebar_and_view_formatting() -> Result<()> {
        let book = dashboard_workbook()?;
        let batch = Batch {
            edits: vec![Edit::CreateDashboard(DashboardSpec {
                name: "Styled".into(),
                width: 1440,
                height: 900,
                padding: 24,
                background: Some("#F5F6F8".into()),
                view_styles: vec![ViewStyleSpec {
                    sheet: "Trend".into(),
                    font_family: Some("Tableau Book".into()),
                    font_size: Some(11),
                    color: Some("#333333".into()),
                    axis: Some(LineStyle {
                        visible: true,
                        color: Some("#DADCE0".into()),
                        width: Some(1),
                    }),
                    grid_lines: Some(LineStyle {
                        visible: true,
                        color: Some("#ECEFF1".into()),
                        width: Some(1),
                    }),
                    zero_line: Some(LineStyle {
                        visible: false,
                        color: None,
                        width: None,
                    }),
                    line_width: Some(2.0),
                    show_labels: Some(true),
                }],
                layout: Layout::Row {
                    gap: 16,
                    padding: 8,
                    children: vec![
                        Item {
                            weight: 1,
                            fixed_size: None,
                            style: ZoneStyle {
                                background: Some("#FFFFFF".into()),
                                border_color: Some("#E4E6EA".into()),
                                border_width: 1,
                                corner_radius: 8,
                                padding: 8,
                                margin: 0,
                            },
                            layout: Layout::Sheet {
                                sheet: "Trend".into(),
                                show_title: Some(false),
                                fit: Some(FitMode::FitWidth),
                            },
                        },
                        Item {
                            weight: 1,
                            fixed_size: Some(280),
                            style: ZoneStyle {
                                background: Some("#FFFFFF".into()),
                                border_color: Some("#E4E6EA".into()),
                                border_width: 1,
                                corner_radius: 8,
                                padding: 8,
                                margin: 0,
                            },
                            layout: Layout::Filter {
                                source_sheet: "Trend".into(),
                                field: "Region".into(),
                                instance: None,
                                control: FilterControl::MultipleDropdown,
                                values: Some(FilterValues::Relevant),
                                title: Some("Region".into()),
                            },
                        },
                    ],
                },
            })],
        };

        let candidate = apply(&book, &batch, 8 << 20)?;
        assert!(candidate.text().contains(r#"type-v2="filter""#));
        assert!(candidate.text().contains(r#"mode="checkdropdown""#));
        assert!(
            candidate
                .text()
                .contains(r#"fixed-size="280" is-fixed="true""#)
        );
        assert!(candidate.text().contains(r#"<zoom type="fit-width"/>"#));
        assert!(candidate.text().contains(r#"show-title="false""#));
        assert!(
            candidate
                .text()
                .contains(r#"<style-rule element="gridline">"#)
        );
        assert!(candidate.text().contains(r#"attr="stroke-size" value="1""#));
        assert!(candidate.text().contains(r#"attr="size" value="2""#));
        assert!(
            candidate
                .text()
                .contains(r#"<datasource-dependencies datasource="Orders">"#)
        );
        assert!(
            candidate
                .text()
                .contains(r#"param="[Orders].[none:Region:nk]""#)
        );
        assert!(
            candidate
                .text()
                .contains(r#"<window class="dashboard" name="Styled">"#)
        );
        Ok(())
    }
}
