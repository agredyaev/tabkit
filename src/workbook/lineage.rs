//! Read-only workbook lineage. The parsed XML is discarded after this compact graph is built.
use crate::error::{Error, Result, require};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[path = "lineage/build.rs"]
mod build;

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Field,
    Datasource,
    WorksheetFilter,
    DatasourceFilter,
    Worksheet,
    Dashboard,
    LocalField,
    ExtractFilter,
    SharedViewFilter,
}

impl NodeKind {
    fn index(self) -> usize {
        self as usize
    }
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct NodeRef {
    pub kind: NodeKind,
    pub id: u32,
}
impl NodeRef {
    fn new(kind: NodeKind, id: usize) -> Self {
        Self {
            kind,
            id: id as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Calculation,
    Rows,
    Cols,
    MarkColor,
    MarkSize,
    MarkText,
    MarkDetail,
    MarkShape,
    MarkTooltip,
    MarkWedgeSize,
    SortField,
    SortUsing,
    WorksheetFilterField,
    WorksheetFiltered,
    DatasourceFilterField,
    DatasourceFiltered,
    ExtractFilterField,
    ExtractFiltered,
    SharedViewFilterField,
    DatasourceUsed,
    SheetInDashboard,
}

#[derive(Clone, Serialize)]
pub struct GraphNode {
    pub reference: NodeRef,
    pub name: String,
    pub caption: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasource: Option<NodeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worksheet: Option<NodeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct GraphEdge {
    pub from: NodeRef,
    pub to: NodeRef,
    pub kind: EdgeKind,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct Gap {
    pub code: String,
    pub object: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Clone, Copy)]
struct Link {
    from: u32,
    to: u32,
    kind: EdgeKind,
}

pub struct Graph {
    pub input_sha256: String,
    pub source_build: Option<String>,
    nodes: Vec<GraphNode>,
    bases: [usize; 10],
    links: Vec<Link>,
    outgoing: Vec<u32>,
    incoming_order: Vec<u32>,
    incoming: Vec<u32>,
    search_caption: Vec<(String, u32)>,
    search_name: Vec<(String, u32)>,
    pub gaps: Vec<Gap>,
}

const EXCLUDED: &[&str] = &[
    "actions",
    "sets_and_groups",
    "tooltip_expressions",
    "parameter_controls",
    "table_calculation_runtime",
    "stories",
];

impl Graph {
    pub fn count(&self) -> (usize, usize) {
        (self.nodes.len(), self.links.len())
    }
    pub fn gap_page(&self, offset: usize, limit: usize) -> Result<Value> {
        require(
            (1..=100).contains(&limit),
            "LIMIT",
            "Lineage page limit must be 1..100",
        )?;
        let total = self.gaps.len();
        let end = offset.saturating_add(limit).min(total);
        Ok(
            json!({"items": &self.gaps[offset.min(total)..end], "total": total,
            "next_offset": (end < total).then_some(end)}),
        )
    }
    pub fn coverage(&self) -> Value {
        json!({"status": if self.gaps.is_empty() {"complete_for_v1_routes"} else {"partial"},
            "gap_count": self.gaps.len(), "excluded_constructs": EXCLUDED})
    }
    pub fn node(&self, r: NodeRef) -> Result<&GraphNode> {
        let begin = self.bases[r.kind.index()];
        let end = self.bases[r.kind.index() + 1];
        self.nodes
            .get(begin + r.id as usize)
            .filter(|_| begin + (r.id as usize) < end)
            .ok_or_else(|| Error::new("TARGET_NOT_FOUND", "Unknown lineage node"))
    }
    fn index(&self, r: NodeRef) -> Result<usize> {
        self.node(r)?;
        Ok(self.bases[r.kind.index()] + r.id as usize)
    }
    fn edge(&self, link: Link) -> GraphEdge {
        GraphEdge {
            from: self.nodes[link.from as usize].reference,
            to: self.nodes[link.to as usize].reference,
            kind: link.kind,
        }
    }
    pub fn find(
        &self,
        prefix: &str,
        by: SearchBy,
        kind: Option<NodeKind>,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        require(
            (1..=100).contains(&limit),
            "LIMIT",
            "Lineage page limit must be 1..100",
        )?;
        require(
            !prefix.trim().is_empty() && prefix.len() <= 256,
            "LIMIT",
            "Search prefix must be 1..256 bytes",
        )?;
        let prefix = prefix.to_lowercase();
        let search = if by == SearchBy::Caption {
            &self.search_caption
        } else {
            &self.search_name
        };
        let start = search.partition_point(|(key, _)| key < &prefix);
        let mut total = 0usize;
        let mut items = Vec::new();
        for (key, index) in &search[start..] {
            if !key.starts_with(&prefix) {
                break;
            }
            let node = &self.nodes[*index as usize];
            if kind.is_some_and(|k| node.reference.kind != k) {
                continue;
            }
            if total >= offset && items.len() < limit {
                items.push(node);
            }
            total += 1;
        }
        Ok(
            json!({"items": items, "by": by, "total": total, "next_offset":
            (offset.saturating_add(items.len()) < total).then(|| offset + items.len())}),
        )
    }
    pub fn neighbors(
        &self,
        node: NodeRef,
        direction: Direction,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        require(
            (1..=100).contains(&limit),
            "LIMIT",
            "Lineage page limit must be 1..100",
        )?;
        let index = self.index(node)?;
        let range = self.adjacent(index, direction);
        let total = range.end - range.start;
        let end = offset.saturating_add(limit).min(total);
        let mut items = Vec::new();
        for pos in offset.min(total)..end {
            let link = self.link_at(range.start + pos, direction);
            let other = if direction == Direction::Downstream {
                link.to
            } else {
                link.from
            };
            items.push(json!({"edge": self.edge(link), "node": self.nodes[other as usize]}));
        }
        Ok(
            json!({"node": self.nodes[index], "direction": direction, "items": items,
            "total": total, "next_offset": (end < total).then_some(end)}),
        )
    }
    fn adjacent(&self, index: usize, direction: Direction) -> std::ops::Range<usize> {
        let offsets = if direction == Direction::Downstream {
            &self.outgoing
        } else {
            &self.incoming
        };
        offsets[index] as usize..offsets[index + 1] as usize
    }
    fn link_at(&self, pos: usize, direction: Direction) -> Link {
        let index = if direction == Direction::Downstream {
            pos
        } else {
            self.incoming_order[pos] as usize
        };
        self.links[index]
    }
    pub fn export(&self, writer: &mut dyn std::io::Write) -> Result<()> {
        writer.write_all(b"{\"schema_version\":1,\"input_sha256\":")?;
        serde_json::to_writer(&mut *writer, &self.input_sha256)?;
        writer.write_all(b",\"source_build\":")?;
        serde_json::to_writer(&mut *writer, &self.source_build)?;
        writer.write_all(b",\"coverage\":")?;
        serde_json::to_writer(&mut *writer, &self.coverage())?;
        writer.write_all(b",\"nodes\":[")?;
        for (i, node) in self.nodes.iter().enumerate() {
            if i > 0 {
                writer.write_all(b",")?;
            }
            serde_json::to_writer(&mut *writer, node)?;
        }
        writer.write_all(b"],\"edges\":[")?;
        for (i, &link) in self.links.iter().enumerate() {
            if i > 0 {
                writer.write_all(b",")?;
            }
            serde_json::to_writer(&mut *writer, &self.edge(link))?;
        }
        writer.write_all(b"],\"gaps\":[")?;
        for (i, gap) in self.gaps.iter().enumerate() {
            if i > 0 {
                writer.write_all(b",")?;
            }
            serde_json::to_writer(&mut *writer, gap)?;
        }
        writer.write_all(b"]}\n")?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upstream,
    Downstream,
}
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchBy {
    #[default]
    Caption,
    Name,
}

/// One resumable breadth-first walk. Each page scans at most `limit` edges.
pub struct ImpactCursor {
    pub id: String,
    pub from: NodeRef,
    pub direction: Direction,
    visited: Vec<bool>,
    queue: Vec<u32>,
    head: usize,
    next: usize,
    depths: Vec<u32>,
    first_page: bool,
}
impl ImpactCursor {
    pub fn new(graph: &Graph, from: NodeRef, direction: Direction) -> Result<Self> {
        let index = graph.index(from)?;
        let mut visited = vec![false; graph.nodes.len()];
        visited[index] = true;
        let queue = if graph.adjacent(index, direction).is_empty() {
            Vec::new()
        } else {
            vec![index as u32]
        };
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            from,
            direction,
            visited,
            queue,
            head: 0,
            next: 0,
            depths: vec![0; graph.nodes.len()],
            first_page: true,
        })
    }
    pub fn page(&mut self, graph: &Graph, limit: usize) -> Result<Value> {
        require(
            (1..=100).contains(&limit),
            "LIMIT",
            "Lineage page limit must be 1..100",
        )?;
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        if self.first_page {
            nodes.push(json!({"node": graph.node(self.from)?, "depth": 0, "parent": Value::Null}));
            self.first_page = false;
        }
        while edges.len() < limit && self.head < self.queue.len() {
            let owner = self.queue[self.head] as usize;
            let range = graph.adjacent(owner, self.direction);
            if self.next >= range.len() {
                self.head += 1;
                self.next = 0;
                continue;
            }
            let link = graph.link_at(range.start + self.next, self.direction);
            self.next += 1;
            let other = if self.direction == Direction::Downstream {
                link.to
            } else {
                link.from
            };
            edges.push(graph.edge(link));
            if !self.visited[other as usize] {
                self.visited[other as usize] = true;
                self.depths[other as usize] = self.depths[owner].saturating_add(1);
                if !graph.adjacent(other as usize, self.direction).is_empty() {
                    self.queue.push(other);
                }
                nodes.push(json!({"node": graph.nodes[other as usize],
                    "depth": self.depths[other as usize],
                    "parent": graph.nodes[owner].reference}));
            }
        }
        while self.head < self.queue.len() {
            let range = graph.adjacent(self.queue[self.head] as usize, self.direction);
            if self.next < range.len() {
                break;
            }
            self.head += 1;
            self.next = 0;
        }
        let done = self.head == self.queue.len();
        Ok(json!({"from": self.from, "direction": self.direction,
            "nodes": nodes, "edges": edges, "done": done,
            "cursor": (!done).then_some(self.id.as_str())}))
    }
}
