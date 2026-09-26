//! Focused Tableau Server 2025 REST adapter. No arbitrary endpoint proxy.
//! Side-effecting requests are never automatically retried.
use crate::{
    config::{
        Config,
        TableauAuth,
        TableauConfig
    },
    error::{
        Error,
        Result,
        require
    },
    fs::{
        hash_file,
        sync_dir
    },
    xml
};
use reqwest::{
    blocking::{
        Client,
        RequestBuilder,
        Response
    },
    header::{
        HeaderValue,
        ACCEPT,
        CONTENT_TYPE
    },
    Method,
    Url
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
use sha2::{
    Digest,
    Sha256
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{
        Read,
        Write
    },
    path::Path,
    time::Duration
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;
#[derive(Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Projects,
    Workbooks,
    Views,
    Datasources
}
impl Resource {
    pub fn plural(self) -> &'static str {
        match self {
            Self::Projects=>"projects",
            Self::Workbooks=>"workbooks",
            Self::Views=>"views",
            Self::Datasources=>"datasources"
        }
    }
    fn singular(self) -> &'static str {
        match self {
            Self::Projects=>"project",
            Self::Workbooks=>"workbook",
            Self::Views=>"view",
            Self::Datasources=>"datasource"
        }
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Explore {
    pub resource: Resource,
    #[serde(default = "one")] pub page: u32,
    #[serde(default = "hundred")] pub page_size: u32,
    /// Only meaningful for workbooks/datasources. IDs come from explore, never names.
    pub project_id: Option<String>,
    /// Only meaningful for views. Lists views of this exact workbook.
    pub workbook_id: Option<String>,
}
fn one()->u32 {
    1
}
fn hundred()->u32 {
    100
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Search {
    pub resource: Resource,
    pub text: String,
    pub project_id: Option<String>
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewExport {
    pub view_id: String,
    pub output: String,
    #[serde(default)] pub filters: BTreeMap<String,
    String>,
}
pub struct Rest {
    cfg: TableauConfig,
    client: Client,
    token: Option<Zeroizing<String>>,
    site_id: String,
    user_id: String,
    max_json: usize,
    auth_timeout: Duration,
}
pub fn cancelled(ct: &CancellationToken) -> Result<()> {
    require(!ct.is_cancelled(), "CANCELLED", "Operation cancelled before the next side effect")
}
pub fn luid(s: &str) -> Result<()> {
    uuid::Uuid::parse_str(s).map_err(|_| Error::new("ID", "Expected a Tableau LUID (UUID), not a display name"))?;
    Ok(())
}
fn object_string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
    .ok_or_else(|| Error::new("REST_SHAPE", format!("Missing response property {key}")))
}
fn unsigned(v: &Value) -> Option<u64> {
    v.as_u64().or_else(||v.as_str().and_then(|s|s.parse().ok()))
}
impl Rest {
    pub fn new(cfg: &Config) -> Result<Self> {
        let t = cfg.tableau.clone().ok_or_else(|| Error::new("CAPABILITY_UNAVAILABLE", "Tableau REST is not configured"))?;
        let mut builder = Client::builder().https_only(true).redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(cfg.limits.request_seconds))
        .user_agent(concat!("tabkit/", env!("CARGO_PKG_VERSION")));
        if let Some(p) = &t.ca_certificate {
            let pem = crate::fs::read_bounded(p, 1 << 20)?;
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&pem)
            .map_err(|_|Error::new("TLS_CONFIG", "Invalid PEM CA certificate"))?);
        }
        Ok(Self {
            cfg:t,
            client:builder.build()?,
            token:None,
            site_id:String::new(),
            user_id:String::new(),
            max_json:cfg.limits.result_bytes,
            auth_timeout:Duration::from_secs(cfg.limits.request_seconds)
        })
    }
    fn url(&self, site: bool, parts: &[&str]) -> Result<Url> {
        let mut url = Url::parse(&self.cfg.server).map_err(|_| Error::new("CONFIG", "Invalid server URL"))?;
        {
            let mut path = url.path_segments_mut().map_err(|_|Error::new("CONFIG", "Invalid URL base"))?;
            path.pop_if_empty().push("api").push(&self.cfg.api_version);
            if site {
                require(!self.site_id.is_empty(), "AUTH_REQUIRED", "Sign in before using Tableau tools")?;
                path.push("sites").push(&self.site_id);
            }
            for p in parts {
                path.push(p);
            }
        }
        Ok(url)
    }
    fn request(&self, method: Method, url: Url) -> Result<RequestBuilder> {
        let token = self.token.as_ref().ok_or_else(|| Error::new("AUTH_REQUIRED", "Sign in first"))?;
        let mut header = HeaderValue::from_str(token.as_str()).map_err(|_|Error::new("AUTH", "Invalid session header"))?;
        header.set_sensitive(true);
        Ok(self.client.request(method, url).header("X-Tableau-Auth", header).header(ACCEPT, "application/json"))
    }
    fn response_json(&mut self, mut response: Response) -> Result<Value> {
        let status = response.status();
        // Invalidate before reading the body: a malformed/oversized 401 cannot
        // leave a known-rejected credential cached. Never replay this request.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            self.clear_session();
            return Err(Error::new("AUTH_EXPIRED", "Tableau rejected the session; the next operation must sign in again")
                .details(json!({"status":401,"session_invalidated":true,"request_replayed":false})));
        }
        let mut data=Vec::new();
        response.by_ref().take(self.max_json as u64 + 1).read_to_end(&mut data)?;
        require(data.len()<=self.max_json,"RESULT_LIMIT","REST response exceeds result byte limit")?;
        if !status.is_success() {
            // Server detail text can echo connection strings or credentials. Expose only stable code.
            let code = crate::wire::decode::<Value>(&data,self.max_json as u64).ok()
            .and_then(|j|j.pointer("/error/code").and_then(Value::as_str).map(str::to_owned));
            return Err(Error::new("REST_REJECTED",format!("Tableau returned HTTP {}",status.as_u16()))
            .details(json!({
                "status":status.as_u16(),
                "tableau_code":code
            })));
        }
        if data.is_empty() {
            return Ok(json!({
            }));
        }
        crate::wire::decode(&data,self.max_json as u64).map_err(|_|Error::new("REST_SHAPE","Expected a JSON Tableau response"))
    }
    fn send_json(&mut self, req: RequestBuilder, side_effect: bool, ct:&CancellationToken)->Result<Value> {
        cancelled(ct)?;
        let response=req.send().map_err(|e| if side_effect {
            Error::new("HTTP_OUTCOME_UNKNOWN", "A side-effecting request may have reached Tableau; do not automatically retry")
        } else {
            e.into()
        })?;
        self.response_json(response)
    }
    pub fn login(&mut self, ct:&CancellationToken)->Result<Value> {
        require(self.token.is_none(),"AUTH_ACTIVE","A session is already active; log out explicitly first")?;
        let (body, auth_kind) = match &self.cfg.auth {
            TableauAuth::OAuth(oauth) => {
                // Browser SSO obtains a short-lived JWT from the operator-configured corporate IdP.
                // Tableau connected-app/EAS trust validates that JWT and exchanges it for a Tableau REST session token.
                let jwt=crate::oauth::acquire_jwt(&self.client,oauth,self.auth_timeout,ct)?;
                (json!({
                    "credentials":{
                        "jwt":jwt.as_str(),
                        "site":{"contentUrl":self.cfg.site}
                    }
                }), "oauth_pkce_connected_app")
            }
            TableauAuth::Pat(pat) => {
                // PAT is an explicit fallback for installations without a connected app/EAS.
                // The secret stays outside MCP args and process argv; only the env variable name is startup config.
                let secret=Zeroizing::new(std::env::var(&pat.secret_env).map_err(|_|Error::new("AUTH_CONFIG",format!("PAT secret environment variable {} is not set",pat.secret_env)))?);
                require(!secret.trim().is_empty(),"AUTH_CONFIG","PAT secret is empty")?;
                (json!({
                    "credentials":{
                        "personalAccessTokenName":pat.name.as_str(),
                        "personalAccessTokenSecret":secret.as_str(),
                        "site":{"contentUrl":self.cfg.site}
                    }
                }), "pat")
            }
        };
        let req=self.client.post(self.url(false,&["auth","signin"])?).header(ACCEPT,"application/json").json(&body);
        let mut j=self.send_json(req,true,ct)?;
        let c=j.get_mut("credentials").ok_or_else(||Error::new("REST_SHAPE","Sign-in credentials object missing"))?;
        let token=object_string(c,"token")?.to_owned();
        let site=c.pointer("/site/id").and_then(Value::as_str).ok_or_else(||Error::new("REST_SHAPE","Site id missing"))?.to_owned();
        let user=c.pointer("/user/id").and_then(Value::as_str).unwrap_or("").to_owned();
        luid(&site)?;
        self.token=Some(Zeroizing::new(token));
        self.site_id=site;
        self.user_id=user;
        if let Some(v)=c.get_mut("token") {
            *v=Value::Null;
        }
        Ok(json!({
            "authenticated":true,
            "auth":auth_kind,
            "site_id":self.site_id,
            "user_id":self.user_id,
            "api_version":self.cfg.api_version
        }))
    }
    pub fn logout(&mut self,ct:&CancellationToken)->Result<Value> {
        if self.token.is_none() {
            return Ok(json!({
                "authenticated":false
            }));
        }
        let result=self.send_json(self.request(Method::POST,self.url(false,&["auth","signout"])?)?,true,ct);
        self.clear_session();
        result.map(|_|json!({
            "authenticated":false
        }))
    }
    fn clear_session(&mut self) {
        self.token = None;
        self.site_id.clear();
        self.user_id.clear();
    }
    pub fn authenticated(&self)->bool {
        self.token.is_some()
    }
    pub fn identity(&self)->Value {
        json!({
            "server":self.cfg.server,
            "site_id":self.site_id,
            "user_id":self.user_id,
            "api_version":self.cfg.api_version
        })
    }
    pub fn explore(&mut self,a:&Explore,ct:&CancellationToken)->Result<Page> {
        require((1..=1000).contains(&a.page_size) && a.page>0,"ARGUMENT","Invalid pagination")?;
        let mut url=if let Some(w)=&a.workbook_id {
            require(matches!(a.resource,Resource::Views),"ARGUMENT","workbook_id is only valid for views")?;
            luid(w)?;
            self.url(true,&["workbooks",w,"views"])?
        } else {
            self.url(true,&[a.resource.plural()])?
        };
        {
            let mut q=url.query_pairs_mut();
            q.append_pair("pageSize",&a.page_size.to_string()).append_pair("pageNumber",&a.page.to_string());
            if let Some(p)=&a.project_id {
                require(matches!(a.resource,Resource::Workbooks|Resource::Datasources),"ARGUMENT","project_id filter only supports workbooks/datasources")?;
                luid(p)?;
                q.append_pair("filter",&format!("projectId:eq:{p}"));
            }
        }
        let j=self.send_json(self.request(Method::GET,url)?,false,ct)?;
        require(j.get(a.resource.plural()).is_some(),"REST_SHAPE","List container missing from response")?;
        let raw=j.get(a.resource.plural()).and_then(|c|c.get(a.resource.singular()));
        let items:Vec<Value>=match raw {
            Some(Value::Array(v))=>v.iter().map(|v|metadata(v,a.resource)).collect(),
            Some(Value::Object(_))=>vec![metadata(raw.unwrap_or(&Value::Null),a.resource)],
            None|Some(Value::Null)=>Vec::new(),
            _=>return Err(Error::new("REST_SHAPE","Unexpected list response")),
        };
        for item in &items {
            require(item["id"].as_str().is_some(),"REST_SHAPE","List item lacks its identity")?;
        }
        let total=j.pointer("/pagination/totalAvailable").and_then(unsigned);
        let more=total.map(|t|a.page as u64 * (a.page_size as u64)<t).unwrap_or(items.len()==a.page_size as usize);
        let next=if more {
            Some(a.page.checked_add(1).ok_or_else(||Error::new("LIMIT","Page overflow"))?)
        }else{
            None
        };
        Ok(Page{items,page:a.page,page_size:a.page_size,total,next_page:next})
    }
    pub fn search(&mut self,a:&Search,max_pages:u32,ct:&CancellationToken)->Result<Value> {
        require(!a.text.trim().is_empty() && a.text.len()<=256,"ARGUMENT","Search text must be 1..256 bytes")?;
        let needle=a.text.to_lowercase();
        let mut found=Vec::new();
        let mut has_more=false;
        let mut scanned=0;
        let mut encoded=2usize;
        for page in 1..=max_pages {
            let j=self.explore(&Explore{
                resource:a.resource,
                page,
                page_size:100,
                project_id:a.project_id.clone(),
                workbook_id:None
            },ct)?;
            for item in j.items {
                scanned+=1;
                if item["name"].as_str().unwrap_or("").to_lowercase().contains(&needle) {
                    encoded=encoded.saturating_add(crate::wire::encoded_len(&item,self.max_json/2)?+1);
                    require(encoded<=self.max_json/2,"RESULT_LIMIT","Search matches exceed budget; narrow the query")?;
                    found.push(item);
                }
            }
            has_more=j.next_page.is_some();
            if !has_more {
                break;
            }
        }
        found.sort_by(|a,b|a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(json!({
            "items":found,
            "scanned":scanned,
            "truncated":has_more,
            "search_semantics":"case-insensitive name substring over bounded REST pages; not Cloud search"
        }))
    }
    pub fn workbook(&mut self,id:&str,ct:&CancellationToken)->Result<Value> {
        luid(id)?;
        let j=self.send_json(self.request(Method::GET,self.url(true,&["workbooks",id])?)?,false,ct)?;
        let w=j.get("workbook").ok_or_else(||Error::new("REST_SHAPE","Workbook object absent"))?;
        Ok(metadata(w,Resource::Workbooks))
    }
    pub fn job(&mut self,id:&str,ct:&CancellationToken)->Result<Value> {
        luid(id)?;
        let j=self.send_json(self.request(Method::GET,self.url(true,&["jobs",id])?)?,false,ct)?;
        let raw=j.get("job").ok_or_else(||Error::new("REST_SHAPE","Job object absent"))?;
        let mut o=serde_json::Map::new();
        for k in ["id","mode","type","progress","createdAt","startedAt","completedAt","finishCode"] {
            if let Some(v)=raw.get(k){
                o.insert(k.into(),v.clone());
            }
        }
        for k in ["publishWorkbook","workbook"] {
            if let Some(v)=raw.get(k){
                o.insert(k.into(),metadata(v,Resource::Workbooks));
            }
        }
        Ok(Value::Object(o))
    }
    /// Stage bytes to a temporary file. A download is not committed until format checks pass.
    pub fn download(&mut self,id:&str,output:&Path,cfg:&Config,ct:&CancellationToken)->Result<Value> {
        luid(id)?;
        let mut u=self.url(true,&["workbooks",id,"content"])?;
        u.query_pairs_mut().append_pair("includeExtract","true");
        self.export(u,output,cfg.limits.file_bytes,ExportKind::Workbook,cfg,ct)
    }
    pub fn view_export(&mut self,a:&ViewExport,output:&Path,image:bool,cfg:&Config,ct:&CancellationToken)->Result<Value> {
        require(cfg.policy.allow_data_output,"DATA_POLICY","Data/image output is disabled by operator configuration")?;
        luid(&a.view_id)?;
        require(a.filters.len()<=32,"LIMIT","Too many view filters")?;
        let mut u=self.url(true,&["views",&a.view_id,if image{
            "image"
        }else{
            "data"
        }])?;
        {
            let mut q=u.query_pairs_mut();
            q.append_pair("maxAge","1");
            if image {
                q.append_pair("resolution","high");
            }
            for(k,v)in&a.filters {
                require(!k.is_empty()&&k.len()<256&&v.len()<4096,"ARGUMENT","Invalid filter")?;
                q.append_pair(&format!("vf_{k}"),v);
            }
        }
        require(output.extension().and_then(|s|s.to_str())==Some(if image{
            "png"
        }else{
            "csv"
        }),"FILE_TYPE","Export output must use .png or .csv as appropriate")?;
        self.export(u,output,if image{
            cfg.limits.result_bytes as u64/2
        }else{
            cfg.limits.result_bytes as u64
        },if image{
            ExportKind::Png
        }else{
            ExportKind::Csv
        },cfg,ct)
    }
    fn export(&mut self,u:Url,output:&Path,limit:u64,kind:ExportKind,cfg:&Config,ct:&CancellationToken)->Result<Value> {
        cancelled(ct)?;
        let mut response=self.request(Method::GET,u)?.send()?;
        if !response.status().is_success(){
            return self.response_json(response).and_then(|_|Err(Error::new("REST_SHAPE","Unexpected download response")));
        }
        if let Some(n)=response.content_length(){
            require(n<=limit,"LIMIT","Download exceeds configured limit")?;
        }
        let parent=output.parent().ok_or_else(||Error::new("PATH","Missing output parent"))?;
        let suffix=match kind{
            ExportKind::Workbook=>if output.extension().and_then(|s|s.to_str())==Some("twbx"){
                ".twbx"
            }else{
                ".twb"
            },
            ExportKind::Png=>".png",
            ExportKind::Csv=>".csv"
        };
        let mut tmp=tempfile::Builder::new().suffix(suffix).tempfile_in(parent)?;
        let mut h=Sha256::new();
        let mut total=0u64;
        let mut buf=[0u8;65536];
        let mut prefix=Vec::new();
        loop {
            cancelled(ct)?;
            let n=response.read(&mut buf)?;
            if n==0{
                break;
            }
            total+=n as u64;
            require(total<=limit,"LIMIT","Download exceeded byte limit")?;
            if prefix.len()<128{
                prefix.extend_from_slice(&buf[..n.min(128-prefix.len())]);
            }
            h.update(&buf[..n]);
            tmp.write_all(&buf[..n])?;
        }
        require(total>0,"REST_SHAPE","Empty downloaded file")?;
        match kind {
            ExportKind::Png=>require(prefix.starts_with(b"\x89PNG\r\n\x1a\n"),"REST_SHAPE","Expected PNG image")?,
            ExportKind::Csv=>{
                require(!prefix.starts_with(b"<html")&&!prefix.starts_with(b"<!DOCTYPE"),"REST_SHAPE","HTML received instead of CSV")?;
            },
            ExportKind::Workbook=>{
                let expected=if prefix.starts_with(b"PK\x03\x04"){
                    "twbx"
                }else{
                    "twb"
                };
                require(output.extension().and_then(|s|s.to_str())==Some(expected),"FILE_TYPE",format!("Server returned {expected}; choose an output with that extension"))?;
                tmp.as_file().sync_all()?;
                crate::package::Package::open(tmp.path(),&cfg.limits)?;
                cancelled(ct)?;
                tmp.persist_noclobber(output).map_err(|e|Error::new("ATOMIC_WRITE",e.error.to_string()))?;
                let hash=crate::fs::digest_hex(h.finalize().into());
                sync_dir(parent).map_err(|_|Error::new("COMMIT_DURABILITY_UNKNOWN","Export exists but directory durability is unconfirmed")
                    .details(json!({"output_may_exist":true,"sha256":hash})))?;
                return Ok(json!({
                    "bytes":total,
                    "sha256":hash,
                    "format":expected,
                    "include_extract":true
                }));
            }
        }
        cancelled(ct)?;
        tmp.as_file().sync_all()?;
        tmp.persist_noclobber(output).map_err(|e|Error::new("ATOMIC_WRITE",e.error.to_string()))?;
        let hash=crate::fs::digest_hex(h.finalize().into());
        sync_dir(parent).map_err(|_|Error::new("COMMIT_DURABILITY_UNKNOWN","Export exists but directory durability is unconfirmed")
            .details(json!({"output_may_exist":true,"sha256":hash})))?;
        Ok(json!({
            "bytes":total,
            "sha256":hash,
            "format":if matches!(kind,ExportKind::Png){
                "png"
            }else{
                "csv"
            },
            "tableau_2025_note":"Dashboard CSV exports can represent only the first sheet; address each worksheet view explicitly"
        }))
    }
    /// Upload chunks sequentially: sequenceID is deliberately not used on REST 3.25.
    pub fn publish(&mut self,path:&Path,file_hash:&str,name:&str,project:&str,overwrite:bool,baseline:Option<&Value>,cfg:&Config,ct:&CancellationToken)->Result<Value> {
        luid(project)?;
        require(!name.trim().is_empty()&&name.len()<=256,"ARGUMENT","Invalid publish name")?;
        require(hash_file(path,cfg.limits.file_bytes)?==file_hash,"STALE_BASE","Publish candidate changed")?;
        let file_type=path.extension().and_then(|s|s.to_str()).unwrap_or("");
        require(matches!(file_type,"twb"|"twbx"),"FILE_TYPE","Publish supports TWB/TWBX")?;
        let j=self.send_json(self.request(Method::POST,self.url(true,&["fileUploads"])?)?,true,ct)?;
        let session=j.pointer("/fileUpload/uploadSessionId").and_then(Value::as_str).ok_or_else(||Error::new("REST_SHAPE","Upload session missing"))?;
        require(!session.is_empty()&&session.len()<=256,"REST_SHAPE","Invalid upload session")?;
        let mut input=File::open(path)?;
        let mut buffer=vec![0u8;8<<20];
        let mut digest=Sha256::new();
        let mut total=0u64;
        loop {
            cancelled(ct)?;
            let mut n=0;
            while n<buffer.len(){
                let got=input.read(&mut buffer[n..])?;
                if got==0{
                    break;
                }
                n+=got;
            }
            if n==0{
                break;
            }
            total+=n as u64;
            require(total<=cfg.limits.file_bytes,"LIMIT","Publish grew beyond limit")?;
            digest.update(&buffer[..n]);
            let(body,ctype)=mixed("",Some(("tableau_file","file",&buffer[..n])));
            let req=self.request(Method::PUT,self.url(true,&["fileUploads",session])?)?.header(CONTENT_TYPE,ctype).body(body);
            self.send_json(req,true,ct)?;
        }
        require(crate::fs::digest_hex(digest.finalize().into())==file_hash&&hash_file(path,cfg.limits.file_bytes)?==file_hash,"STALE_BASE","File changed during upload; upload will not be committed")?;
        if let Some(expected)=baseline {
            let id=expected["id"].as_str().ok_or_else(||Error::new("REST_SHAPE","Baseline id missing"))?;
            let current=self.workbook(id,ct)?;
            require(current==*expected,"REMOTE_CHANGED","Remote workbook changed while uploading; refusing commit")?;
        }
        let payload=format!("<tsRequest><workbook name=\"{}\"><project id=\"{}\" /></workbook></tsRequest>",xml::escape_attribute(name,b'"')?,project);
        let(body,ctype)=mixed(&payload,None);
        let mut u=self.url(true,&["workbooks"])?;
        u.query_pairs_mut().append_pair("uploadSessionId",session).append_pair("workbookType",file_type)
        .append_pair("overwrite",if overwrite{
            "true"
        }else{
            "false"
        }).append_pair("asJob","true");
        let j=self.send_json(self.request(Method::POST,u)?.header(CONTENT_TYPE,ctype).body(body),true,ct)?;
        if let Some(job)=j.get("job"){
            let id=object_string(job,"id")?;
            return Ok(json!({
                "status":"submitted",
                "job_id":id,
                "candidate_sha256":file_hash,
                "verified_in_tableau":false
            }));
        }
        if let Some(w)=j.get("workbook"){
            return Ok(json!({
                "status":"published",
                "workbook":metadata(w,Resource::Workbooks),
                "candidate_sha256":file_hash,
                "verified_in_tableau":false
            }));
        }
        Err(Error::new("HTTP_OUTCOME_UNKNOWN","Commit returned an unrecognized success response; inspect server before retrying"))
    }
}
#[derive(Clone,Copy)]enum ExportKind{
    Workbook,
    Png,
    Csv
}
fn metadata(v:&Value,kind:Resource)->Value {
    let mut o=serde_json::Map::new();
    o.insert("kind".into(),json!(kind.singular()));
    for k in ["id","name","contentUrl","createdAt","updatedAt","size","showTabs","webpageUrl","sheetType"]{
        if let Some(x)=v.get(k){
            if x.is_string()||x.is_number()||x.is_boolean(){
                o.insert(k.into(),x.clone());
            }
        }
    }
    for k in ["project","owner","workbook"]{
        if let Some(x)=v.get(k){
            let mut child=serde_json::Map::new();
            for attr in["id","name"]{
                if let Some(y)=x.get(attr){
                    child.insert(attr.into(),y.clone());
                }
            }
            o.insert(k.into(),Value::Object(child));
        }
    }
    Value::Object(o)
}
fn mixed(payload:&str,file:Option<(&str,&str,&[u8])>)->(Vec<u8>,String){
    let boundary=format!("tabkit-{}",uuid::Uuid::new_v4().simple());
    let mut b=format!("--{boundary}\r\nContent-Disposition: name=\"request_payload\"\r\nContent-Type: text/xml\r\n\r\n{payload}\r\n").into_bytes();
    if let Some((part,name,data))=file{
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: name=\"{part}\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        b.extend_from_slice(data);
        b.extend_from_slice(b"\r\n");
    }
    b.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (b,format!("multipart/mixed; boundary={boundary}"))
}

#[derive(serde::Serialize)]
pub struct Page {
    pub items:Vec<Value>,pub page:u32,pub page_size:u32,pub total:Option<u64>,pub next_page:Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rest() -> Rest {
        Rest { cfg:TableauConfig { server:"https://tableau.invalid".into(), site:"test".into(), api_version:"3.25".into(),
            auth:TableauAuth::Pat(crate::config::PatConfig { name:"test".into(), secret_env:"UNUSED_TEST_SECRET".into() }),
            ca_certificate:None, publish_projects:Vec::new() },
            client:Client::builder().timeout(Duration::from_secs(2)).no_proxy().build().unwrap(),
            token:Some(Zeroizing::new("test-session-not-a-real-secret".into())), site_id:"site".into(), user_id:"user".into(),
            max_json:8192, auth_timeout:Duration::from_secs(1) }
    }
    fn response(status: &str, body: &str) -> Response {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let reply = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len());
        let worker = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap(); s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new(); let mut b = [0u8;1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut b).unwrap(); assert!(n > 0); request.extend_from_slice(&b[..n]);
            }
            s.write_all(reply.as_bytes()).unwrap();
        });
        let r = Client::builder().timeout(Duration::from_secs(2)).no_proxy().build().unwrap()
            .get(format!("http://{address}/test")).send().unwrap();
        worker.join().unwrap(); r
    }
    #[test]
    fn unauthorized_invalidates_session_even_with_malformed_body() {
        let mut r = rest();
        let error = r.response_json(response("401 Unauthorized", "not JSON")).unwrap_err();
        assert_eq!(error.code,"AUTH_EXPIRED"); assert!(!r.authenticated());
        assert!(r.site_id.is_empty() && r.user_id.is_empty());
        assert_eq!(error.details.unwrap()["request_replayed"], false);
    }
    #[test]
    fn forbidden_does_not_silently_change_identity() {
        let mut r = rest();
        assert_eq!(r.response_json(response("403 Forbidden", r#"{"error":{"code":"403000"}}"#)).unwrap_err().code,"REST_REJECTED");
        assert!(r.authenticated()); assert_eq!(r.site_id,"site");
    }
    #[test]
    fn success_response_preserves_session_and_returns_data() {
        let mut r = rest();
        assert_eq!(r.response_json(response("200 OK", r#"{"workbooks":[]}"#)).unwrap(), json!({"workbooks":[]}));
        assert!(r.authenticated());
    }
    #[test]
    fn revoked_session_cannot_construct_authorized_requests() {
        let mut r = rest(); r.clear_session();
        assert!(r.request(Method::GET, Url::parse("https://tableau.invalid/api").unwrap()).is_err());
    }
}
