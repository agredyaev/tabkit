use super::{Extra, Resolver, source::descendants};
use crate::workbook::lineage::{EdgeKind, NodeKind, NodeRef};
use crate::{
    formula,
    workbook::{DatasourceId, FieldId, Workbook},
    xml::{NodeId, Xml},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn build(
    book: &Workbook,
    resolver: &Resolver<'_>,
    sheets: &BTreeMap<NodeId, usize>,
    dashboards: &BTreeMap<NodeId, usize>,
    out: &mut Extra,
) {
    let xml = &book.xml;
    let sheet_names: BTreeMap<_, _> = book
        .worksheets
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), NodeRef::new(NodeKind::Worksheet, i)))
        .collect();
    let dash_names: BTreeMap<_, _> = book
        .dashboards
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), NodeRef::new(NodeKind::Dashboard, i)))
        .collect();
    let mut groups = BTreeMap::<(DatasourceId, String), NodeRef>::new();
    let mut ambiguous_groups = BTreeSet::new();
    if let Ok(root) = xml.one_child(NodeId(0), "datasources") {
        for (i, ds_node) in xml.named_children(root, "datasource").enumerate() {
            let ds = DatasourceId(i as u32);
            let owner = NodeRef::new(NodeKind::Datasource, i);
            for group in xml.named_children(ds_node, "group") {
                let Some(name) = xml.value(group, "name") else {
                    out.gap("GROUP_NAME", group, "Group has no name");
                    continue;
                };
                let kind = if xml
                    .attributes(group)
                    .any(|a| a.name.ends_with("ui-builder") && a.value == "filter-group")
                {
                    NodeKind::Set
                } else {
                    NodeKind::Group
                };
                let reference=out.add(kind,name,xml.value(group,"caption").unwrap_or(name),Some(owner),None,
                    json!({"definition":group_definition(xml,group),"hidden":xml.value(group,"hidden")==Some("true")}));
                let key = (ds, name.to_owned());
                if ambiguous_groups.contains(&key) {
                    out.gap("GROUP_REFERENCE", group, "Duplicate set or group name");
                } else if groups.insert(key.clone(), reference).is_some() {
                    groups.remove(&key);
                    ambiguous_groups.insert(key);
                    out.gap("GROUP_REFERENCE", group, "Duplicate set or group name");
                }
                for node in descendants(xml, group) {
                    if xml.tag(node) != "groupfilter" {
                        continue;
                    }
                    for attr in ["level", "member", "expression", "count"] {
                        if let Some(text) = xml.value(node, attr) {
                            for source in references(resolver, None, Some(ds), text) {
                                out.edge(source, reference, EdgeKind::GroupInput);
                            }
                        }
                    }
                }
            }
        }
    }
    for (i, field) in book.fields.iter().enumerate() {
        let Some(formula) = field.formula.as_deref() else {
            continue;
        };
        if let Ok(analysis) = formula::analyze(formula) {
            for input in analysis.references {
                if let Some(group) = group_reference(
                    book,
                    resolver,
                    &groups,
                    None,
                    Some(field.datasource),
                    &input,
                ) {
                    out.edge(group, NodeRef::new(NodeKind::Field, i), EdgeKind::GroupUse);
                    out.resolved_diagnostic(
                        "UNRESOLVED_REFERENCE",
                        book.field_key(FieldId(i as u32)),
                        format!("Use an inspected internal field name: {}", input.field),
                    );
                }
            }
        }
    }
    let mut filter_ids = [0usize; 4];
    for &node in &xml.semantic.filters {
        let (kind, slot, ds) = if let Some(sheet) = xml.ancestor(node, "worksheet") {
            let ds = xml
                .ancestor(node, "datasource-dependencies")
                .and_then(|n| xml.value(n, "datasource"))
                .and_then(|n| book.datasource_lookup.get(n))
                .copied();
            let _ = sheet;
            (NodeKind::WorksheetFilter, 0, ds)
        } else if xml.ancestor(node, "extract").is_some() {
            (
                NodeKind::ExtractFilter,
                2,
                xml.ancestor(node, "datasource")
                    .and_then(|n| xml.value(n, "name"))
                    .and_then(|n| book.datasource_lookup.get(n))
                    .copied(),
            )
        } else if let Some(source) = xml.ancestor(node, "datasource") {
            (
                NodeKind::DatasourceFilter,
                1,
                xml.value(source, "name")
                    .and_then(|n| book.datasource_lookup.get(n))
                    .copied(),
            )
        } else if xml.ancestor(node, "shared-view").is_some() {
            (
                NodeKind::SharedViewFilter,
                3,
                xml.ancestor(node, "shared-view")
                    .and_then(|n| xml.value(n, "name"))
                    .and_then(|n| book.datasource_lookup.get(n))
                    .copied(),
            )
        } else {
            continue;
        };
        let target = NodeRef::new(kind, filter_ids[slot]);
        filter_ids[slot] += 1;
        if let Some(column) = xml.value(node, "column")
            && let Ok(a) = formula::analyze(column)
            && a.references.len() == 1
            && let Some(group) = group_reference(
                book,
                resolver,
                &groups,
                xml.ancestor(node, "worksheet"),
                ds,
                &a.references[0],
            )
        {
            out.edge(group, target, EdgeKind::GroupUse);
            out.edge(
                group,
                target,
                match kind {
                    NodeKind::WorksheetFilter => EdgeKind::WorksheetFilterField,
                    NodeKind::DatasourceFilter => EdgeKind::DatasourceFilterField,
                    NodeKind::ExtractFilter => EdgeKind::ExtractFilterField,
                    _ => EdgeKind::SharedViewFilterField,
                },
            );
            out.resolved("FILTER_REFERENCE", node);
        }
    }
    for &node in &xml.semantic.shelves {
        let Some(sheet) = xml.ancestor(node, "worksheet") else {
            continue;
        };
        let Some(&sid) = sheets.get(&sheet) else {
            continue;
        };
        let Ok(text) = xml.text_content(node) else {
            continue;
        };
        let Ok(analysis) = formula::analyze(text) else {
            continue;
        };
        let mut has_group = false;
        let mut all_known = true;
        for field in &analysis.references {
            if let Some(group) = group_reference(book, resolver, &groups, Some(sheet), None, field)
            {
                has_group = true;
                out.edge(
                    group,
                    NodeRef::new(NodeKind::Worksheet, sid),
                    if xml.tag(node) == "rows" {
                        EdgeKind::Rows
                    } else {
                        EdgeKind::Cols
                    },
                );
            } else if resolver.reference(Some(sheet), None, field).is_none() {
                all_known = false;
            }
        }
        if has_group && all_known {
            out.resolved("SHELF_REFERENCE", node);
        }
    }
    for &node in &xml.semantic.column_bindings {
        let Some(sheet) = xml.ancestor(node, "worksheet") else {
            continue;
        };
        let Some(&sid) = sheets.get(&sheet) else {
            continue;
        };
        let Some(column) = xml.value(node, "column") else {
            continue;
        };
        let Ok(analysis) = formula::analyze(column) else {
            continue;
        };
        if analysis.references.len() != 1 {
            continue;
        }
        let Some(group) = group_reference(
            book,
            resolver,
            &groups,
            Some(sheet),
            None,
            &analysis.references[0],
        ) else {
            continue;
        };
        let kind = match xml.tag(node) {
            "color" => EdgeKind::MarkColor,
            "size" => EdgeKind::MarkSize,
            "text" => EdgeKind::MarkText,
            "detail" | "lod" => EdgeKind::MarkDetail,
            "shape" => EdgeKind::MarkShape,
            "tooltip" => EdgeKind::MarkTooltip,
            "wedge-size" => EdgeKind::MarkWedgeSize,
            "manual-sort" => EdgeKind::SortField,
            _ => continue,
        };
        if kind != EdgeKind::SortField && xml.ancestor(node, "encodings").is_none() {
            continue;
        }
        out.edge(group, NodeRef::new(NodeKind::Worksheet, sid), kind);
        out.resolved(
            if kind == EdgeKind::SortField {
                "SORT_REFERENCE"
            } else {
                "MARK_REFERENCE"
            },
            node,
        );
    }
    if let Ok(actions) = xml.one_child(NodeId(0), "actions") {
        for action in xml.children(actions) {
            let tag = xml.tag(action);
            if tag != "action" && !tag.ends_with("-action") {
                continue;
            }
            let name = xml.value(action, "name").unwrap_or(tag);
            let source = xml.named_children(action, "source").next();
            let activation = xml
                .named_children(action, "activation")
                .next()
                .and_then(|n| xml.value(n, "type"));
            let parameters: Vec<_> = descendants(xml, action)
                .into_iter()
                .filter(|&n| xml.tag(n) == "param")
                .map(|n| json!({"name":xml.value(n,"name"),"value":xml.value(n,"value")}))
                .collect();
            let operations: Vec<_> = xml
                .children(action)
                .filter(|&n| !matches!(xml.tag(n), "activation" | "source" | "params"))
                .map(|n| element(xml, n, 0))
                .collect();
            let source_detail=source.map(|n|json!({"type":xml.value(n,"type"),"worksheet":xml.value(n,"worksheet"),
                "dashboard":xml.value(n,"dashboard"),"datasource":xml.value(n,"datasource"),
                "excluded_sheets":xml.named_children(n,"exclude-sheet").filter_map(|e|xml.value(e,"name")).collect::<Vec<_>>()}));
            let reference = out.add(
                NodeKind::Action,
                name,
                xml.value(action, "caption").unwrap_or(name),
                None,
                None,
                json!({"type":tag,"activation":activation,"source":source_detail,
                    "parameters":parameters,"operations":operations}),
            );
            let mut source_sheet = None;
            let mut source_ds = None;
            if let Some(src) = source {
                if let Some(name) = xml.value(src, "worksheet") {
                    if let Some(&sheet) = sheet_names.get(name) {
                        out.edge(sheet, reference, EdgeKind::ActionSource);
                        source_sheet = sheets
                            .iter()
                            .find(|(_, i)| **i == sheet.id as usize)
                            .map(|(n, _)| *n);
                    } else {
                        out.gap("ACTION_SOURCE", src, "Action worksheet is unresolved");
                    }
                } else if let Some(name) = xml.value(src, "dashboard") {
                    if let Some(&dash) = dash_names.get(name) {
                        out.edge(dash, reference, EdgeKind::ActionSource);
                    } else {
                        out.gap("ACTION_SOURCE", src, "Action dashboard is unresolved");
                    }
                } else if let Some(name) = xml.value(src, "datasource") {
                    if let Some(&id) = book.datasource_lookup.get(name) {
                        source_ds = Some(id);
                        out.edge(
                            NodeRef::new(NodeKind::Datasource, id.0 as usize),
                            reference,
                            EdgeKind::ActionSource,
                        );
                    } else {
                        out.gap("ACTION_SOURCE", src, "Action datasource is unresolved");
                    }
                }
            } else {
                out.gap("ACTION_SOURCE", action, "Action has no source");
            }
            for param in descendants(xml, action)
                .into_iter()
                .filter(|&n| xml.tag(n) == "param")
            {
                match xml.value(param, "name") {
                    Some("source-field") => {
                        if let Some(value) = xml.value(param, "value") {
                            let refs = references(resolver, source_sheet, source_ds, value);
                            if refs.is_empty() {
                                out.gap("ACTION_INPUT", param, "Action source field is unresolved");
                            }
                            for field in refs {
                                out.edge(field, reference, EdgeKind::ActionInput);
                            }
                        }
                    }
                    Some("target-parameter") => {
                        if let Some(value) = xml.value(param, "value") {
                            let refs = references(resolver, None, None, value);
                            if refs.len() != 1 {
                                out.gap("ACTION_TARGET", param, "Action parameter is unresolved");
                            }
                            for field in refs {
                                out.edge(reference, field, EdgeKind::ActionTarget);
                            }
                        }
                    }
                    Some("target-group") => {
                        if let Some(value) = xml.value(param, "value") {
                            let target = formula::analyze(value)
                                .ok()
                                .and_then(|a| a.references.into_iter().next())
                                .and_then(|r| {
                                    book.datasource_lookup
                                        .get(r.datasource.as_deref()?)
                                        .copied()
                                        .map(|ds| (ds, r.field))
                                })
                                .and_then(|key| groups.get(&key).copied());
                            if let Some(target) = target {
                                out.edge(reference, target, EdgeKind::ActionTarget);
                            } else {
                                out.gap("ACTION_TARGET", param, "Action set is unresolved");
                            }
                        }
                    }
                    _ => {}
                }
            }
            for link in xml.named_children(action, "link") {
                if let Some(value) = xml.value(link, "expression") {
                    for token in tokens(value) {
                        for field in references(resolver, source_sheet, source_ds, token) {
                            out.edge(field, reference, EdgeKind::ActionInput);
                        }
                    }
                }
            }
            for target in descendants(xml, action)
                .into_iter()
                .filter(|&n| xml.tag(n) == "target")
            {
                let name = xml
                    .value(target, "worksheet")
                    .or_else(|| xml.value(target, "dashboard"))
                    .or_else(|| xml.value(target, "name"));
                if let Some(name) = name {
                    if let Some(&sheet) = sheet_names.get(name) {
                        out.edge(reference, sheet, EdgeKind::ActionTarget);
                    } else if let Some(&dash) = dash_names.get(name) {
                        out.edge(reference, dash, EdgeKind::ActionTarget);
                    } else {
                        out.gap(
                            "ACTION_TARGET",
                            target,
                            "Action sheet or dashboard target is unresolved",
                        );
                    }
                }
            }
            for command in xml.named_children(action, "command") {
                for param in xml.named_children(command, "param") {
                    if xml.value(param, "name") != Some("target") {
                        continue;
                    }
                    if let Some(name) = xml.value(param, "value") {
                        if let Some(&sheet) = sheet_names.get(name) {
                            out.edge(reference, sheet, EdgeKind::ActionTarget);
                        } else if let Some(&dash) = dash_names.get(name) {
                            out.edge(reference, dash, EdgeKind::ActionTarget);
                        } else {
                            out.gap(
                                "ACTION_TARGET",
                                param,
                                "Action command target is unresolved",
                            );
                        }
                    }
                }
            }
        }
    }
    for (index, _) in xml.nodes.iter().enumerate() {
        let node = NodeId(index as u32);
        match xml.tag(node) {
            "custom" if xml.ancestor(node, "encodings").is_some() => {
                let Some(sheet) = xml.ancestor(node, "worksheet") else {
                    out.gap("CUSTOM_ENCODING_SCOPE", node, "Custom encoding is outside a worksheet");
                    continue;
                };
                let Some(&sid) = sheets.get(&sheet) else {
                    out.gap("CUSTOM_ENCODING_SCOPE", node, "Custom encoding worksheet is unresolved");
                    continue;
                };
                let owner = NodeRef::new(NodeKind::Worksheet, sid);
                let kind = xml.value(node, "custom-type-name").unwrap_or("custom");
                let column = xml.value(node, "column").unwrap_or("");
                let attributes: BTreeMap<_, _> = xml.attributes(node)
                    .map(|a| (a.name.to_owned(), a.value.to_owned())).collect();
                let reference = out.add(NodeKind::CustomEncoding, column, &format!("{kind} encoding"), None,
                    Some(owner), json!({"custom_type":kind,"column":column,"attributes":attributes}));
                out.edge(reference, owner, EdgeKind::CustomEncodingOnSheet);
                let source = formula::analyze(column).ok()
                    .filter(|a| a.references.len() == 1)
                    .and_then(|a| resolver.reference(Some(sheet), None, &a.references[0])
                        .or_else(|| group_reference(book, resolver, &groups, Some(sheet), None, &a.references[0])));
                if let Some(source) = source {
                    out.edge(source, reference, EdgeKind::CustomEncodingField);
                } else {
                    out.gap("CUSTOM_ENCODING_FIELD", node, "Custom encoding field is unresolved");
                }
                out.resolved("COLUMN_BINDING", node);
            }
            "customized-tooltip" => {
                let Some(sheet) = xml.ancestor(node, "worksheet") else {
                    continue;
                };
                let Some(&sid) = sheets.get(&sheet) else {
                    continue;
                };
                let owner = NodeRef::new(NodeKind::Worksheet, sid);
                let mut runs = Vec::new();
                let mut embedded = Vec::new();
                for run in descendants(xml, node)
                    .into_iter()
                    .filter(|&n| xml.tag(n) == "run")
                {
                    let Ok(text) = xml.text_content(run) else {
                        continue;
                    };
                    runs.push(text);
                    if text.starts_with("<Sheet ") {
                        if let Some(name) = tag_attribute(text, "name") {
                            embedded.push((
                                name.to_owned(),
                                tag_attribute(text, "filter").map(str::to_owned),
                            ));
                        } else {
                            out.gap("TOOLTIP_SHEET", node, "Embedded tooltip sheet has no name");
                        }
                    }
                }
                let reference = out.add(
                    NodeKind::Tooltip,
                    &format!("{} tooltip {}", book.worksheets[sid], node.0),
                    &format!("{} tooltip", book.worksheets[sid]),
                    None,
                    Some(owner),
                    json!({"runs":runs,"embedded_sheets":embedded}),
                );
                out.edge(reference, owner, EdgeKind::TooltipOnSheet);
                for text in &runs {
                    if text.starts_with("<Sheet ") {
                        continue;
                    }
                    for token in tokens(text) {
                        let refs = references(resolver, Some(sheet), None, token);
                        if refs.is_empty() && token.starts_with('[') {
                            out.gap("TOOLTIP_REFERENCE", node, "Tooltip field is unresolved");
                        }
                        for field in refs {
                            out.edge(field, reference, EdgeKind::TooltipInput);
                        }
                    }
                }
                for (name, _) in embedded {
                    if let Some(&target) = sheet_names.get(name.as_str()) {
                        out.edge(reference, target, EdgeKind::TooltipSheet);
                    } else {
                        out.gap(
                            "TOOLTIP_SHEET",
                            node,
                            "Embedded tooltip sheet is unresolved",
                        );
                    }
                }
            }
            "zone"
                if matches!(
                    xml.value(node, "type-v2")
                        .or_else(|| xml.value(node, "type")),
                    Some("paramctrl")
                ) =>
            {
                let Some(dashboard) = xml.ancestor(node, "dashboard") else {
                    out.gap(
                        "PARAMETER_CONTROL_SCOPE",
                        node,
                        "Parameter control is outside a dashboard",
                    );
                    continue;
                };
                let Some(&did) = dashboards.get(&dashboard) else {
                    continue;
                };
                let owner = NodeRef::new(NodeKind::Dashboard, did);
                let parameter = xml.value(node, "param").unwrap_or("");
                let reference=out.add(NodeKind::ParameterControl,parameter,parameter,None,None,
                    json!({"mode":xml.value(node,"mode"),"parameter":parameter,"zone_id":xml.value(node,"id")}));
                out.edge(reference, owner, EdgeKind::ControlOnDashboard);
                let refs = references(resolver, None, None, parameter);
                if refs.len() != 1 {
                    out.gap(
                        "PARAMETER_CONTROL",
                        node,
                        "Parameter control target is unresolved",
                    );
                }
                for field in refs {
                    out.edge(field, reference, EdgeKind::ControlParameter);
                }
            }
            _ => {}
        }
    }
    if let Ok(stories) = xml.one_child(NodeId(0), "stories") {
        for story in xml.named_children(stories, "story") {
            let name = xml.value(story, "name").unwrap_or("story");
            add_story(xml, story, name, None, &sheet_names, &dash_names, out);
        }
    }
    for (index, _) in xml.nodes.iter().enumerate() {
        let flipboard = NodeId(index as u32);
        if xml.tag(flipboard) != "flipboard" || xml.ancestor(flipboard, "story").is_some() {
            continue;
        }
        let Some(dashboard) = xml.ancestor(flipboard, "dashboard") else {
            continue;
        };
        let Some(&did) = dashboards.get(&dashboard) else {
            continue;
        };
        let name = xml.value(dashboard, "name").unwrap_or("story board");
        add_story(
            xml,
            flipboard,
            name,
            Some(NodeRef::new(NodeKind::Dashboard, did)),
            &sheet_names,
            &dash_names,
            out,
        );
    }
    for (index, _) in xml.nodes.iter().enumerate() {
        let calc = NodeId(index as u32);
        if xml.tag(calc) != "table-calc" {
            continue;
        }
        let sheet = xml.ancestor(calc, "worksheet");
        let worksheet = sheet
            .and_then(|n| sheets.get(&n).copied())
            .map(|i| NodeRef::new(NodeKind::Worksheet, i));
        let parent = xml
            .ancestor(calc, "column-instance")
            .or_else(|| xml.ancestor(calc, "column"));
        let name = parent
            .and_then(|p| xml.value(p, "name"))
            .unwrap_or("table calculation");
        let column = parent.and_then(|p| xml.value(p, "column")).unwrap_or(name);
        let ds = xml
            .ancestor(calc, "datasource-dependencies")
            .and_then(|n| xml.value(n, "datasource"))
            .or_else(|| {
                xml.ancestor(calc, "datasource")
                    .and_then(|n| xml.value(n, "name"))
            })
            .and_then(|n| book.datasource_lookup.get(n))
            .copied();
        let addresses: Vec<_> = descendants(xml, calc)
            .into_iter()
            .filter(|&n| xml.tag(n) == "address")
            .flat_map(|n| xml.named_children(n, "value"))
            .filter_map(|n| xml.text_content(n).ok())
            .collect();
        let partitions: Vec<_> = descendants(xml, calc)
            .into_iter()
            .filter(|&n| xml.tag(n) == "partition")
            .flat_map(|n| xml.named_children(n, "value"))
            .filter_map(|n| xml.text_content(n).ok())
            .collect();
        let order: Vec<_> = descendants(xml, calc)
            .into_iter()
            .filter(|&n| xml.tag(n) == "order")
            .filter_map(|n| xml.value(n, "field"))
            .collect();
        let attrs: BTreeMap<_, _> = xml
            .attributes(calc)
            .map(|a| (a.name.to_owned(), a.value.to_owned()))
            .collect();
        let reference = out.add(
            NodeKind::TableCalculation,
            name,
            name,
            ds.map(|d| NodeRef::new(NodeKind::Datasource, d.0 as usize)),
            worksheet,
            json!({"column":column,"attributes":attrs,"address":addresses,
                "partition":partitions,"order":order,"configuration":element(xml,calc,0)}),
        );
        let inputs = references(resolver, sheet, ds, column);
        if inputs.len() != 1 {
            out.gap(
                "TABLE_CALC_INPUT",
                calc,
                "Table calculation input is unresolved",
            );
        } else {
            out.edge(inputs[0], reference, EdgeKind::TableCalculationInput);
        }
        for field_name in order {
            let mut refs = references(resolver, sheet, ds, field_name);
            if refs.is_empty()
                && let Ok(analysis) = formula::analyze(field_name)
                && analysis.references.len() == 1
                && let Some(group) =
                    group_reference(book, resolver, &groups, sheet, ds, &analysis.references[0])
            {
                refs.push(group);
            }
            if refs.len() != 1 {
                out.gap(
                    "TABLE_CALC_ORDER",
                    calc,
                    "Table calculation order field is unresolved",
                );
            } else {
                out.edge(refs[0], reference, EdgeKind::TableCalculationOrder);
            }
        }
        if let Some(owner) = worksheet {
            out.edge(reference, owner, EdgeKind::TableCalculationOnSheet);
        }
    }
}

fn references(
    resolver: &Resolver<'_>,
    sheet: Option<NodeId>,
    ds: Option<DatasourceId>,
    text: &str,
) -> Vec<NodeRef> {
    formula::analyze(text)
        .ok()
        .map(|a| {
            a.references
                .into_iter()
                .filter_map(|r| resolver.reference(sheet, ds, &r))
                .collect()
        })
        .unwrap_or_default()
}
fn group_reference(
    book: &Workbook,
    resolver: &Resolver<'_>,
    groups: &BTreeMap<(DatasourceId, String), NodeRef>,
    sheet: Option<NodeId>,
    ds: Option<DatasourceId>,
    reference: &formula::Reference,
) -> Option<NodeRef> {
    let name = if let Some(inner) = reference
        .field
        .strip_prefix("[io:")
        .and_then(|s| s.strip_suffix(']'))
    {
        let (name, suffix) = inner.rsplit_once(':')?;
        if !matches!(suffix, "nk" | "ok" | "qk") {
            return None;
        }
        format!("[{name}]")
    } else {
        reference.field.clone()
    };
    if let Some(ds_name) = reference.datasource.as_deref() {
        let ds = *book.datasource_lookup.get(ds_name)?;
        return groups.get(&(ds, name)).copied();
    }
    if let Some(ds) = ds {
        return groups.get(&(ds, name)).copied();
    }
    let mut found = None;
    for ds in resolver.sheet_sources.get(&sheet?)? {
        if let Some(&group) = groups.get(&(*ds, name.clone()))
            && found.replace(group).is_some()
        {
            return None;
        }
    }
    found
}
fn tokens(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut rest = text;
    while let Some(begin) = rest.find('<') {
        rest = &rest[begin + 1..];
        let Some(end) = rest.find('>') else {
            break;
        };
        result.push(&rest[..end]);
        rest = &rest[end + 1..];
    }
    result
}
fn tag_attribute<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let tail = text.split_once(&needle)?.1;
    tail.split_once('"').map(|(value, _)| value)
}
fn group_definition(xml: &Xml, group: NodeId) -> Value {
    fn one(xml: &Xml, node: NodeId, depth: usize) -> Value {
        if depth == 64 {
            return json!({"depth_limit":true});
        }
        json!({"attributes":xml.attributes(node).map(|a|(a.name.to_owned(),a.value.to_owned())).collect::<BTreeMap<_,_>>(),
            "children":xml.named_children(node,"groupfilter").map(|n|one(xml,n,depth+1)).collect::<Vec<_>>()})
    }
    json!(
        xml.named_children(group, "groupfilter")
            .map(|n| one(xml, n, 0))
            .collect::<Vec<_>>()
    )
}
fn element(xml: &Xml, node: NodeId, depth: usize) -> Value {
    if depth == 32 {
        return json!({"depth_limit":true});
    }
    json!({"tag":xml.tag(node),
        "attributes":xml.attributes(node).map(|a|(a.name.to_owned(),a.value.to_owned())).collect::<BTreeMap<_,_>>(),
        "text":xml.text_content(node).ok().filter(|s|!s.is_empty()),
        "children":xml.children(node).map(|n|element(xml,n,depth+1)).collect::<Vec<_>>()})
}
fn add_story(
    xml: &Xml,
    root: NodeId,
    name: &str,
    dashboard: Option<NodeRef>,
    sheet_names: &BTreeMap<&str, NodeRef>,
    dash_names: &BTreeMap<&str, NodeRef>,
    out: &mut Extra,
) {
    let story = out.add(
        NodeKind::Story,
        name,
        name,
        None,
        None,
        json!({"name":name,"dashboard":dashboard,"active_point":xml.value(root,"active-id")}),
    );
    if let Some(dashboard) = dashboard {
        out.edge(story, dashboard, EdgeKind::StoryOnDashboard);
    }
    for point in descendants(xml, root)
        .into_iter()
        .filter(|&n| xml.tag(n) == "story-point")
    {
        let caption = xml.value(point, "caption").unwrap_or("story point");
        let target = xml.value(point, "captured-sheet");
        let reference = out.add(
            NodeKind::StoryPoint,
            xml.value(point, "id").unwrap_or(caption),
            caption,
            None,
            None,
            json!({"caption":caption,"captured_sheet":target}),
        );
        out.edge(reference, story, EdgeKind::StoryContains);
        if let Some(name) = target {
            match (sheet_names.get(name), dash_names.get(name)) {
                (Some(&sheet), None) => out.edge(sheet, reference, EdgeKind::StoryPointSheet),
                (None, Some(&dash)) => out.edge(dash, reference, EdgeKind::StoryPointSheet),
                _ => out.gap(
                    "STORY_TARGET",
                    point,
                    "Story point target is ambiguous or unresolved",
                ),
            }
        } else {
            out.gap("STORY_TARGET", point, "Story point has no captured sheet");
        }
    }
}
