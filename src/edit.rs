//! Pure planning. No files, network, clock or randomness here.
use crate::workbook::{FieldId, FilterId};
use crate::{
    config::Config,
    error::{
        Error,
        Result,
        require
    },
    formula,
    patch::{
        self,
        Delta,
        Patch
    },
    scalar::{
        Domain,
        Scalar
    },
    workbook::{
        Workbook,
        state_hash
    },
    xml::{
        NodeId,
        escape_attribute,
        escape_text
    }
};
use schemars::JsonSchema;
use serde::{
    Deserialize,
    Serialize
};
use serde_json::{
    Value,
    json
};
use std::collections::{
    BTreeMap,
    BTreeSet
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeSet {
    pub schema_version: u32,
    /// SHA-256 of the full original TWB or TWBX, not just its embedded XML.
    pub input_sha256: String,
    pub operations: Vec<Operation>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag="op", rename_all="snake_case", deny_unknown_fields)]
pub enum Operation {
    SetCalculation {
        field_id: FieldId,
        expected_formula: String,
        formula: String
    },
    SetParameter {
        field_id: FieldId,
        expected_state_hash: String,
        current: Option<Scalar>,
        domain: Option<Domain>
    },
    SetFilterValues {
        filter_id: FilterId,
        expected_state_hash: String,
        values: Vec<String>
    },
    SetFilterRange {
        filter_id: FilterId,
        expected_state_hash: String,
        min: Scalar,
        max: Scalar
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: u32,
    pub engine_version: String,
    pub input: String,
    pub changes: ChangeSet,
    pub twb_sha256: String,
    pub candidate_twb_sha256: String,
    pub patches: Vec<Patch>,
    pub delta: Vec<Delta>,
    pub warnings: Vec<String>,
    pub tableau_semantics: String,
}
/// Compact comparison authority retained after the original index is released.
struct Prepared {
    changes: ChangeSet,
    before: BTreeMap<String, Value>,
    expected: BTreeMap<String, Value>,
    patches: Vec<Patch>,
    warnings: Vec<String>,
    twb_sha256: String,
    before_checks: crate::validation::LocalValidation,
}
/// Borrowed convenience path for callers that intentionally retain the source.
#[allow(dead_code)]
pub fn plan(input: &str, package_sha256: &str, book: &Workbook, changes: ChangeSet, cfg: &Config) -> Result<(Plan, Workbook)> {
    let prepared = prepare(package_sha256, book, changes, cfg)?;
    let candidate = emit_candidate(&book.xml.text, &prepared, cfg)?;
    complete(input, prepared, candidate, cfg)
}
/// Product path: preserve independently, then release the source before candidate admission.
pub fn plan_owned(input: &str, package_sha256: &str, book: Workbook, changes: ChangeSet, cfg: &Config) -> Result<(Plan, Workbook)> {
    let prepared = prepare(package_sha256, &book, changes, cfg)?;
    let source = book.into_xml().into_text();
    let candidate = emit_candidate(&source, &prepared, cfg)?;
    drop(source);
    complete(input, prepared, candidate, cfg)
}
fn prepare(package_sha256: &str, book: &Workbook, changes: ChangeSet, cfg: &Config) -> Result<Prepared> {
    require(changes.schema_version == 1, "SCHEMA_VERSION", "Unsupported changeset schema")?;
    require(changes.input_sha256 == package_sha256, "STALE_BASE", "Input artifact hash differs from the inspected snapshot")?;
    require(!changes.operations.is_empty() && changes.operations.len() <= cfg.limits.max_operations, "LIMIT", "Changeset operation count is invalid")?;
    book.require_2025()?;
    let before = book.snapshot()?;
    let mut expected = BTreeMap::new(); // Sparse requested changes, not a second full snapshot.
    let mut patches = Vec::new();
    let mut warnings = Vec::new();
    let mut touched = BTreeSet::new();
    for operation in &changes.operations {
        let key = match operation {
            Operation::SetCalculation {
                field_id,
                ..
            }
            | Operation::SetParameter {
                field_id,
                ..
            }
            => format!("field/{field_id}"),
            Operation::SetFilterValues {
                filter_id,
                ..
            }
            | Operation::SetFilterRange {
                filter_id,
                ..
            }
            => format!("filter/{filter_id}"),
        };
        require(touched.insert(key), "CONFLICTING_OPERATIONS", "A changeset can change each field/filter only once; combine parameter current/domain into one operation")?;
        match operation {
            Operation::SetCalculation {
                field_id,
                expected_formula,
                formula: new
            }
            => {
                require(cfg.policy.allow_unverified_formula_edits, "POLICY", "Operator must acknowledge lexical-only formula checks in config before formula edits")?;
                let field = book.field(*field_id)?;
                require(!field.parameter, "TYPE_MISMATCH", "Use set_parameter for parameters")?;
                require(field.formula.as_deref() == Some(expected_formula), "STALE_PRECONDITION", "Calculation formula changed")?;
                let analysis = formula::analyze(new)?;
                for reference in &analysis.references {
                    book.resolve(field.datasource, reference)?;
                }
                warnings.extend(analysis.warnings);
                warnings.push("Formula semantics/result type/aggregation need verification in Tableau 2025; local checks are not a Tableau compiler".into());
                for &column in &field.copies {
                    let calc = book.xml.one_child(column, "calculation")?;
                    require(book.xml.value(calc, "class") == Some("tableau"), "UNSUPPORTED_SHAPE", "Only Tableau calculations are editable")?;
                    require(book.xml.required(calc, "formula")? == expected_formula, "INCONSISTENT_DEFINITION", "A dependency copy has a different calculation")?;
                    set_attr(book, calc, "formula", new, "calculation formula and dependency copies", &mut patches)?;
                }
                require(!field.copies.is_empty(), "UNSUPPORTED_SHAPE", "No editable definition")?;
                expected.insert(format!("field/{field_id}/formula"), json!(new));
            }
            Operation::SetParameter {
                field_id,
                expected_state_hash,
                current,
                domain
            }
            => {
                require(current.is_some() || domain.is_some(), "INVALID_ARGUMENT", "Specify current and/or domain")?;
                let field = book.field(*field_id)?;
                let (old_value, old_domain) = book.parameter_state(*field_id)?;
                require(state_hash(&json!({
                    "current":old_value,
                    "domain":old_domain
                })) == *expected_state_hash, "STALE_PRECONDITION", "Parameter state changed")?;
                let new_value = current.as_ref().unwrap_or(&old_value);
                let new_domain = domain.as_ref().unwrap_or(&old_domain);
                new_domain.accepts(new_value, &field.datatype)?;
                require(std::mem::discriminant(&old_domain) == std::mem::discriminant(new_domain), "UNSUPPORTED_SHAPE", "v1 changes static domains in place, not domain kinds")?;
                let value_literal = if new_value.cmp_value(&old_value)? != std::cmp::Ordering::Equal {
                    Some(new_value.literal(&field.datatype)?)
                } else { None };
                for &column in &field.copies {
                    let copied = book.parameter_at(column, &field.datatype)?;
                    require(copied.0 == old_value && copied.1 == old_domain, "INCONSISTENT_DEFINITION", "Parameter dependency copy differs from the primary")?;
                    if let Some(literal) = &value_literal {
                        set_attr(book, column, "value", literal, "parameter current value", &mut patches)?;
                        let calc = book.xml.one_child(column, "calculation")?;
                        set_attr(book, calc, "formula", literal, "parameter default calculation", &mut patches)?;
                    }
                    if old_domain != *new_domain {
                        set_domain(book, column, &field.datatype, new_domain, &mut patches)?;
                    }
                }
                expected.insert(format!("field/{field_id}/parameter"), json!({
                    "current":new_value,
                    "domain":new_domain
                }));
            }
            Operation::SetFilterValues {
                filter_id,
                expected_state_hash,
                values
            }
            => {
                let filter = book.filter(*filter_id)?;
                let old = book.filter_state(*filter_id)?;
                require(state_hash(&old) == *expected_state_hash, "STALE_PRECONDITION", "Filter state changed")?;
                require(filter.kind == "categorical", "TYPE_MISMATCH", "Expected categorical filter")?;
                require(!values.is_empty() && values.len() <= 10_000, "LIMIT", "Filter requires 1..10000 values; an empty filter is not implicit remove")?;
                require(values.iter().collect::<BTreeSet<_>>().len() == values.len(), "INVALID_ARGUMENT", "Duplicate categorical values")?;
                for v in values {
                    Scalar::String(v.clone()).literal("string")?;
                }
                let new = json!({
                    "kind":"categorical",
                    "values":values
                });
                if old != new {
                    let group = book.xml.one_child(filter.node, "groupfilter")?;
                    let level = book.categorical_level(*filter_id)?;
                    let replacement = categorical_fragment(book, group, &level, values)?;
                    replace_node(book, group, replacement, "categorical values", &mut patches);
                }
                expected.insert(format!("filter/{filter_id}/state"), new);
            }
            Operation::SetFilterRange {
                filter_id,
                expected_state_hash,
                min,
                max
            }
            => {
                let filter = book.filter(*filter_id)?;
                let old = book.filter_state(*filter_id)?;
                require(state_hash(&old) == *expected_state_hash, "STALE_PRECONDITION", "Filter state changed")?;
                require(filter.kind == "quantitative", "TYPE_MISMATCH", "Expected an in-range filter")?;
                let dtype = &book.field(filter.field.ok_or_else(|| Error::new("UNRESOLVED_REFERENCE", "Unknown filter field"))?)?.datatype;
                Domain::Range {
                    min: min.clone(),
                    max: max.clone(),
                    step: None
                }.accepts(min, dtype)?;
                for (tag, value) in [("min",min),("max",max)] {
                    let n = book.xml.one_child(filter.node, tag)?;
                    require(book.xml.children(n).next().is_none() && book.xml.node(n).attributes.is_empty(), "UNSUPPORTED_SHAPE", "Range bounds must be plain text elements")?;
                    let literal = escape_text(&value.literal(dtype)?)?;
                    if Scalar::parse(dtype, &book.xml.text_content(n)?)? != *value {
                        replace_node(book, n, format!("<{tag}>{literal}</{tag}>"), "range bound", &mut patches);
                    }
                }
                expected.insert(format!("filter/{filter_id}/state"), json!({
                    "kind":"range",
                    "min":min,
                    "max":max
                }));
            }
        }
    }
    patches.sort_by_key(|p| (p.span.start,p.span.end));

    Ok(Prepared { changes, before, expected, patches, warnings,
        twb_sha256: book.xml.sha256.clone(), before_checks: crate::validation::local(book) })
}
fn emit_candidate(source: &str, p: &Prepared, cfg: &Config) -> Result<String> {
    let candidate = patch::apply(source, &p.twb_sha256, &p.patches, cfg.limits.xml_bytes)?;
    patch::verify_preservation(source, &candidate, &p.patches)?;
    Ok(candidate)
}
fn complete(input: &str, p: Prepared, candidate: String, cfg: &Config) -> Result<(Plan, Workbook)> {
    let Prepared { changes, before, expected, patches, mut warnings, twb_sha256, before_checks } = p;
    let after = Workbook::parse(candidate.into_bytes(), &cfg.limits)?;
    after.require_acyclic()?;
    let actual = after.snapshot()?;
    require(actual.len() == before.len() && actual.keys().eq(before.keys()) &&
        before.iter().all(|(key, old)| actual.get(key) == Some(expected.get(key).unwrap_or(old))),
        "UNEXPECTED_SEMANTIC_DELTA", "Candidate semantic snapshot differs from the requested changes")?;
    // Verify the final batch: another edit may have changed transitive dependencies.
    let changed_calculations:Vec<FieldId>=changes.operations.iter().filter_map(|op|match op {
        Operation::SetCalculation{field_id,expected_formula,formula} if formula!=expected_formula=>Some(*field_id),
        _=>None,
    }).collect();
    after.require_calculation_dependencies(&changed_calculations)?;
    reject_new_diagnostics(&before_checks, &after)?;
    // Check that any mirrored definitions also reflect the candidate primary state.
    verify_changed_copies(&after, &changes.operations)?;
    let delta = diff(&before, &actual);
    warnings.sort();
    warnings.dedup();
    let result = Plan {
        schema_version: 1,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        input: input.into(),
        changes,
        twb_sha256,
        candidate_twb_sha256: after.xml.sha256.clone(),
        patches,
        delta,
        warnings,
        tableau_semantics: "not_run".into()
    };
    Ok((result,after))
}
fn set_attr(book: &Workbook, node: NodeId, name: &str, value: &str, reason: &str, patches: &mut Vec<Patch>) -> Result<()> {
    let a = book.xml.attr(node, name).ok_or_else(|| Error::new("UNSUPPORTED_SHAPE", format!("Required attribute {name} is absent")))?;
    if a.value == value {
        return Ok(());
    }
    patches.push(Patch {
        span:a.span,
        expected:book.xml.text[a.span.range()].into(),
        replacement:escape_attribute(value,a.quote)?,
        reason:reason.into()
    });
    Ok(())
}
fn set_scalar_attr(book:&Workbook,node:NodeId,name:&str,new:&Scalar,dtype:&str,reason:&str,patches:&mut Vec<Patch>)->Result<()> {
    let old=Scalar::parse(dtype,book.xml.required(node,name)?)?;
    if old.cmp_value(new)? == std::cmp::Ordering::Equal {
        return Ok(());
    }
    set_attr(book,node,name,&new.literal(dtype)?,reason,patches)
}
fn replace_node(book: &Workbook, node: NodeId, replacement: String, reason: &str, patches: &mut Vec<Patch>) {
    let span = book.xml.node(node).span;
    if book.xml.text[span.range()] == replacement {
        return;
    }
    patches.push(Patch {
        span,
        expected:book.xml.text[span.range()].into(),
        replacement,
        reason:reason.into()
    });
}
fn set_domain(book: &Workbook, column: NodeId, dtype: &str, domain: &Domain, patches: &mut Vec<Patch>) -> Result<()> {
    match domain {
        Domain::Any => {
        },
        Domain::Range {
            min,
            max,
            step
        }
        => {
            let r = book.xml.one_child(column,"range")?;
            set_scalar_attr(book,r,"min",min,dtype,"parameter minimum",patches)?;
            set_scalar_attr(book,r,"max",max,dtype,"parameter maximum",patches)?;
            match (book.xml.value(r,"granularity"),step) {
                (Some(_),Some(v)) => set_scalar_attr(book,r,"granularity",v,dtype,"parameter granularity",patches)?,
                (None,None) => {
                },
                _ => return Err(Error::new("UNSUPPORTED_SHAPE","Adding/removing granularity is outside the v1 shape contract")),
            }
        }
        Domain::List {
            values
        }
        => {
            let m = book.xml.one_child(column,"members")?;
            require(book.xml.node(m).attributes.is_empty(),"UNSUPPORTED_SHAPE","Unknown members-container metadata")?;
            let mut old = BTreeMap::new();
            for member in book.xml.children(m) {
                require(book.xml.tag(member)=="member" && book.xml.children(member).next().is_none(),"UNSUPPORTED_SHAPE","Unknown list member structure")?;
                for a in book.xml.attributes(member) {
                    require(matches!(a.name,"value"|"alias"),"UNSUPPORTED_SHAPE","Unknown list member metadata")?;
                }
                old.insert(Scalar::parse(dtype,book.xml.required(member,"value")?)?.literal(dtype)?,member);
            }
            let mut out=String::from("<members>");
            for v in values {
                let literal=v.literal(dtype)?;
                if let Some(n)=old.get(&literal) {
                    out.push_str(&book.xml.text[book.xml.node(*n).span.range()]);
                }
                else {
                    out.push_str(&format!("<member value=\"{}\"/>",escape_attribute(&literal,b'"')?));
                }
            }
            out.push_str("</members>");
            replace_node(book,m,out,"parameter static domain",patches);
        }
    }
    Ok(())
}
fn categorical_fragment(book:&Workbook,group:NodeId,level:&str,values:&[String])->Result<String>{
    let level=escape_attribute(level,b'"')?;
    let mut ui=String::new();
    for a in book.xml.attributes(group) {
        if a.name.starts_with("user:ui-") {
            ui.push_str(&format!(" {}=\"{}\"",a.name,escape_attribute(a.value,b'"')?));
        }
    }
    let member=|v:&str|->Result<String>{
        Ok(format!("<groupfilter function=\"member\" level=\"{level}\" member=\"{}\"/>",escape_attribute(&Scalar::String(v.into()).literal("string")?,b'"')?))
    };
    if values.len()==1 {
        let literal=escape_attribute(&Scalar::String(values[0].clone()).literal("string")?,b'"')?;
        Ok(format!("<groupfilter function=\"member\" level=\"{level}\" member=\"{literal}\"{ui}/>"))
    }else{
        let mut out=format!("<groupfilter function=\"union\"{ui}>");
        for v in values {
            out.push_str(&member(v)?);
        }
        out.push_str("</groupfilter>");
        Ok(out)
    }
}
fn reject_new_diagnostics(before_checks:&crate::validation::LocalValidation,after:&Workbook)->Result<()> {
    let mut counts=BTreeMap::new();
    let after_checks = crate::validation::local(after);
    for d in &before_checks.diagnostics {
        *counts.entry(d).or_insert(0usize)+=1;
    }
    for d in &after_checks.diagnostics {
        let n=counts.entry(d).or_insert(0);
        require(*n>0,"NEW_DIAGNOSTIC",format!("{}: {}",d.object,d.message))?;
        *n-=1;
    }
    Ok(())
}
fn verify_changed_copies(book:&Workbook,operations:&[Operation])->Result<()> {
    for op in operations {
        match op {
            Operation::SetCalculation{
                field_id,
                formula,
                ..
            }
            =>{
                for &n in &book.field(*field_id)?.copies {
                    let c=book.xml.one_child(n,"calculation")?;
                    require(book.xml.value(c,"formula")==Some(formula.as_str()),"POSTCONDITION","Calculation dependency copy was not updated")?;
                }
            }
            Operation::SetParameter{
                field_id,
                ..
            }
            =>{
                let f=book.field(*field_id)?;
                let expected=book.parameter_state(*field_id)?;
                for &n in &f.copies {
                    require(book.parameter_at(n,&f.datatype)?==expected,"POSTCONDITION","Parameter dependency copy was not updated")?;
                }
            }
            _=>{
            },
        }
    }
    Ok(())
}
pub fn diff(before:&BTreeMap<String,Value>,after:&BTreeMap<String,Value>)->Vec<Delta>{
    let keys:BTreeSet<_>=before.keys().chain(after.keys()).collect();
    keys.into_iter().filter_map(|key|{
        let a=before.get(key).unwrap_or(&Value::Null);
        let b=after.get(key).unwrap_or(&Value::Null);
        (a!=b).then(||Delta{
            object:key.clone(),
            property:"value".into(),
            before:a.clone(),
            after:b.clone()
        })
    }).collect()
}
