use crate::{
    config::Limits,
    error::{
        Error,
        Result,
        require
    },
    formula,
    fs::sha256,
    scalar::{
        Domain,
        Scalar
    },
    xml::{
        NodeId,
        Xml
    }
};
use schemars::JsonSchema;
use serde::{Serialize, Deserialize};
use serde_json::{
    Value,
    json
};
use ahash::AHashMap as HashMap;
use std::collections::{
    BTreeMap,
    BTreeSet,
    VecDeque
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct FieldId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct DatasourceId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct FilterId(pub u32);
impl std::fmt::Display for FieldId {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { write!(f,"{}",self.0) }
}
impl std::fmt::Display for FilterId {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { write!(f,"{}",self.0) }
}
#[derive(Clone, Debug, Serialize)]
pub struct Datasource {
    pub id: DatasourceId,
    pub name: String,
    pub caption: String
}
pub struct Field {
    pub datasource: DatasourceId,
    pub name: String,
    pub caption: String,
    pub datatype: String,
    pub role: String,
    pub node: Option<NodeId>,
    /// Primary column first, then worksheet dependency copies in document order.
    pub copies: Vec<NodeId>,
    pub formula: Option<String>,
    pub parameter: bool,
}
pub struct Filter {
    pub id: FilterId,
    pub node: NodeId,
    pub worksheet: String,
    pub column: String,
    pub field: Option<FieldId>,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub code: String,
    pub object: String,
    pub message: String
}
#[derive(Clone, Debug, Serialize)]
pub struct Edge {
    pub from: FieldId,
    pub to: FieldId
}
/// Worksheet dependency declarations. Field IDs are stored in one flat side table.
pub struct DependencyScope {
    pub node: NodeId,
    pub worksheet: Option<NodeId>,
    pub datasource: Option<DatasourceId>,
    pub fields: std::ops::Range<usize>,
}
#[derive(Clone, Debug, Serialize)]
pub struct LocalDefinition {
    pub node_id: NodeId,
    pub worksheet: Option<String>,
    pub datasource: Option<String>,
    pub name: Option<String>,
    pub formula: Option<String>,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct FieldUse {
    pub field_id: FieldId,
    pub worksheet_node: NodeId,
    pub node_id: NodeId,
    pub kind: &'static str,
}
pub struct Workbook {
    datasource_lookup:BTreeMap<String,
    DatasourceId>,
    field_lookup:Vec<HashMap<String, FieldId>>,
    pub xml: Xml,
    pub source_build: Option<String>,
    pub datasources: Vec<Datasource>,
    pub fields: Vec<Field>,
    pub filters: Vec<Filter>,
    pub edges: Vec<Edge>,
    referrer_order: Vec<usize>,
    filter_order: Vec<usize>,
    pub ref_offsets: Vec<u32>,
    pub dependency_scopes: Vec<DependencyScope>,
    pub dependency_fields: Vec<FieldId>,
    pub local_definitions: Vec<LocalDefinition>,
    pub known_uses: Vec<FieldUse>,
    pub diagnostics: Vec<Diagnostic>,
    pub worksheets: Vec<String>,
    pub dashboards: Vec<String>,
}
impl Workbook {
    pub fn into_xml(self) -> Xml { self.xml }
    pub fn parse(bytes: Vec<u8>, limits: &Limits) -> Result<Self> {
        Self::from_xml(Xml::parse(bytes, limits)?)
    }
    pub(crate) fn parse_with_sha256(bytes: Vec<u8>, limits: &Limits, sha256: String) -> Result<Self> {
        Self::from_xml(Xml::parse_with_sha256(bytes, limits, sha256)?)
    }
    pub fn from_xml(xml: Xml) -> Result<Self> {
        let root = NodeId(0);
        require(xml.tag(root) == "workbook", "FORMAT", "Expected a Tableau workbook root")?;
        let source_build = xml.value(root, "source-build").map(str::to_owned);
        let ds_container = xml.one_child(root, "datasources")?;
        let mut datasources = Vec::new();
        let mut fields = Vec::new();
        let mut ds_ids = BTreeMap::new();
        let mut field_ids: Vec<HashMap<String, FieldId>> = Vec::new();
        let mut defined_fields:Vec<Vec<FieldId>>=Vec::new();
        for ds in xml.named_children(ds_container, "datasource") {
            let name = xml.required(ds, "name")?.to_string();
            require(!ds_ids.contains_key(&name), "AMBIGUOUS_TARGET", "Duplicate datasource internal name")?;
            let id = DatasourceId(datasources.len() as u32);
            field_ids.push(HashMap::new());
            defined_fields.push(Vec::new());
            ds_ids.insert(name.clone(), id);
            datasources.push(Datasource {
                id,
                caption: xml.value(ds, "caption").unwrap_or(&name).into(),
                name
            });
            for n in xml.named_children(ds, "column") {
                let name = xml.required(n, "name")?.to_string();

                require(!field_ids[id.0 as usize].contains_key(name.as_str()), "AMBIGUOUS_TARGET", "Duplicate datasource field definition")?;
                let fid = FieldId(fields.len() as u32);
                field_ids[id.0 as usize].insert(name.clone(), fid);
                defined_fields[id.0 as usize].push(fid);
                let calc: Vec<_> = xml.named_children(n, "calculation").collect();
                require(calc.len() <= 1, "UNSUPPORTED_SHAPE", "Multiple calculations in a column")?;
                let formula = calc.first().and_then(|c| xml.value(*c, "formula")).map(str::to_owned);
                fields.push(Field {
                    datasource: id,
                    caption: xml.value(n, "caption").unwrap_or(&name).into(),
                    name,
                    datatype: xml.value(n, "datatype").unwrap_or("unknown").into(),
                    role: xml.value(n, "role").unwrap_or("unknown").into(),
                    node: Some(n),
                    copies: vec![n],
                    formula,
                    parameter: xml.value(n, "param-domain-type").is_some()
                });
            }
            // Physical columns not materialized as global <column> still resolve references.
            let ds_span = xml.node(ds).span;
            for &n in &xml.semantic.metadata_records {
                let span=xml.node(n).span;
                if !(ds_span.start < span.start && span.end <= ds_span.end)
                    || xml.value(n,"class")!=Some("column") { continue; }
                let local = xml.named_children(n, "local-name").next();
                let Some(local) = local else {
                    continue;
                };
                let name = xml.text_content(local)?.to_owned();

                if field_ids[id.0 as usize].contains_key(name.as_str()) {
                    continue;
                }
                let dtype = xml.named_children(n, "local-type").next().map(|t| xml.text_content(t)).transpose()?.unwrap_or("unknown").to_owned();
                let fid = FieldId(fields.len() as u32);
                field_ids[id.0 as usize].insert(name.clone(), fid);
                fields.push(Field {
                    datasource: id,
                    caption: name.clone(),
                    name,
                    datatype: dtype,
                    role: "physical".into(),
                    node: None,
                    copies: Vec::new(),
                    formula: None,
                    parameter: false
                });
            }
        }
        // Dependency copies are commonly repeated once per worksheet. Reserve from the
        // admitted scope count per datasource to avoid geometric growth in every Field.
        // The cap bounds speculative slack for sparse or unusual documents.
        let mut scope_counts=vec![0usize;datasources.len()];
        for &scope in &xml.semantic.datasource_dependencies {
            if let Some(id)=xml.value(scope,"datasource").and_then(|n|ds_ids.get(n)).copied() {
                scope_counts[id.0 as usize]=scope_counts[id.0 as usize].saturating_add(1);
            }
        }
        for field in &mut fields {
            if field.node.is_some() {
                let additional=scope_counts[field.datasource.0 as usize].min(256);
                field.copies.reserve_exact(additional);
            }
        }
        // Known copies remain attached to a global field. Local/unknown definitions
        // are exposed explicitly instead of disappearing from the coverage report.
        let mut dependency_scopes = Vec::new();
        let mut dependency_fields = Vec::new();
        let mut local_definitions = Vec::new();
        for &scope in &xml.semantic.datasource_dependencies {
            let ds_name = xml.value(scope, "datasource");
            let ds = ds_name.and_then(|n| ds_ids.get(n)).copied();
            let worksheet = xml.ancestor(scope, "worksheet");
            let begin = dependency_fields.len();
            let mut ordered_index=0usize;
            let mut ordered=true;
            for n in xml.named_children(scope, "column") {
                let name = xml.value(n, "name");
                let fid = ds.and_then(|d| {
                    let name=name?;
                    if ordered {
                        if let Some(&candidate)=defined_fields[d.0 as usize].get(ordered_index) {
                            if fields[candidate.0 as usize].name==name {
                                ordered_index+=1;
                                return Some(candidate);
                            }
                        }
                        ordered=false;
                    }
                    field_ids[d.0 as usize].get(name).copied()
                });
                if let Some(fid) = fid {
                    fields[fid.0 as usize].copies.push(n);
                    dependency_fields.push(fid);
                } else {
                    let formula = xml.named_children(n,"calculation").next()
                        .and_then(|c|xml.value(c,"formula")).map(str::to_owned);
                    local_definitions.push(LocalDefinition {
                        node_id:n,
                        worksheet:worksheet.and_then(|w|xml.value(w,"name")).map(str::to_owned),
                        datasource:ds_name.map(str::to_owned),
                        name:name.map(str::to_owned), formula,
                        reason:"No indexed global definition; preserved and read-only in v1".into(),
                    });
                }
            }
            dependency_scopes.push(DependencyScope {
                node:scope, worksheet, datasource:ds, fields:begin..dependency_fields.len(),
            });
        }
        // Build worksheet instance resolution once; avoid a document scan per filter.
        let mut instances:BTreeMap<(NodeId,DatasourceId), BTreeMap<String,BTreeSet<FieldId>>>=BTreeMap::new();
        for &n in &xml.semantic.column_instances {
            let Some(sheet)=xml.ancestor(n,"worksheet") else{
                continue;
            };
            let Some(deps)=xml.ancestor(n,"datasource-dependencies") else{
                continue;
            };
            let Some(dsid)=xml.value(deps,"datasource").and_then(|s|ds_ids.get(s)) else{
                continue;
            };
            if let(Some(name),Some(column))=(xml.value(n,"name"),xml.value(n,"column")){
                if let Some(fid)=field_ids[dsid.0 as usize].get(column){
                    instances.entry((sheet,*dsid)).or_default().entry(name.into()).or_default().insert(*fid);
                }
            }
        }
        let mut known_uses = Vec::new();
        let resolve_qualified = |sheet:NodeId,ds:&str, instance:&str| -> Option<FieldId> {
            let dsid = *ds_ids.get(ds)?;
            field_ids[dsid.0 as usize].get(instance).copied().or_else(|| {
                let ids = instances.get(&(sheet,dsid))?.get(instance)?;
                (ids.len()==1).then(||ids.iter().next().copied()).flatten()
            })
        };
        for &n in &xml.semantic.column_bindings {
            let Some(sheet)=xml.ancestor(n,"worksheet") else {continue;};
            if let Some(column)=xml.value(n,"column") {
                if let Ok((ds,field))=formula::qualified(column) {
                    if let Some(field_id)=resolve_qualified(sheet,&ds,&field) {
                        known_uses.push(FieldUse{field_id,worksheet_node:sheet,node_id:n,kind:"column_binding"});
                    }
                }
            }
        }
        for &n in &xml.semantic.shelves {
            let Some(sheet)=xml.ancestor(n,"worksheet") else {continue;};
            if xml.node(n).first_child().is_none() {
                if let Ok(a)=formula::analyze(xml.text_content(n)?) {
                    for r in a.references {
                        if let Some(ds)=r.datasource {
                            if let Some(field_id)=resolve_qualified(sheet,&ds,&r.field) {
                                known_uses.push(FieldUse{field_id,worksheet_node:sheet,node_id:n,kind:"shelf"});
                            }
                        }
                    }
                }
            }
        }
        known_uses.sort_by_key(|u|(u.field_id,u.worksheet_node,u.node_id,u.kind));
        known_uses.dedup_by_key(|u|(u.field_id,u.worksheet_node,u.node_id,u.kind));
        let worksheets:Vec<String>=xml.semantic.worksheets.iter()
            .map(|&n|xml.required(n,"name").map(str::to_owned)).collect::<Result<_>>()?;
        let dashboards:Vec<String>=xml.semantic.dashboards.iter()
            .map(|&n|xml.required(n,"name").map(str::to_owned)).collect::<Result<_>>()?;
        let mut filters=Vec::with_capacity(xml.semantic.filters.len());
        for &n in &xml.semantic.filters {
            let Some(sheet)=xml.ancestor(n,"worksheet") else {continue;};
            let column=xml.value(n,"column").unwrap_or("").to_string();
            let mut field=None;
            if let Ok((ds,instance))=formula::qualified(&column) {
                if let Some(dsid)=ds_ids.get(&ds) {
                    field=field_ids[dsid.0 as usize].get(instance.as_str()).copied();
                    if field.is_none() {
                        if let Some(resolved)=instances.get(&(sheet,*dsid)).and_then(|m|m.get(instance.as_str())) {
                            if resolved.len()==1 {field=resolved.iter().next().copied();}
                        }
                    }
                }
            }
            filters.push(Filter{id:FilterId(filters.len() as u32),node:n,
                worksheet:xml.required(sheet,"name")?.into(),column,field,
                kind:xml.value(n,"class").unwrap_or("unknown").into()});
        }
        require(unique(&worksheets) && unique(&dashboards), "AMBIGUOUS_TARGET", "Duplicate worksheet or dashboard names")?;
        let mut w = Self {
            datasource_lookup:ds_ids,
            field_lookup:field_ids,
            xml,
            source_build,
            datasources,
            fields,
            filters,
            edges: Vec::new(),
            referrer_order: Vec::new(), filter_order: Vec::new(),
            ref_offsets: Vec::new(),
            dependency_scopes, dependency_fields, local_definitions, known_uses,
            diagnostics: Vec::new(),
            worksheets,
            dashboards
        };
        w.build_refs();
        Ok(w)
    }
    pub fn require_2025(&self) -> Result<()> {
        let build = self.source_build.as_deref().ok_or_else(|| Error::new("VERSION_UNVERIFIED", "Mutation needs source-build identifying Tableau 2025; inspect remains available"))?;
        let version = build.split_whitespace().next().unwrap_or("");
        let numbers:Vec<_>=version.split('.').collect();
        let supported=numbers.len()>=3 && numbers[0]=="2025" && matches!(numbers[1],"1"|"2"|"3") && numbers[2].parse::<u32>().is_ok();
        require(supported, "UNSUPPORTED_VERSION", format!("Mutation targets numeric Tableau 2025.1/2/3 source-build, received {version}"))
    }
    pub fn field(&self, id: FieldId) -> Result<&Field> {
        self.fields.get(id.0 as usize).ok_or_else(|| Error::new("TARGET_NOT_FOUND", "Unknown field_id"))
    }
    pub fn filter(&self, id: FilterId) -> Result<&Filter> {
        self.filters.get(id.0 as usize).ok_or_else(|| Error::new("TARGET_NOT_FOUND", "Unknown filter_id"))
    }
    pub fn field_key(&self, id: FieldId) -> String {
        let f = &self.fields[id.0 as usize];
        format!("{}::{}", self.datasources[f.datasource.0 as usize].name, f.name)
    }
    pub fn resolve(&self, datasource: DatasourceId, r: &formula::Reference) -> Result<FieldId> {
        let ds = match &r.datasource {
            None => datasource,
            Some(name) => *self.datasource_lookup.get(name)
            .ok_or_else(|| Error::new("UNRESOLVED_REFERENCE", format!("Unknown datasource {name}")))?,
        };
        self.field_lookup.get(ds.0 as usize).and_then(|m|m.get(r.field.as_str())).copied()
        .ok_or_else(||Error::new("UNRESOLVED_REFERENCE",format!("Use an inspected internal field name: {}",r.field)))
    }
    fn build_refs(&mut self) {
        let mut edges = Vec::new();
        let mut diagnostics = Vec::new();
        for (i, f) in self.fields.iter().enumerate() {
            if f.parameter {
                continue;
            }
            let Some(text) = &f.formula else {
                continue;
            };
            let object = self.field_key(FieldId(i as u32));
            if let Some(column)=f.node {
                if self.xml.named_children(column,"calculation").next()
                    .and_then(|n|self.xml.value(n,"class"))!=Some("tableau") {
                    diagnostics.push(Diagnostic{code:"UNSUPPORTED_CALCULATION_CLASS".into(),object,
                        message:"Non-Tableau calculation is preserved without reference analysis".into()});
                    continue;
                }
            }
            match formula::analyze(text) {
                Ok(a) => {
                    for r in a.references {
                        match self.resolve(f.datasource, &r) {
                            Ok(to) => edges.push(Edge {
                                from: FieldId(i as u32),
                                to
                            }),
                            Err(e) => diagnostics.push(Diagnostic {
                                code: e.code.into(),
                                object: object.clone(),
                                message: e.message
                            }),
                        }
                    }
                }
                Err(e) => diagnostics.push(Diagnostic {
                    code: e.code.into(),
                    object,
                    message: e.message
                }),
            }
        }
        edges.sort_by_key(|e| (e.from, e.to));
        edges.dedup_by_key(|e| (e.from, e.to));
        self.ref_offsets = vec![0; self.fields.len()+1];
        for e in &edges {
            self.ref_offsets[e.from.0 as usize + 1] += 1;
        }
        for i in 1..self.ref_offsets.len() {
            self.ref_offsets[i] += self.ref_offsets[i-1];
        }
        self.referrer_order = (0..edges.len()).collect();
        self.referrer_order.sort_unstable_by_key(|i| (edges[*i].to, edges[*i].from));
        self.filter_order = (0..self.filters.len()).collect();
        self.filter_order.sort_unstable_by_key(|i| (self.filters[*i].field, self.filters[*i].id));
        self.edges = edges;
        diagnostics.sort();
        self.diagnostics = diagnostics;
    }
    /// Validate the calculation graph that would exist after replacing selected formulas.
    /// None keeps the admitted field edges; Some is the fully resolved replacement edge list.
    pub(crate) fn validate_calculation_overrides(&self, overrides: &[Option<Vec<FieldId>>]) -> Result<()> {
        require(overrides.len() == self.fields.len(), "INTERNAL", "Formula override table length mismatch")?;
        let mut edges=Vec::with_capacity(self.edges.len());
        for i in 0..self.fields.len() {
            let from=FieldId(i as u32);
            if let Some(targets)=&overrides[i] {
                edges.extend(targets.iter().copied().map(|to|Edge{from,to}));
            } else {
                edges.extend(self.edges[self.ref_offsets[i] as usize..self.ref_offsets[i+1] as usize]
                    .iter().map(|e|Edge{from:e.from,to:e.to}));
            }
        }
        let offsets=edge_offsets(self.fields.len(),&edges);
        require_acyclic_edges(self.fields.len(),&edges,&offsets)?;
        let changed:Vec<_>=overrides.iter().enumerate()
            .filter_map(|(i,v)|v.as_ref().map(|_|FieldId(i as u32))).collect();
        self.require_calculation_dependencies_edges(&changed,&edges,&offsets,overrides)
    }
    pub fn require_acyclic(&self) -> Result<()> {
        require_acyclic_edges(self.fields.len(),&self.edges,&self.ref_offsets)
    }
    /// Refuse changes requiring a worksheet dependency rewrite that v1 does not implement.
    /// This is an edit-admission rule, not an assertion about every valid Tableau XML form.
    pub fn require_calculation_dependencies(&self, changed: &[FieldId]) -> Result<()> {
        let overrides:Vec<Option<Vec<FieldId>>>=std::iter::repeat_with(||None).take(self.fields.len()).collect();
        self.require_calculation_dependencies_edges(changed,&self.edges,&self.ref_offsets,&overrides)
    }
    fn require_calculation_dependencies_edges(&self, changed: &[FieldId], edges:&[Edge],
        ref_offsets:&[u32], overrides:&[Option<Vec<FieldId>>]) -> Result<()> {
        for &id in changed { self.field(id)?; }
        if changed.is_empty(){return Ok(());}
        let mut reverse_offsets=vec![0usize;self.fields.len()+1];
        for e in edges { reverse_offsets[e.to.0 as usize+1]+=1; }
        for i in 1..reverse_offsets.len(){reverse_offsets[i]+=reverse_offsets[i-1];}
        let mut cursor=reverse_offsets.clone();
        let mut reverse=vec![FieldId(0);edges.len()];
        for e in edges {
            let slot=&mut cursor[e.to.0 as usize];
            reverse[*slot]=e.from; *slot+=1;
        }
        let mut affected = vec![false;self.fields.len()];
        for &id in changed { affected[id.0 as usize]=true; }
        let mut pending=changed.to_vec();
        while let Some(f)=pending.pop() {
            for &caller in &reverse[reverse_offsets[f.0 as usize]..reverse_offsets[f.0 as usize+1]] {
                if !affected[caller.0 as usize] { affected[caller.0 as usize]=true; pending.push(caller); }
            }
        }
        let mut sheets=BTreeSet::new();
        for scope in &self.dependency_scopes {
            if self.dependency_fields[scope.fields.clone()].iter().any(|f|affected[f.0 as usize]) {
                if let Some(sheet)=scope.worksheet { sheets.insert(sheet); }
                else { return Err(Error::new("DEPENDENCY_UPDATE_REQUIRED","Changed calculation has a dependency copy outside the supported worksheet scope")); }
            }
        }
        for usage in &self.known_uses {
            if affected[usage.field_id.0 as usize] { sheets.insert(usage.worksheet_node); }
        }
        // Build owner ranges once per operation, not one full scan per worksheet.
        let mut scopes: Vec<_> = self.dependency_scopes.iter().enumerate()
            .filter_map(|(i,s)| s.worksheet.map(|w| (w,i))).collect();
        scopes.sort_unstable();
        let mut uses: Vec<_> = self.known_uses.iter().enumerate().map(|(i,u)| (u.worksheet_node,i)).collect();
        uses.sort_unstable();
        let mut declared = vec![0usize; self.fields.len()];
        let mut seen = vec![0usize; self.fields.len()];
        let mut pending = Vec::new();
        for (iteration, sheet) in sheets.into_iter().enumerate() {
            let epoch = iteration + 1; pending.clear();
            let start = scopes.partition_point(|(w,_)| *w < sheet);
            let end = scopes.partition_point(|(w,_)| *w <= sheet);
            for &(_,i) in &scopes[start..end] {
                for &field in &self.dependency_fields[self.dependency_scopes[i].fields.clone()] {
                    let slot = &mut declared[field.0 as usize];
                    require(*slot != epoch,"DEPENDENCY_UPDATE_REQUIRED","Duplicate worksheet declarations prevent unambiguous dependency proof")?;
                    *slot = epoch;
                    if affected[field.0 as usize] { pending.push(field); }
                }
            }
            pending.sort_unstable();
            let start = uses.partition_point(|(w,_)| *w < sheet);
            let end = uses.partition_point(|(w,_)| *w <= sheet);
            pending.extend(uses[start..end].iter().map(|(_,i)|self.known_uses[*i].field_id).filter(|f|affected[f.0 as usize]));
            while let Some(field) = pending.pop() {
                let i = field.0 as usize;
                if seen[i] == epoch { continue; } seen[i] = epoch;
                if declared[i] != epoch {
                    return Err(Error::new("DEPENDENCY_UPDATE_REQUIRED",format!(
                        "Worksheet {} needs an explicit dependency for {}; v1 will not invent it",
                        self.xml.value(sheet,"name").unwrap_or("?"),self.field_key(field))));
                }
                if overrides[i].is_none() && !self.diagnostics.is_empty() {
                    let key = self.field_key(field);
                    if self.diagnostics.iter().any(|d|d.object==key) {
                        return Err(Error::new("DEPENDENCY_UPDATE_REQUIRED",format!(
                            "Dependency closure for {key} contains an unanalyzed calculation")));
                    }
                }
                pending.extend(edges[ref_offsets[i] as usize..ref_offsets[i+1] as usize].iter().map(|e|e.to));
            }
        }
        Ok(())
    }
    pub fn scope_report(&self, i:usize)->Value {
        let s=&self.dependency_scopes[i];
        json!({"node_id":s.node,"worksheet":s.worksheet.and_then(|n|self.xml.value(n,"name")),
            "datasource_id":s.datasource,"field_ids":&self.dependency_fields[s.fields.clone()]})
    }
    pub fn parameter_state(&self, id: FieldId) -> Result<(Scalar, Domain)> {
        let f = self.field(id)?;
        require(f.parameter, "TYPE_MISMATCH", "Target is not a parameter")?;
        let n = f.node.ok_or_else(|| Error::new("UNSUPPORTED_SHAPE", "Parameter has no primary definition"))?;
        self.parameter_at(n, &f.datatype)
    }
    pub fn parameter_at(&self, n: NodeId, dtype: &str) -> Result<(Scalar, Domain)> {
        // Never silently rewrite dynamic value/domain definitions.
        for child in self.xml.children(n) {
            require(matches!(self.xml.tag(child), "calculation" | "members" | "range"), "UNSUPPORTED_SHAPE", "Only static parameters with known children are editable")?;
        }
        for attr in self.xml.attributes(n) {
            let name = attr.name.to_ascii_lowercase();
            require(!name.contains("source-field") && !name.contains("refresh") && !name.contains("dynamic"), "UNSUPPORTED_SHAPE", "Dynamic parameter metadata is not editable")?;
        }
        let current = Scalar::parse(dtype, self.xml.required(n, "value")?)?;
        let calc = self.xml.one_child(n, "calculation")?;
        require(self.xml.value(calc,"class")==Some("tableau") && self.xml.children(calc).next().is_none(),"UNSUPPORTED_SHAPE","Parameter requires a plain Tableau constant calculation")?;
        let formula = Scalar::parse(dtype, self.xml.required(calc, "formula")?)?;
        require(current.cmp_value(&formula)? == std::cmp::Ordering::Equal, "INCONSISTENT_DEFINITION", "Parameter value and calculation disagree")?;
        let domain = match self.xml.required(n, "param-domain-type")? {
            "any" | "all" => Domain::Any,
            "list" => {
                let members = self.xml.one_child(n, "members")?;
                let mut values = Vec::new();
                for member in self.xml.children(members) {
                    require(self.xml.tag(member) == "member", "UNSUPPORTED_SHAPE", "Unknown parameter member construct")?;
                    values.push(Scalar::parse(dtype, self.xml.required(member, "value")?)?);
                }
                Domain::List {
                    values
                }
            }
            "range" => {
                let r = self.xml.one_child(n, "range")?;
                Domain::Range {
                    min: Scalar::parse(dtype, self.xml.required(r, "min")?)?,
                    max: Scalar::parse(dtype, self.xml.required(r, "max")?)?,
                    step: self.xml.value(r, "granularity").map(|s| Scalar::parse(dtype, s)).transpose()?
                }
            }
            _ => return Err(Error::new("UNSUPPORTED_SHAPE", "Unknown parameter domain")),
        };
        domain.accepts(&current, dtype)?;
        Ok((current, domain))
    }
    pub fn filter_state(&self, id: FilterId) -> Result<Value> {
        let f = self.filter(id)?;
        require(matches!(f.kind.as_str(),"categorical"|"quantitative"), "UNSUPPORTED_SHAPE", "Unsupported filter class is preserved, not validated")?;
        let fid = f.field.ok_or_else(|| Error::new("UNRESOLVED_REFERENCE", "Filter field cannot be resolved"))?;
        let dtype = &self.field(fid)?.datatype;
        match f.kind.as_str() {
            "categorical" => {
                require(dtype == "string", "UNSUPPORTED_SHAPE", "v1 categorical filters require string fields")?;
                let g = self.xml.one_child(f.node, "groupfilter")?;
                let mut values = Vec::new();
                self.members(g, None, &mut values)?;
                Ok(json!({
                    "kind":"categorical",
                    "values":values
                }))
            }
            "quantitative" => {
                require(self.xml.value(f.node, "included-values") == Some("in-range"), "UNSUPPORTED_SHAPE", "Only in-range filters are editable")?;
                let min = self.xml.one_child(f.node, "min")?;
                let max = self.xml.one_child(f.node, "max")?;
                let min = Scalar::parse(dtype, self.xml.text_content(min)?)?;
                let max = Scalar::parse(dtype, self.xml.text_content(max)?)?;
                Domain::Range {
                    min: min.clone(),
                    max: max.clone(),
                    step: None
                }.accepts(&min, dtype)?;
                Ok(json!({
                    "kind":"range",
                    "min":min,
                    "max":max
                }))
            }
            _ => Err(Error::new("UNSUPPORTED_SHAPE", "Only existing categorical and in-range filters are editable")),
        }
    }
    pub fn categorical_level(&self, id: FilterId) -> Result<String> {
        let f = self.filter(id)?;
        let group = self.xml.one_child(f.node, "groupfilter")?;
        let member = if self.xml.value(group, "function") == Some("member") {
            group
        }
        else {
            self.xml.one_child_or_first_member(group)?
        };
        Ok(self.xml.required(member, "level")?.into())
    }
    fn members(&self, n: NodeId, level: Option<&str>, values: &mut Vec<String>) -> Result<()> {
        let func = self.xml.required(n, "function")?;
        for a in self.xml.attributes(n) {
            require(matches!(a.name, "function" | "level" | "member") || a.name.starts_with("user:ui-"), "UNSUPPORTED_SHAPE", "Unknown groupfilter attributes")?;
            if a.name == "user:ui-enumeration" {
                require(a.value == "inclusive", "UNSUPPORTED_SHAPE", "Exclusion filter is unsupported")?;
            }
        }
        match func {
            "member" => {
                require(self.xml.children(n).next().is_none(), "UNSUPPORTED_SHAPE", "Nested member construct")?;
                let found = self.xml.required(n, "level")?;
                if let Some(expected) = level {
                    require(found == expected, "UNSUPPORTED_SHAPE", "Mixed filter levels")?;
                }
                let s = Scalar::parse("string", self.xml.required(n, "member")?)?;
                if let Scalar::String(s) = s {
                    values.push(s);
                }
                Ok(())
            }
            "union" => {
                let children: Vec<_> = self.xml.children(n).collect();
                require(!children.is_empty() && children.len() <= 10_000, "UNSUPPORTED_SHAPE", "Empty or oversized union")?;
                let l = self.xml.required(children[0], "level")?;
                for child in children {
                    require(self.xml.tag(child) == "groupfilter" && self.xml.value(child, "function") == Some("member"), "UNSUPPORTED_SHAPE", "Only a flat union of members is supported")?;
                    self.members(child, Some(l), values)?;
                }
                Ok(())
            }
            _ => Err(Error::new("UNSUPPORTED_SHAPE", "Only inclusive members and flat unions are editable")),
        }
    }
    pub fn field_report(&self, i: usize) -> Value {
        let f=&self.fields[i];
        let id = FieldId(i as u32);
        let ub = self.known_uses.partition_point(|u| u.field_id < id);
        let ue = self.known_uses.partition_point(|u| u.field_id <= id);
        let rb = self.referrer_order.partition_point(|e| self.edges[*e].to < id);
        let re = self.referrer_order.partition_point(|e| self.edges[*e].to <= id);
        let fb = self.filter_order.partition_point(|f| self.filters[*f].field < Some(id));
        let fe = self.filter_order.partition_point(|f| self.filters[*f].field <= Some(id));
        let parameter=if f.parameter {
            Some(match self.parameter_state(FieldId(i as u32)) {
                Ok((current,domain))=>json!({
                    "current":current,
                    "domain":domain,
                    "state_hash":state_hash(&json!({
                        "current":current,
                        "domain":domain
                    }))
                }),
                Err(e)=>json!({
                    "editable":false,
                    "reason":e
                }),
            })
        }else{
            None
        };
        json!({
            "field_id":i,
            "datasource_id":f.datasource,
            "name":f.name,
            "caption":f.caption,
            "datatype":f.datatype,
            "role":f.role,
            "formula":if f.parameter{
                None
            }else{
                f.formula.as_ref()
            },
            "parameter":parameter,
            "definition_copies":f.copies.len(),
            "declared_in_worksheets":f.copies.iter()
                .filter_map(|n|self.xml.ancestor(*n,"worksheet"))
                .filter_map(|w|self.xml.value(w,"name")).collect::<BTreeSet<_>>(),
            "known_worksheet_uses":self.known_uses[ub..ue].iter()
                .filter_map(|u|self.xml.value(u.worksheet_node,"name")).collect::<BTreeSet<_>>(),
            "referenced_by_calculations":self.referrer_order[rb..re].iter()
                .map(|e|self.edges[*e].from).collect::<Vec<_>>(),
            "filters":self.filter_order[fb..fe].iter()
                .map(|f|self.filters[*f].id).collect::<Vec<_>>(),
            "usage_coverage":"Known global formula edges, worksheet declarations, column bindings and shelves; not complete Tableau lineage"
        })
    }
    pub fn filter_report(&self, i:usize)->Value {
        let f=&self.filters[i];
        match self.filter_state(f.id) {
            Ok(state)=>json!({
                "filter_id":f.id,
                "worksheet":f.worksheet,
                "column":f.column,
                "field_id":f.field,
                "state_hash":state_hash(&state),
                "state":state
            }),
            Err(e)=>json!({
                "filter_id":f.id,
                "worksheet":f.worksheet,
                "column":f.column,
                "editable":false,
                "reason":e
            }),
        }
    }
    pub fn overview(&self)->Value {
        json!({
            "twb_sha256":self.xml.sha256,
            "source_build":self.source_build,
            "version_editable":self.require_2025().is_ok(),
            "counts":{
                "datasources":self.datasources.len(),
                "fields":self.fields.len(),
                "filters":self.filters.len(),
                "worksheets":self.worksheets.len(),
                "dashboards":self.dashboards.len(),
                "references":self.edges.len(),
                "read_only_local_definitions":self.local_definitions.len(),
                "known_uses":self.known_uses.len(),
                "diagnostics":self.diagnostics.len()
            },
            "validation_scope":"Known structures and lexical/reference analysis, not Tableau semantic validation"
        })
    }
    pub fn snapshot(&self) -> Result<BTreeMap<String,
    Value>> {
        let mut result = BTreeMap::new();
        result.insert("workbook/worksheets".into(), json!(self.worksheets));
        result.insert("workbook/dashboards".into(), json!(self.dashboards));
        result.insert("workbook/datasources".into(), json!(self.datasources));
        for (i,f) in self.fields.iter().enumerate() {
            let key = format!("field/{i}");
            result.insert(format!("{key}/identity"), json!([f.datasource,f.name,f.caption,f.datatype,f.role]));
            if f.parameter {
                match self.parameter_state(FieldId(i as u32)) {
                    Ok((v,d)) => {
                        result.insert(format!("{key}/parameter"), json!({
                            "current":v,
                            "domain":d
                        }));
                    }
                    Err(_) => {
                        result.insert(format!("{key}/opaque"), json!(f.node.map(|n| sha256(self.xml.text[self.xml.node(n).span.range()].as_bytes()))));
                    }
                }
            } else {
                result.insert(format!("{key}/formula"), json!(f.formula));
            }
        }
        for f in &self.filters {
            result.insert(format!("filter/{}/identity", f.id), json!([f.worksheet, f.column, f.kind]));
            let state = self.filter_state(f.id).unwrap_or_else(|_| json!({
                "opaque_sha256":sha256(self.xml.text[self.xml.node(f.node).span.range()].as_bytes())
            }));
            result.insert(format!("filter/{}/state", f.id), state);
        }
        Ok(result)
    }
}
fn edge_offsets(fields:usize,edges:&[Edge])->Vec<u32>{
    let mut offsets=vec![0u32;fields+1];
    for e in edges { offsets[e.from.0 as usize+1]+=1; }
    for i in 1..offsets.len(){offsets[i]+=offsets[i-1];}
    offsets
}
fn require_acyclic_edges(fields:usize,edges:&[Edge],offsets:&[u32])->Result<()>{
    let mut indegree=vec![0u32;fields];
    for e in edges { indegree[e.to.0 as usize]+=1; }
    let mut ready:VecDeque<usize>=indegree.iter().enumerate()
        .filter_map(|(i,n)|(*n==0).then_some(i)).collect();
    let mut count=0;
    while let Some(i)=ready.pop_front(){
        count+=1;
        for e in &edges[offsets[i] as usize..offsets[i+1] as usize]{
            let j=e.to.0 as usize; indegree[j]-=1;
            if indegree[j]==0 {ready.push_back(j);}
        }
    }
    require(count==fields,"REFERENCE_CYCLE","Known calculation references contain a cycle")
}
impl Xml {
    fn one_child_or_first_member(&self, n: NodeId) -> Result<NodeId> {
        self.named_children(n, "groupfilter").next().ok_or_else(|| Error::new("UNSUPPORTED_SHAPE", "Missing filter member"))
    }
}
fn unique(items: &[String]) -> bool {
    items.iter().collect::<BTreeSet<_>>().len() == items.len()
}
pub fn state_hash(v: &Value) -> String {
    sha256(v.to_string().as_bytes())
}
