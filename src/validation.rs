//! One typed local-validation result shared by inspect, assertions and publish.
//! Unsupported syntax is not a known-invalid value, and neither is Tableau execution.
use crate::{error::{Error,Result},workbook::{Workbook,FieldId,Diagnostic}};
use serde::{Serialize,Deserialize};

#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Status { Valid, Partial, Invalid }
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalValidation {
    pub passed:bool,
    pub status:Status,
    pub diagnostics:Vec<Diagnostic>,
    pub unsupported_objects:Vec<Diagnostic>,
    pub static_parameters:usize,
    pub simple_filters:usize,
}
impl LocalValidation {
    pub fn require_passed(&self)->Result<()> {
        if self.passed { return Ok(()); }
        Err(Error::new("LOCAL_VALIDATION_FAILED","Known local errors must be repaired before publication")
            .details(serde_json::json!({"count":self.diagnostics.len(),"diagnostics":self.diagnostics.iter().take(8).collect::<Vec<_>>()})))
    }
}
fn unsupported(code:&str)->bool { code.starts_with("UNSUPPORTED_") || code=="VERSION_UNVERIFIED" }
fn record(r:&mut LocalValidation,object:String,error:Error) {
    let d=Diagnostic{code:error.code.into(),object,message:error.message};
    if unsupported(error.code) { r.unsupported_objects.push(d); } else { r.diagnostics.push(d); }
}
pub fn local(book:&Workbook)->LocalValidation {
    let mut r=LocalValidation{passed:true,status:Status::Valid,diagnostics:Vec::new(),
        unsupported_objects:Vec::new(),static_parameters:0,simple_filters:0};
    for d in &book.diagnostics {
        if unsupported(&d.code) { r.unsupported_objects.push(d.clone()); }
        else { r.diagnostics.push(d.clone()); }
    }
    if let Err(e)=book.require_acyclic() { record(&mut r,"workbook".into(),e); }
    for (i,f) in book.fields.iter().enumerate() {
        let id=FieldId(i as u32);
        let object=book.field_key(id);
        if f.parameter {
            match book.parameter_state(id) {
                Ok(primary)=>{
                    r.static_parameters+=1;
                    for &node in &f.copies {
                        match book.parameter_at(node,&f.datatype) {
                            Ok(copy) if copy!=primary=>record(&mut r,object.clone(),Error::new("INCONSISTENT_DEFINITION","Parameter copy differs from its primary definition")),
                            Err(e)=>record(&mut r,object.clone(),e),
                            _=>{},
                        }
                    }
                }
                Err(e)=>record(&mut r,object,e),
            }
        } else if let Some(formula)=&f.formula {
            for &column in &f.copies {
                match book.xml.one_child(column,"calculation") {
                    Ok(calc)=>{
                        if book.xml.value(calc,"class")!=Some("tableau") {
                            record(&mut r,object.clone(),Error::new("UNSUPPORTED_CALCULATION_CLASS","Non-Tableau calculation is preserved without semantic validation"));
                        } else if book.xml.value(calc,"formula")!=Some(formula.as_str()) {
                            record(&mut r,object.clone(),Error::new("INCONSISTENT_DEFINITION","Calculation copy differs from its primary formula"));
                        }
                    }
                    Err(_)=>record(&mut r,object.clone(),Error::new("INCONSISTENT_DEFINITION","Known calculation copy lacks a single formula definition")),
                }
            }
        }
    }
    for filter in &book.filters {
        match book.filter_state(filter.id) {
            Ok(_)=>r.simple_filters+=1,
            Err(e)=>record(&mut r,format!("filter/{}",filter.id),e),
        }
    }
    for d in &book.local_definitions {
        record(&mut r,format!("local_definition/{}",d.node_id.0),Error::new("UNSUPPORTED_LOCAL_DEFINITION",d.reason.clone()));
    }
    r.diagnostics.sort(); r.diagnostics.dedup();
    r.unsupported_objects.sort(); r.unsupported_objects.dedup();
    r.passed=r.diagnostics.is_empty();
    r.status=if !r.passed {Status::Invalid} else if !r.unsupported_objects.is_empty(){Status::Partial}else{Status::Valid};
    r
}
