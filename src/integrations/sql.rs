//! Conservative SELECT admission. This is not a general Hyper SQL compiler.
use crate::error::{
    Error,
    Result,
    require
};
use sqlparser::{
    ast::{
        Expr,
        ObjectName,
        Query,
        SetExpr,
        Statement,
        TableFactor,
        Visit,
        Visitor
    },
    dialect::PostgreSqlDialect,
    parser::Parser
};
use std::{
    collections::BTreeSet,
    ops::ControlFlow
};
// Keep PostgreSQL token/precedence behavior, but disable speculative expression-named
// function arguments. They are outside our SELECT contract and sqlparser 0.53
// reparses nested expressions exponentially on that optional branch (fuzz regression).
#[derive(Debug)]
struct ReadOnlyPostgres;
impl sqlparser::dialect::Dialect for ReadOnlyPostgres {
    fn dialect(&self)->std::any::TypeId { std::any::TypeId::of::<PostgreSqlDialect>() }
    fn identifier_quote_style(&self, _identifier: &str)->Option<char> { PostgreSqlDialect{}.identifier_quote_style(_identifier) }
    fn is_delimited_identifier_start(&self, ch: char)->bool { PostgreSqlDialect{}.is_delimited_identifier_start(ch) }
    fn is_identifier_start(&self, ch: char)->bool { PostgreSqlDialect{}.is_identifier_start(ch) }
    fn is_identifier_part(&self, ch: char)->bool { PostgreSqlDialect{}.is_identifier_part(ch) }
    fn supports_unicode_string_literal(&self)->bool { PostgreSqlDialect{}.supports_unicode_string_literal() }
    fn is_custom_operator_part(&self, ch: char)->bool { PostgreSqlDialect{}.is_custom_operator_part(ch) }
    fn get_next_precedence(&self, parser: &Parser)->Option<std::result::Result<u8, sqlparser::parser::ParserError>> { PostgreSqlDialect{}.get_next_precedence(parser) }
    fn parse_statement(&self, parser: &mut Parser)->Option<std::result::Result<Statement, sqlparser::parser::ParserError>> { PostgreSqlDialect{}.parse_statement(parser) }
    fn supports_filter_during_aggregation(&self)->bool { PostgreSqlDialect{}.supports_filter_during_aggregation() }
    fn supports_group_by_expr(&self)->bool { PostgreSqlDialect{}.supports_group_by_expr() }
    fn prec_value(&self, prec: sqlparser::dialect::Precedence)->u8 { PostgreSqlDialect{}.prec_value(prec) }
    fn allow_extract_custom(&self)->bool { PostgreSqlDialect{}.allow_extract_custom() }
    fn allow_extract_single_quotes(&self)->bool { PostgreSqlDialect{}.allow_extract_single_quotes() }
    fn supports_create_index_with_clause(&self)->bool { PostgreSqlDialect{}.supports_create_index_with_clause() }
    fn supports_explain_with_utility_options(&self)->bool { PostgreSqlDialect{}.supports_explain_with_utility_options() }
    fn supports_listen_notify(&self)->bool { PostgreSqlDialect{}.supports_listen_notify() }
    fn supports_factorial_operator(&self)->bool { PostgreSqlDialect{}.supports_factorial_operator() }
    fn supports_comment_on(&self)->bool { PostgreSqlDialect{}.supports_comment_on() }
    fn supports_load_extension(&self)->bool { PostgreSqlDialect{}.supports_load_extension() }
    fn supports_named_fn_args_with_colon_operator(&self)->bool { PostgreSqlDialect{}.supports_named_fn_args_with_colon_operator() }
    fn supports_named_fn_args_with_expr_name(&self)->bool { false }
}

pub struct ReadQuery {
    pub sql: String,
    pub tables: Vec<(String,String)>
}
struct Guard {
    tables:BTreeSet<(String,String)>,
    exprs:usize
}
fn reject(message:&str)->ControlFlow<Error> {
    ControlFlow::Break(Error::new("SQL_POLICY",message))
}
impl Visitor for Guard {
    type Break=Error;
    fn pre_visit_query(&mut self,q:&Query)->ControlFlow<Error>{
        if q.with.is_some()||!q.locks.is_empty()||q.for_clause.is_some()||q.settings.is_some()||q.format_clause.is_some()||!q.limit_by.is_empty(){
            return reject("CTEs, locking, settings and output clauses are outside the read-only subset");
        }
        match q.body.as_ref(){
            SetExpr::Select(s)=>{
                if s.into.is_some(){
                    return reject("SELECT INTO is forbidden");
                }
            },
            _=>return reject("Only SELECT query bodies are supported; no table/values/DML/set operations"),
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_statement(&mut self,s:&Statement)->ControlFlow<Error>{
        if !matches!(s,Statement::Query(_)){
            return reject("Exactly one SELECT statement is required");
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_table_factor(&mut self,t:&TableFactor)->ControlFlow<Error>{
        match t{
            TableFactor::Table{
                name,
                args,
                with_hints,
                version,
                ..
            }
            if args.is_none()&&with_hints.is_empty()&&version.is_none()=>{
                if name.0.len()!=2{
                    return reject("Use an explicitly quoted schema.table; no implicit schemas or cross-database names");
                }
                let schema=&name.0[0];
                let table=&name.0[1];
                if schema.quote_style!=Some('"')||table.quote_style!=Some('"'){
                    return reject("Schema and table identifiers must use double quotes");
                }
                if schema.value.is_empty()||table.value.is_empty()||schema.value.contains(['\0','\n','\r'])||table.value.contains(['\0','\n','\r']){
                    return reject("Invalid relation name");
                }
                if schema.value.to_ascii_lowercase().starts_with("pg_")||schema.value.eq_ignore_ascii_case("information_schema"){
                    return reject("System catalogs are exposed only through typed metadata tools");
                }
                self.tables.insert((schema.value.clone(),table.value.clone()));
            },
            _=>return reject("Only ordinary base-table references are allowed, not table functions/derived tables"),
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_relation(&mut self,_:&ObjectName)->ControlFlow<Error>{
        ControlFlow::Continue(())
    }
    fn pre_visit_expr(&mut self,e:&Expr)->ControlFlow<Error>{
        self.exprs+=1;
        if self.exprs>10000{
            return reject("Expression budget exceeded");
        }
        match e {
            Expr::Identifier(_)|Expr::CompoundIdentifier(_)|Expr::Value(_)|Expr::Nested(_)|Expr::BinaryOp{
                ..
            }
            |Expr::UnaryOp{
                ..
            }
            |
            Expr::IsNull(_)|Expr::IsNotNull(_)|Expr::Between{
                ..
            }
            |Expr::InList{
                ..
            }
            |Expr::Like{
                ..
            }
            |Expr::ILike{
                ..
            }
            |Expr::Case{
                ..
            }
            =>{
            },
            Expr::Function(f)=>{
                const FUNCTIONS:&[&str]=&["COUNT","SUM","MIN","MAX","AVG","ABS","ROUND","FLOOR","CEIL","CEILING","COALESCE","NULLIF","LOWER","UPPER","LENGTH","TRIM","LTRIM","RTRIM","SUBSTRING","REPLACE"];
                if f.name.0.len()!=1||!FUNCTIONS.contains(&f.name.0[0].value.to_ascii_uppercase().as_str()) {
                    return reject("Function is not on the audited read-only allowlist");
                }
            },
            _=>return reject("Expression is outside the conservative SELECT subset"),
        }
        ControlFlow::Continue(())
    }
}
pub fn admit(sql:&str,rows:usize)->Result<ReadQuery>{
    require(!sql.is_empty()&&sql.len()<=65536,"SQL_POLICY","SQL size must be 1..65536 bytes")?;
    require(rows>0&&rows<=100000,"SQL_POLICY","Invalid row budget")?;
    // Parser recursion limit is enforced before visitor recursion.
    let mut statements=Parser::new(&ReadOnlyPostgres).with_recursion_limit(64).try_with_sql(sql)
    .and_then(|mut p|p.parse_statements()).map_err(|_|Error::new("SQL_SYNTAX","SQL is not in the supported PostgreSQL-compatible SELECT syntax"))?;
    require(statements.len()==1,"SQL_POLICY","Exactly one SELECT statement is required")?;
    let statement=statements.remove(0);
    let mut guard=Guard{
        tables:BTreeSet::new(),
        exprs:0
    };
    if let ControlFlow::Break(e)=statement.visit(&mut guard){
        return Err(e);
    }
    require(!guard.tables.is_empty(),"SQL_POLICY","Query must address an extract base table")?;
    // The outer LIMIT bounds rows transferred; it does not bound scan work. The worker timeout does.
    Ok(ReadQuery{
        sql:format!("SELECT * FROM ({statement}) AS \"tabkit_result\" LIMIT {}",rows+1),
        tables:guard.tables.into_iter().collect()
    })
}
pub fn identifier(s:&str)->Result<String>{
    require(!s.is_empty()&&s.len()<=512&&!s.contains(['\0','\n','\r']),"SQL_POLICY","Invalid SQL identifier")?;
    Ok(format!("\"{}\"",s.replace('"',"\"\"")))
}
