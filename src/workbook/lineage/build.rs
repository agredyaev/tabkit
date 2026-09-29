use super::{EdgeKind, Gap, Graph, GraphNode, Link, NodeKind, NodeRef};
use crate::{
    error::{Result, require},
    formula,
    workbook::{DatasourceId, FieldId, Workbook},
    xml::NodeId,
};
use std::collections::{BTreeMap, BTreeSet};
#[path = "extra.rs"]
mod extra;

impl Graph {
    pub fn build(book: Workbook, input_sha256: String) -> Result<Self> {
        let xml = &book.xml;
        let mut gaps: Vec<Gap> = book
            .diagnostics
            .iter()
            .map(|d| Gap {
                code: d.code.clone(),
                object: d.object.clone(),
                detail: d.message.clone(),
                context: None,
            })
            .collect();
        let mut worksheet_ids = BTreeMap::new();
        for (i, &n) in xml.semantic.worksheets.iter().enumerate() {
            worksheet_ids.insert(n, i);
        }
        let mut dashboard_ids = BTreeMap::new();
        for (i, &n) in xml.semantic.dashboards.iter().enumerate() {
            dashboard_ids.insert(n, i);
        }
        let mut filters = Vec::new();
        let mut worksheet_filter_count = 0;
        let mut datasource_filter_count = 0;
        let mut extract_filter_count = 0;
        let mut shared_view_filter_count = 0;
        for &n in &xml.semantic.filters {
            if let Some(sheet) = xml.ancestor(n, "worksheet") {
                filters.push((
                    n,
                    NodeRef::new(NodeKind::WorksheetFilter, worksheet_filter_count),
                    worksheet_ids
                        .get(&sheet)
                        .copied()
                        .map(|i| NodeRef::new(NodeKind::Worksheet, i)),
                ));
                worksheet_filter_count += 1;
            } else if xml.ancestor(n, "extract").is_some() {
                let owner = xml
                    .ancestor(n, "datasource")
                    .and_then(|ds| xml.value(ds, "name"))
                    .and_then(|name| book.datasource_lookup.get(name))
                    .map(|id| NodeRef::new(NodeKind::Datasource, id.0 as usize));
                filters.push((
                    n,
                    NodeRef::new(NodeKind::ExtractFilter, extract_filter_count),
                    owner,
                ));
                extract_filter_count += 1;
            } else if let Some(ds) = xml.ancestor(n, "datasource") {
                let owner = xml
                    .value(ds, "name")
                    .and_then(|s| book.datasource_lookup.get(s))
                    .map(|id| NodeRef::new(NodeKind::Datasource, id.0 as usize));
                filters.push((
                    n,
                    NodeRef::new(NodeKind::DatasourceFilter, datasource_filter_count),
                    owner,
                ));
                datasource_filter_count += 1;
            } else if xml.ancestor(n, "shared-view").is_some() {
                filters.push((
                    n,
                    NodeRef::new(NodeKind::SharedViewFilter, shared_view_filter_count),
                    None,
                ));
                shared_view_filter_count += 1;
                gap(
                    &mut gaps,
                    "SHARED_VIEW_SCOPE",
                    n,
                    "Shared-view filter target worksheets are not established",
                );
            } else {
                gap(
                    &mut gaps,
                    "FILTER_SCOPE",
                    n,
                    "Filter scope is not a worksheet or datasource",
                );
            }
        }
        let mut local_definitions = Vec::new();
        for definition in &book.local_definitions {
            let sheet = xml
                .ancestor(definition.node_id, "worksheet")
                .and_then(|node| worksheet_ids.get(&node).map(|&id| (node, id)));
            let datasource = xml
                .ancestor(definition.node_id, "datasource-dependencies")
                .and_then(|node| xml.value(node, "datasource"))
                .and_then(|name| book.datasource_lookup.get(name).copied());
            if let (Some((sheet, sheet_id)), Some(datasource)) = (sheet, datasource) {
                local_definitions.push((definition, sheet, sheet_id, datasource));
            } else {
                gap(
                    &mut gaps,
                    "LOCAL_DEFINITION",
                    definition.node_id,
                    "Local field has no known worksheet or datasource",
                );
            }
        }
        for field in &book.fields {
            for &copy in field.copies.iter().skip(1) {
                let formula = xml
                    .named_children(copy, "calculation")
                    .next()
                    .and_then(|node| xml.value(node, "formula"));
                if formula != field.formula.as_deref() {
                    gap(
                        &mut gaps,
                        "INCONSISTENT_DEFINITION",
                        copy,
                        "Worksheet copy differs from the indexed field formula",
                    );
                }
            }
        }
        let mut nodes = Vec::with_capacity(book.fields.len() + book.datasources.len() +
            book.worksheets.len() + book.dashboards.len() + filters.len() + local_definitions.len());
        for (i, f) in book.fields.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Field, i),
                name: f.name.clone(),
                caption: f.caption.clone(),
                datasource: Some(NodeRef::new(NodeKind::Datasource, f.datasource.0 as usize)),
                worksheet: None,
                formula: f.formula.clone(),
                data_type: Some(f.datatype.clone()),
                role: Some(f.role.clone()),
            });
        }
        for (i, d) in book.datasources.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Datasource, i),
                name: d.name.clone(),
                caption: d.caption.clone(),
                datasource: None,
                worksheet: None,
                formula: None,
                data_type: None,
                role: None,
            });
        }
        for (n, r, owner) in filters
            .iter()
            .filter(|(_, r, _)| r.kind == NodeKind::WorksheetFilter)
        {
            nodes.push(filter_node(xml, *n, *r, *owner));
        }
        for (n, r, owner) in filters
            .iter()
            .filter(|(_, r, _)| r.kind == NodeKind::DatasourceFilter)
        {
            nodes.push(filter_node(xml, *n, *r, *owner));
        }
        for (i, name) in book.worksheets.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Worksheet, i),
                name: name.clone(),
                caption: name.clone(),
                datasource: None,
                worksheet: None,
                formula: None,
                data_type: None,
                role: None,
            });
        }
        for (i, name) in book.dashboards.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Dashboard, i),
                name: name.clone(),
                caption: name.clone(),
                datasource: None,
                worksheet: None,
                formula: None,
                data_type: None,
                role: None,
            });
        }
        let mut local_lookup = BTreeMap::<(NodeId, DatasourceId, String), BTreeSet<NodeRef>>::new();
        for (id, &(definition, sheet, sheet_id, datasource)) in local_definitions.iter().enumerate()
        {
            let reference = NodeRef::new(NodeKind::LocalField, id);
            let name = definition.name.clone().unwrap_or_default();
            if name.is_empty() {
                gap(
                    &mut gaps,
                    "LOCAL_DEFINITION",
                    definition.node_id,
                    "Local field has no internal name",
                );
            } else {
                local_lookup
                    .entry((sheet, datasource, name.clone()))
                    .or_default()
                    .insert(reference);
            }
            nodes.push(GraphNode {
                reference,
                caption: xml
                    .value(definition.node_id, "caption")
                    .unwrap_or(&name)
                    .into(),
                name,
                datasource: Some(NodeRef::new(NodeKind::Datasource, datasource.0 as usize)),
                worksheet: Some(NodeRef::new(NodeKind::Worksheet, sheet_id)),
                formula: definition.formula.clone(),
                data_type: xml.value(definition.node_id, "datatype").map(str::to_owned),
                role: xml.value(definition.node_id, "role").map(str::to_owned),
            });
        }
        for (n, r, owner) in filters
            .iter()
            .filter(|(_, r, _)| r.kind == NodeKind::ExtractFilter)
        {
            nodes.push(filter_node(xml, *n, *r, *owner));
        }
        for (n, r, _) in filters
            .iter()
            .filter(|(_, r, _)| r.kind == NodeKind::SharedViewFilter)
        {
            let mut node = filter_node(xml, *n, *r, None);
            node.datasource = xml
                .ancestor(*n, "shared-view")
                .and_then(|view| xml.value(view, "name"))
                .and_then(|name| book.datasource_lookup.get(name))
                .map(|ds| NodeRef::new(NodeKind::Datasource, ds.0 as usize));
            nodes.push(node);
        }
        let mut raw = Vec::<(NodeRef, NodeRef, EdgeKind)>::new();
        for e in &book.edges {
            raw.push((
                NodeRef::new(NodeKind::Field, e.to.0 as usize),
                NodeRef::new(NodeKind::Field, e.from.0 as usize),
                EdgeKind::Calculation,
            ));
        }
        let mut sheet_sources: BTreeMap<NodeId, BTreeSet<DatasourceId>> = BTreeMap::new();
        for &scope in &xml.semantic.datasource_dependencies {
            let Some(sheet) = xml.ancestor(scope, "worksheet") else {
                continue;
            };
            let Some(name) = xml.value(scope, "datasource") else {
                gap(
                    &mut gaps,
                    "DATASOURCE_REFERENCE",
                    scope,
                    "Datasource dependency has no name",
                );
                continue;
            };
            if let Some(&ds) = book.datasource_lookup.get(name) {
                sheet_sources.entry(sheet).or_default().insert(ds);
            } else {
                gap(
                    &mut gaps,
                    "DATASOURCE_REFERENCE",
                    scope,
                    "Unknown worksheet datasource",
                );
            }
        }
        // Some books declare a worksheet datasource without a dependency copy.
        for (i, _) in xml.nodes.iter().enumerate() {
            let n = NodeId(i as u32);
            if xml.tag(n) != "datasource" {
                continue;
            }
            let Some(sheet) = xml.ancestor(n, "worksheet") else {
                continue;
            };
            let Some(parent) = xml.node(n).parent() else {
                continue;
            };
            if xml.tag(parent) != "datasources" {
                continue;
            }
            if let Some(ds) = xml
                .value(n, "name")
                .and_then(|s| book.datasource_lookup.get(s))
            {
                sheet_sources.entry(sheet).or_default().insert(*ds);
            } else {
                gap(
                    &mut gaps,
                    "DATASOURCE_REFERENCE",
                    n,
                    "Unknown worksheet datasource",
                );
            }
        }
        for (sheet, sources) in &sheet_sources {
            if let Some(&sid) = worksheet_ids.get(sheet) {
                for ds in sources {
                    raw.push((
                        NodeRef::new(NodeKind::Datasource, ds.0 as usize),
                        NodeRef::new(NodeKind::Worksheet, sid),
                        EdgeKind::DatasourceUsed,
                    ));
                }
            }
        }
        let instances = instances(&book, &local_lookup);
        let resolver = Resolver {
            book: &book,
            instances: &instances,
            local_lookup: &local_lookup,
            sheet_sources: &sheet_sources,
        };
        for (id, &(definition, sheet, _, datasource)) in local_definitions.iter().enumerate() {
            let Some(formula) = definition.formula.as_deref() else {
                continue;
            };
            match formula::analyze(formula) {
                Ok(analysis) => {
                    for reference in analysis.references {
                        if let Some(source) =
                            resolver.reference(Some(sheet), Some(datasource), &reference)
                        {
                            raw.push((
                                source,
                                NodeRef::new(NodeKind::LocalField, id),
                                EdgeKind::Calculation,
                            ));
                        } else {
                            gap(
                                &mut gaps,
                                "LOCAL_FORMULA_REFERENCE",
                                definition.node_id,
                                "Unresolved or ambiguous local calculation input",
                            );
                        }
                    }
                }
                Err(_) => gap(
                    &mut gaps,
                    "LOCAL_FORMULA",
                    definition.node_id,
                    "Local calculation could not be analyzed",
                ),
            }
        }
        for &n in &xml.semantic.shelves {
            let Some(sheet) = xml.ancestor(n, "worksheet") else {
                continue;
            };
            let Some(&sid) = worksheet_ids.get(&sheet) else {
                continue;
            };
            let kind = if xml.tag(n) == "rows" {
                EdgeKind::Rows
            } else {
                EdgeKind::Cols
            };
            let text = match xml.text_content(n) {
                Ok(text) if !text.trim().is_empty() => text,
                Ok(_) => continue,
                Err(_) => {
                    gap(
                        &mut gaps,
                        "SHELF_EXPRESSION",
                        n,
                        "Shelf content is not a leaf expression",
                    );
                    continue;
                }
            };
            match formula::analyze(text) {
                Ok(analysis) => {
                    for reference in analysis.references {
                        match resolver.reference(Some(sheet), None, &reference) {
                            Some(field) => {
                                raw.push((field, NodeRef::new(NodeKind::Worksheet, sid), kind))
                            }
                            None => gap(
                                &mut gaps,
                                "SHELF_REFERENCE",
                                n,
                                "Unresolved or ambiguous shelf field",
                            ),
                        }
                    }
                }
                Err(_) => gap(
                    &mut gaps,
                    "SHELF_EXPRESSION",
                    n,
                    "Shelf expression could not be analyzed",
                ),
            }
        }
        for &n in &xml.semantic.column_bindings {
            let Some(sheet) = xml.ancestor(n, "worksheet") else {
                continue;
            };
            let Some(&sid) = worksheet_ids.get(&sheet) else {
                continue;
            };
            if xml.tag(n) == "filter" {
                continue;
            }
            if xml.tag(n) == "calculation" {
                continue;
            }
            if xml.tag(n) == "manual-sort" {
                if let Some(column)=xml.value(n,"column")
                    && let Ok(analysis)=formula::analyze(column)
                    && analysis.references.len()==1
                    && let Some(field)=resolver.reference(Some(sheet),None,&analysis.references[0])
                {
                    raw.push((field,NodeRef::new(NodeKind::Worksheet,sid),EdgeKind::SortField));
                } else {
                    gap(&mut gaps,"SORT_REFERENCE",n,"Manual sort field is unresolved");
                }
                continue;
            }
            if xml.tag(n) == "computed-sort" {
                for (attribute, kind) in [
                    ("column", EdgeKind::SortField),
                    ("using", EdgeKind::SortUsing),
                ] {
                    let Some(expression) = xml.value(n, attribute) else {
                        gap(&mut gaps, "SORT_REFERENCE", n, "Sort field is missing");
                        continue;
                    };
                    match formula::analyze(expression) {
                        Ok(analysis) if analysis.references.len() == 1 => {
                            if let Some(field) =
                                resolver.reference(Some(sheet), None, &analysis.references[0])
                            {
                                raw.push((field, NodeRef::new(NodeKind::Worksheet, sid), kind));
                            } else {
                                gap(&mut gaps, "SORT_REFERENCE", n, "Unresolved sort field");
                            }
                        }
                        _ => gap(&mut gaps, "SORT_REFERENCE", n, "Ambiguous sort field"),
                    }
                }
                continue;
            }
            let kind = match xml.tag(n) {
                "color" => Some(EdgeKind::MarkColor),
                "size" => Some(EdgeKind::MarkSize),
                "text" => Some(EdgeKind::MarkText),
                "detail" | "lod" => Some(EdgeKind::MarkDetail),
                "shape" => Some(EdgeKind::MarkShape),
                "tooltip" => Some(EdgeKind::MarkTooltip),
                "wedge-size" => Some(EdgeKind::MarkWedgeSize),
                _ => None,
            };
            let Some(kind) = kind.filter(|_| xml.ancestor(n, "encodings").is_some()) else {
                gap(
                    &mut gaps,
                    "COLUMN_BINDING",
                    n,
                    "Column binding is outside supported mark encodings",
                );
                continue;
            };
            let Some(column) = xml.value(n, "column") else {
                continue;
            };
            match formula::analyze(column) {
                Ok(a) if a.references.len() == 1 => {
                    if let Some(field) = resolver.reference(Some(sheet), None, &a.references[0]) {
                        raw.push((field, NodeRef::new(NodeKind::Worksheet, sid), kind));
                    } else {
                        gap(
                            &mut gaps,
                            "MARK_REFERENCE",
                            n,
                            "Unresolved or ambiguous mark field",
                        );
                    }
                }
                _ => gap(
                    &mut gaps,
                    "MARK_REFERENCE",
                    n,
                    "Mark binding does not identify one field",
                ),
            }
        }
        for (n, filter, owner) in &filters {
            let (field_kind, owner_kind) = match filter.kind {
                NodeKind::WorksheetFilter => (
                    EdgeKind::WorksheetFilterField,
                    Some(EdgeKind::WorksheetFiltered),
                ),
                NodeKind::DatasourceFilter => (
                    EdgeKind::DatasourceFilterField,
                    Some(EdgeKind::DatasourceFiltered),
                ),
                NodeKind::ExtractFilter => (
                    EdgeKind::ExtractFilterField,
                    Some(EdgeKind::ExtractFiltered),
                ),
                NodeKind::SharedViewFilter => (EdgeKind::SharedViewFilterField, None),
                _ => unreachable!("only filters are collected"),
            };
            if let (Some(owner), Some(kind)) = (*owner, owner_kind) {
                raw.push((*filter, owner, kind));
            } else if filter.kind != NodeKind::SharedViewFilter {
                gap(&mut gaps, "FILTER_SCOPE", *n, "Filter owner is unresolved");
            }
            let Some(column) = xml.value(*n, "column") else {
                gap(
                    &mut gaps,
                    "FILTER_REFERENCE",
                    *n,
                    "Filter has no column reference",
                );
                continue;
            };
            match formula::analyze(column) {
                Ok(a) if a.references.len() == 1 => {
                    let sheet = xml.ancestor(*n, "worksheet");
                    let ds = (*owner)
                        .filter(|owner| owner.kind == NodeKind::Datasource)
                        .map(|owner| DatasourceId(owner.id))
                        .or_else(|| {
                            xml.ancestor(*n, "shared-view")
                                .and_then(|view| xml.value(view, "name"))
                                .and_then(|name| book.datasource_lookup.get(name).copied())
                        });
                    if let Some(field) = resolver.reference(sheet, ds, &a.references[0]) {
                        raw.push((field, *filter, field_kind));
                    } else {
                        gap(
                            &mut gaps,
                            "FILTER_REFERENCE",
                            *n,
                            "Unresolved or ambiguous filter field",
                        );
                    }
                }
                _ => gap(
                    &mut gaps,
                    "FILTER_REFERENCE",
                    *n,
                    "Filter does not identify one field",
                ),
            }
        }
        let worksheet_names: BTreeMap<&str, usize> = book
            .worksheets
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), i))
            .collect();
        for (i, _) in xml.nodes.iter().enumerate() {
            let n = NodeId(i as u32);
            if xml.tag(n) != "zone" {
                continue;
            }
            let Some(dashboard) = xml.ancestor(n, "dashboard") else {
                continue;
            };
            let Some(&did) = dashboard_ids.get(&dashboard) else {
                continue;
            };
            let Some(name) = xml.value(n, "name") else {
                continue;
            };
            let kind = xml.value(n, "type-v2");
            if !matches!(kind, None | Some("worksheet")) {
                continue;
            }
            if let Some(&sid) = worksheet_names.get(name) {
                raw.push((
                    NodeRef::new(NodeKind::Worksheet, sid),
                    NodeRef::new(NodeKind::Dashboard, did),
                    EdgeKind::SheetInDashboard,
                ));
            } else {
                gap(&mut gaps, "DASHBOARD_ZONE", n, "Unresolved worksheet zone");
            }
        }
        let mut details = vec![None; nodes.len()];
        extra::build(&book, &resolver, &worksheet_ids, &dashboard_ids)
            .finish(&mut nodes, &mut details, &mut raw, &mut gaps);
        let mut bases = [0usize; super::NODE_KIND_COUNT + 1];
        let mut last_kind = 0;
        for (index, node) in nodes.iter().enumerate() {
            let kind = node.reference.kind.index();
            require(kind >= last_kind, "INTERNAL", "Lineage nodes are out of kind order")?;
            while last_kind < kind { bases[last_kind + 1] = index; last_kind += 1; }
            require(node.reference.id as usize == index - bases[kind], "INTERNAL", "Lineage node ID mismatch")?;
        }
        while last_kind < super::NODE_KIND_COUNT { bases[last_kind + 1] = nodes.len(); last_kind += 1; }
        require(nodes.len() <= u32::MAX as usize, "LIMIT", "Too many lineage nodes")?;
        for (node, filter, _) in &filters {
            details[bases[filter.kind.index()] + filter.id as usize] = Some(extra::element(xml, *node, 0));
        }
        let mut links = Vec::with_capacity(raw.len());
        for (from, to, kind) in raw {
            let from = bases[from.kind.index()] + from.id as usize;
            let to = bases[to.kind.index()] + to.id as usize;
            links.push(Link {
                from: from as u32,
                to: to as u32,
                kind,
            });
        }
        links.sort_unstable_by_key(|e| (e.from, e.kind, e.to));
        links.dedup_by_key(|e| (e.from, e.kind, e.to));
        require(
            links.len() <= u32::MAX as usize,
            "LIMIT",
            "Too many lineage links",
        )?;
        let outgoing = offsets(nodes.len(), links.iter().map(|e| e.from));
        let mut incoming_order: Vec<u32> = (0..links.len() as u32).collect();
        incoming_order.sort_unstable_by_key(|&i| {
            (
                links[i as usize].to,
                links[i as usize].kind,
                links[i as usize].from,
            )
        });
        let incoming = offsets(
            nodes.len(),
            incoming_order.iter().map(|&i| links[i as usize].to),
        );
        let mut search_caption: Vec<_> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.caption.to_lowercase(), i as u32))
            .collect();
        search_caption.sort_unstable();
        let mut search_name: Vec<_> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.to_lowercase(), i as u32))
            .collect();
        search_name.sort_unstable();
        gaps.sort();
        gaps.dedup();
        for item in &mut gaps {
            let Some(id) = item
                .object
                .strip_prefix("xml_node:")
                .and_then(|id| id.parse::<u32>().ok())
                .filter(|&id| (id as usize) < xml.nodes.len())
            else {
                continue;
            };
            let node = NodeId(id);
            let mut context = Vec::new();
            if let Some(sheet) = xml.ancestor(node, "worksheet")
                && let Some(name) = xml.value(sheet, "name")
            {
                context.push(format!("worksheet={name}"));
            } else if let Some(view) = xml.ancestor(node, "shared-view")
                && let Some(name) = xml.value(view, "name")
            {
                context.push(format!("shared_view={name}"));
            } else if let Some(datasource) = xml.ancestor(node, "datasource")
                && let Some(name) = xml.value(datasource, "name")
            {
                context.push(format!("datasource={name}"));
            }
            context.push(format!("tag={}", xml.tag(node)));
            if let Some(reference) = xml
                .value(node, "column")
                .or_else(|| xml.value(node, "name"))
            {
                context.push(format!(
                    "reference={}",
                    reference.chars().take(160).collect::<String>()
                ));
            } else if matches!(xml.tag(node), "rows" | "cols")
                && let Ok(expression) = xml.text_content(node)
            {
                context.push(format!(
                    "expression={}",
                    expression.chars().take(160).collect::<String>()
                ));
            }
            item.context = Some(context.join("; "));
        }
        Ok(Self {
            input_sha256,
            source_build: book.source_build,
            nodes,
            bases,
            details,
            links,
            outgoing,
            incoming_order,
            incoming,
            search_caption,
            search_name,
            gaps,
        })
    }
}

fn field_ref(id: FieldId) -> NodeRef {
    NodeRef::new(NodeKind::Field, id.0 as usize)
}
fn gap(gaps: &mut Vec<Gap>, code: &str, node: NodeId, detail: &str) {
    gaps.push(Gap {
        code: code.into(),
        object: format!("xml_node:{}", node.0),
        detail: detail.into(),
        context: None,
    });
}
fn filter_node(xml: &crate::xml::Xml, n: NodeId, r: NodeRef, owner: Option<NodeRef>) -> GraphNode {
    let column = xml.value(n, "column").unwrap_or("");
    let class = xml.value(n, "class").unwrap_or("unknown");
    GraphNode {
        reference: r,
        name: column.into(),
        caption: format!("{class} filter: {column}"),
        datasource: owner.filter(|owner| owner.kind == NodeKind::Datasource),
        worksheet: owner.filter(|owner| owner.kind == NodeKind::Worksheet),
        formula: None,
        data_type: None,
        role: None,
    }
}
fn offsets(count: usize, ids: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut out = vec![0u32; count + 1];
    for id in ids {
        out[id as usize + 1] += 1;
    }
    for i in 1..out.len() {
        out[i] += out[i - 1];
    }
    out
}
type LocalLookup = BTreeMap<(NodeId, DatasourceId, String), BTreeSet<NodeRef>>;
type Instances = BTreeMap<(Option<NodeId>, DatasourceId, String), BTreeSet<NodeRef>>;
fn instances(book: &Workbook, local_lookup: &LocalLookup) -> Instances {
    let xml = &book.xml;
    let mut out = Instances::new();
    for &n in &xml.semantic.column_instances {
        let (Some(name), Some(column)) = (xml.value(n, "name"), xml.value(n, "column")) else {
            continue;
        };
        let sheet = xml.ancestor(n, "worksheet");
        let datasource_name = xml
            .ancestor(n, "datasource-dependencies")
            .and_then(|scope| xml.value(scope, "datasource"))
            .or_else(|| {
                xml.ancestor(n, "datasource")
                    .and_then(|scope| xml.value(scope, "name"))
            });
        let Some(&ds) = datasource_name.and_then(|s| book.datasource_lookup.get(s)) else {
            continue;
        };
        let mut targets = BTreeSet::new();
        if let Some(&field) = book.field_lookup[ds.0 as usize].get(column) {
            targets.insert(field_ref(field));
        } else if let Some(sheet) = sheet
            && let Some(local) = local_lookup.get(&(sheet, ds, column.into()))
        {
            targets.extend(local.iter().copied());
        }
        if !targets.is_empty() {
            out.entry((sheet, ds, name.into()))
                .or_default()
                .extend(targets);
        }
    }
    out
}
struct Resolver<'a> {
    book: &'a Workbook,
    instances: &'a Instances,
    local_lookup: &'a LocalLookup,
    sheet_sources: &'a BTreeMap<NodeId, BTreeSet<DatasourceId>>,
}
impl Resolver<'_> {
    fn reference(
        &self,
        sheet: Option<NodeId>,
        datasource: Option<DatasourceId>,
        r: &formula::Reference,
    ) -> Option<NodeRef> {
        let ds = if let Some(name) = &r.datasource {
            Some(*self.book.datasource_lookup.get(name)?)
        } else {
            datasource
        };
        if let Some(ds) = ds {
            return self.in_source(sheet, ds, &r.field);
        }
        let sheet = sheet?;
        let mut found = None;
        for ds in self.sheet_sources.get(&sheet)? {
            if let Some(field) = self.in_source(Some(sheet), *ds, &r.field)
                && found.replace(field).is_some()
            {
                return None;
            }
        }
        found
    }
    fn in_source(&self, sheet: Option<NodeId>, ds: DatasourceId, name: &str) -> Option<NodeRef> {
        self.book.field_lookup[ds.0 as usize]
            .get(name)
            .copied()
            .map(field_ref)
            .or_else(|| {
                let ids = self.local_lookup.get(&(sheet?, ds, name.into()))?;
                if ids.len() == 1 {
                    ids.iter().next().copied()
                } else {
                    None
                }
            })
            .or_else(|| {
                let ids = self.instances.get(&(sheet, ds, name.into()))?;
                if ids.len() == 1 {
                    ids.iter().next().copied()
                } else {
                    None
                }
            })
            .or_else(|| {
                let inner = name.strip_prefix('[')?.strip_suffix(']')?;
                let (prefix, rest) = inner.split_once(':')?;
                if !matches!(prefix, "none" | "usr" | "sum" | "tmn" | "yr") { return None; }
                let (field, suffix) = rest.rsplit_once(':')?;
                if !matches!(suffix, "nk" | "ok" | "qk") { return None; }
                self.book.field_lookup[ds.0 as usize]
                    .get(&format!("[{field}]"))
                    .copied().map(field_ref)
            })
    }
}
