use super::{EdgeKind, Gap, Graph, GraphNode, Link, NodeKind, NodeRef};
use crate::{
    error::{Error, Result, require},
    formula,
    workbook::{DatasourceId, FieldId, Workbook},
    xml::NodeId,
};
use std::collections::{BTreeMap, BTreeSet};

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
            })
            .collect();
        gaps.extend(book.local_definitions.iter().map(|d| Gap {
            code: "LOCAL_DEFINITION".into(),
            object: format!("xml_node:{}", d.node_id.0),
            detail: d.reason.clone(),
        }));
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
                gap(
                    &mut gaps,
                    "EXTRACT_FILTER",
                    n,
                    "Extract filter is outside v1 datasource-filter lineage",
                );
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
            } else {
                gap(
                    &mut gaps,
                    "FILTER_SCOPE",
                    n,
                    "Filter scope is not a worksheet or datasource",
                );
            }
        }
        let counts = [
            book.fields.len(),
            book.datasources.len(),
            worksheet_filter_count,
            datasource_filter_count,
            book.worksheets.len(),
            book.dashboards.len(),
        ];
        let mut bases = [0usize; 7];
        for (i, count) in counts.into_iter().enumerate() {
            require(
                count <= u32::MAX as usize,
                "LIMIT",
                "Too many lineage nodes of one kind",
            )?;
            bases[i + 1] = bases[i]
                .checked_add(count)
                .ok_or_else(|| Error::new("LIMIT", "Lineage node count overflow"))?;
        }
        require(
            bases[6] <= u32::MAX as usize,
            "LIMIT",
            "Too many lineage nodes",
        )?;
        let mut nodes = Vec::with_capacity(bases[6]);
        for (i, f) in book.fields.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Field, i),
                name: f.name.clone(),
                caption: f.caption.clone(),
                datasource: Some(NodeRef::new(NodeKind::Datasource, f.datasource.0 as usize)),
            });
        }
        for (i, d) in book.datasources.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Datasource, i),
                name: d.name.clone(),
                caption: d.caption.clone(),
                datasource: None,
            });
        }
        for (n, r, _) in filters
            .iter()
            .filter(|(_, r, _)| r.kind == NodeKind::WorksheetFilter)
        {
            nodes.push(filter_node(xml, *n, *r));
        }
        for (n, r, _) in filters
            .iter()
            .filter(|(_, r, _)| r.kind == NodeKind::DatasourceFilter)
        {
            nodes.push(filter_node(xml, *n, *r));
        }
        for (i, name) in book.worksheets.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Worksheet, i),
                name: name.clone(),
                caption: name.clone(),
                datasource: None,
            });
        }
        for (i, name) in book.dashboards.iter().enumerate() {
            nodes.push(GraphNode {
                reference: NodeRef::new(NodeKind::Dashboard, i),
                name: name.clone(),
                caption: name.clone(),
                datasource: None,
            });
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
        let instances = instances(&book);
        let resolver = Resolver {
            book: &book,
            instances: &instances,
            sheet_sources: &sheet_sources,
        };
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
                            Some(fid) => raw.push((
                                field_ref(fid),
                                NodeRef::new(NodeKind::Worksheet, sid),
                                kind,
                            )),
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
            let kind = match xml.tag(n) {
                "color" => Some(EdgeKind::MarkColor),
                "size" => Some(EdgeKind::MarkSize),
                "text" => Some(EdgeKind::MarkText),
                "detail" | "lod" => Some(EdgeKind::MarkDetail),
                "shape" => Some(EdgeKind::MarkShape),
                "tooltip" => Some(EdgeKind::MarkTooltip),
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
                    if let Some(fid) = resolver.reference(Some(sheet), None, &a.references[0]) {
                        raw.push((field_ref(fid), NodeRef::new(NodeKind::Worksheet, sid), kind));
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
            let Some(owner) = owner else {
                gap(&mut gaps, "FILTER_SCOPE", *n, "Filter owner is unresolved");
                continue;
            };
            let (field_kind, owner_kind) = if filter.kind == NodeKind::WorksheetFilter {
                (EdgeKind::WorksheetFilterField, EdgeKind::WorksheetFiltered)
            } else {
                (
                    EdgeKind::DatasourceFilterField,
                    EdgeKind::DatasourceFiltered,
                )
            };
            raw.push((*filter, *owner, owner_kind));
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
                    let ds = if owner.kind == NodeKind::Datasource {
                        Some(DatasourceId(owner.id))
                    } else {
                        None
                    };
                    if let Some(fid) = resolver.reference(sheet, ds, &a.references[0]) {
                        raw.push((field_ref(fid), *filter, field_kind));
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
        Ok(Self {
            input_sha256,
            source_build: book.source_build,
            nodes,
            bases,
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
    });
}
fn filter_node(xml: &crate::xml::Xml, n: NodeId, r: NodeRef) -> GraphNode {
    let column = xml.value(n, "column").unwrap_or("");
    let class = xml.value(n, "class").unwrap_or("unknown");
    GraphNode {
        reference: r,
        name: column.into(),
        caption: format!("{class} filter: {column}"),
        datasource: None,
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
type Instances = BTreeMap<(NodeId, DatasourceId, String), BTreeSet<FieldId>>;
fn instances(book: &Workbook) -> Instances {
    let xml = &book.xml;
    let mut out = Instances::new();
    for &n in &xml.semantic.column_instances {
        let (Some(sheet), Some(scope), Some(name), Some(column)) = (
            xml.ancestor(n, "worksheet"),
            xml.ancestor(n, "datasource-dependencies"),
            xml.value(n, "name"),
            xml.value(n, "column"),
        ) else {
            continue;
        };
        let Some(&ds) = xml
            .value(scope, "datasource")
            .and_then(|s| book.datasource_lookup.get(s))
        else {
            continue;
        };
        if let Some(&field) = book.field_lookup[ds.0 as usize].get(column) {
            out.entry((sheet, ds, name.into()))
                .or_default()
                .insert(field);
        }
    }
    out
}
struct Resolver<'a> {
    book: &'a Workbook,
    instances: &'a Instances,
    sheet_sources: &'a BTreeMap<NodeId, BTreeSet<DatasourceId>>,
}
impl Resolver<'_> {
    fn reference(
        &self,
        sheet: Option<NodeId>,
        datasource: Option<DatasourceId>,
        r: &formula::Reference,
    ) -> Option<FieldId> {
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
    fn in_source(&self, sheet: Option<NodeId>, ds: DatasourceId, name: &str) -> Option<FieldId> {
        self.book.field_lookup[ds.0 as usize]
            .get(name)
            .copied()
            .or_else(|| {
                let ids = self.instances.get(&(sheet?, ds, name.into()))?;
                if ids.len() == 1 {
                    ids.iter().next().copied()
                } else {
                    None
                }
            })
    }
}
