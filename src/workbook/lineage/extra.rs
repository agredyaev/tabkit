use super::{Resolver, gap};
use crate::workbook::lineage::{EdgeKind, Gap, GraphNode, NODE_KIND_COUNT, NodeKind, NodeRef};
use crate::{workbook::Workbook, xml::NodeId};
use serde_json::Value;
use std::collections::BTreeMap;

#[path = "source.rs"]
mod source;
#[path = "view.rs"]
mod view;

pub(super) struct Extra {
    buckets: Vec<Vec<(GraphNode, Option<Value>)>>,
    pub edges: Vec<(NodeRef, NodeRef, EdgeKind)>,
    pub gaps: Vec<Gap>,
    resolved: Vec<(String, NodeId)>,
    resolved_diagnostics: Vec<(String, String, String)>,
}

impl Extra {
    fn new() -> Self {
        Self {
            buckets: vec![Vec::new(); NODE_KIND_COUNT],
            edges: Vec::new(),
            gaps: Vec::new(),
            resolved: Vec::new(),
            resolved_diagnostics: Vec::new(),
        }
    }
    pub(super) fn add(
        &mut self,
        kind: NodeKind,
        name: &str,
        caption: &str,
        datasource: Option<NodeRef>,
        worksheet: Option<NodeRef>,
        detail: Value,
    ) -> NodeRef {
        let id = self.buckets[kind.index()].len();
        let reference = NodeRef::new(kind, id);
        self.buckets[kind.index()].push((
            GraphNode {
                reference,
                name: name.into(),
                caption: caption.into(),
                datasource,
                worksheet,
                formula: None,
                data_type: None,
                role: None,
            },
            Some(detail),
        ));
        reference
    }
    pub(super) fn edge(&mut self, from: NodeRef, to: NodeRef, kind: EdgeKind) {
        self.edges.push((from, to, kind));
    }
    pub(super) fn gap(&mut self, code: &str, node: NodeId, detail: &str) {
        gap(&mut self.gaps, code, node, detail);
    }
    pub(super) fn resolved(&mut self, code: &str, node: NodeId) {
        self.resolved.push((code.into(), node));
    }
    pub(super) fn resolved_diagnostic(&mut self, code: &str, object: String, detail: String) {
        self.resolved_diagnostics
            .push((code.into(), object, detail));
    }
    pub(super) fn finish(
        self,
        nodes: &mut Vec<GraphNode>,
        details: &mut Vec<Option<Value>>,
        raw: &mut Vec<(NodeRef, NodeRef, EdgeKind)>,
        gaps: &mut Vec<Gap>,
    ) {
        for bucket in self.buckets.into_iter().skip(NodeKind::Connection.index()) {
            for (node, detail) in bucket {
                nodes.push(node);
                details.push(detail);
            }
        }
        raw.extend(self.edges);
        gaps.extend(self.gaps);
        gaps.retain(|g| {
            !self
                .resolved
                .iter()
                .any(|(code, node)| g.code == *code && g.object == format!("xml_node:{}", node.0))
                && !self
                    .resolved_diagnostics
                    .iter()
                    .any(|(code, object, detail)| {
                        g.code == *code && g.object == *object && g.detail == *detail
                    })
        });
    }
}

pub(super) fn build(
    book: &Workbook,
    resolver: &Resolver<'_>,
    worksheet_ids: &BTreeMap<NodeId, usize>,
    dashboard_ids: &BTreeMap<NodeId, usize>,
) -> Extra {
    let mut extra = Extra::new();
    source::build(book, resolver, &mut extra);
    view::build(book, resolver, worksheet_ids, dashboard_ids, &mut extra);
    extra
}
