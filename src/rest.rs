//! Tableau REST 3.29 boundary for server-backed operations.
//!
//! This client is deliberately narrow: PAT sign-in, workbook discovery/download,
//! `validateWorkbook`, multipart publish, job lookup, and sign-out. Configuration
//! is pinned to REST 3.29 and HTTPS. Redirects are disabled.
//!
//! Tableau is the authority for XSD and semantic workbook validation. Local
//! package checks establish file safety/preservation; they are not a substitute
//! for `validateWorkbook`.

use crate::{
    Error, REST_API_VERSION, Result, TABLEAU_PRODUCT_TARGET,
    package::{self, Limits, Package},
    require,
};
use reqwest::{
    Method, Url,
    blocking::{Client, RequestBuilder, Response, multipart},
    header::{ACCEPT, CONTENT_TYPE, HeaderValue},
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// Tableau REST connection contract.
#[derive(Clone)]
pub struct Config {
    /// Absolute HTTPS Tableau Server base URL.
    pub server: String,
    /// Tableau site content URL used during sign-in.
    pub site: String,
    /// Must equal [`REST_API_VERSION`]; other versions are rejected.
    pub api_version: String,
    /// Personal access token name sent to Tableau.
    pub pat_name: String,
    /// Environment-variable name containing the PAT secret, never the secret itself.
    pub pat_secret_env: String,
    /// Whole-request timeout; connection establishment also has a fixed 15-second cap.
    pub timeout: Duration,
}

impl Config {
    /// Validates the static compatibility/security contract without making a request.
    pub fn validate(&self) -> Result<()> {
        let url = Url::parse(&self.server)
            .map_err(|_| Error::new("CONFIG", "Invalid Tableau server URL"))?;
        require(
            url.scheme() == "https" && url.host_str().is_some(),
            "CONFIG",
            "Tableau server must be an HTTPS URL",
        )?;
        require(
            self.api_version == REST_API_VERSION,
            "CONFIG",
            format!(
                "TapKit targets Tableau REST {REST_API_VERSION} (Server {TABLEAU_PRODUCT_TARGET})"
            ),
        )?;
        require(
            !self.pat_name.trim().is_empty(),
            "CONFIG",
            "PAT name cannot be empty",
        )?;
        require(
            !self.pat_secret_env.trim().is_empty(),
            "CONFIG",
            "PAT secret environment variable name cannot be empty",
        )
    }
}

/// Authenticated REST 3.29 session bound to one Tableau site.
pub struct Rest {
    config: Config,
    client: Client,
    token: String,
    site_id: String,
}

#[derive(Debug, Serialize)]
pub struct DownloadReceipt {
    pub bytes: u64,
    pub sha256: String,
    pub format: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkbookInfo {
    pub id: String,
    pub name: String,
    pub content_url: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub updated_at: Option<String>,
    pub has_extracts: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct Validation {
    pub valid: bool,
    pub response: Value,
}

impl Rest {
    /// Creates an HTTPS-only Tableau session from PAT configuration.
    ///
    /// The secret is read from `Config::pat_secret_env`; it is not stored in the
    /// configuration value or surfaced in TapKit errors. Redirects are disabled
    /// so credentials are never forwarded to a different origin.
    pub fn sign_in(config: Config) -> Result<Self> {
        config.validate()?;
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(config.timeout)
            .user_agent(concat!("tabkit/", env!("CARGO_PKG_VERSION")))
            .build()?;

        let secret = std::env::var(&config.pat_secret_env).map_err(|_| {
            Error::new(
                "AUTH_CONFIG",
                format!(
                    "PAT secret environment variable {} is not set",
                    config.pat_secret_env
                ),
            )
        })?;
        require(
            !secret.trim().is_empty(),
            "AUTH_CONFIG",
            "PAT secret is empty",
        )?;

        let url = api_url(&config, None, &["auth", "signin"])?;
        let response = client
            .post(url)
            .header(ACCEPT, "application/json")
            .json(&json!({
                "credentials": {
                    "personalAccessTokenName": config.pat_name,
                    "personalAccessTokenSecret": secret,
                    "site": { "contentUrl": config.site }
                }
            }))
            .send()?;
        let value = read_json(response, "Tableau sign-in")?;
        let credentials = value
            .get("credentials")
            .ok_or_else(|| Error::new("REST_SHAPE", "Sign-in response lacks credentials"))?;
        let token = credentials
            .get("token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("REST_SHAPE", "Sign-in response lacks token"))?
            .to_owned();
        let site_id = credentials
            .pointer("/site/id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("REST_SHAPE", "Sign-in response lacks site id"))?
            .to_owned();

        Ok(Self {
            config,
            client,
            token,
            site_id,
        })
    }

    /// Searches exact name first, then bounded case-insensitive substring fallback.
    ///
    /// `limit` must be 1..=100. If Tableau returns no exact match, at most ten
    /// 1,000-item pages are scanned; the method does not claim exhaustive search
    /// beyond that bound.
    pub fn search_workbooks(&self, query: &str, limit: usize) -> Result<Vec<WorkbookInfo>> {
        let query = query.trim();
        require(
            !query.is_empty(),
            "ARGUMENT",
            "Workbook query cannot be empty",
        )?;
        require(
            (1..=100).contains(&limit),
            "ARGUMENT",
            "Workbook search limit must be 1..100",
        )?;

        let mut exact_url = self.site_url(&["workbooks"])?;
        exact_url
            .query_pairs_mut()
            .append_pair("filter", &format!("name:eq:{query}"))
            .append_pair("pageSize", &limit.to_string())
            .append_pair(
                "fields",
                "id,name,contentUrl,updatedAt,hasExtracts,project.id,project.name",
            );
        let exact = workbook_infos(
            self.send_json("Workbook search", self.request(Method::GET, exact_url)?)?,
        )?;
        if !exact.is_empty() {
            return Ok(exact.into_iter().take(limit).collect());
        }

        let mut results = Vec::new();
        let needle = query.to_lowercase();
        let page_size = 1000usize;
        for page in 1..=10usize {
            let mut url = self.site_url(&["workbooks"])?;
            url.query_pairs_mut()
                .append_pair("pageSize", &page_size.to_string())
                .append_pair("pageNumber", &page.to_string())
                .append_pair(
                    "fields",
                    "id,name,contentUrl,updatedAt,hasExtracts,project.id,project.name",
                );
            let page_items = workbook_infos(
                self.send_json("Workbook search", self.request(Method::GET, url)?)?,
            )?;
            let page_len = page_items.len();
            for item in page_items {
                if item.name.to_lowercase().contains(&needle) {
                    results.push(item);
                    if results.len() == limit {
                        return Ok(results);
                    }
                }
            }
            if page_len < page_size {
                break;
            }
        }
        Ok(results)
    }

    /// Downloads to a unique file after detecting the actual TWB/TWBX payload type.
    ///
    /// Bytes are streamed under `Limits::file_bytes`, hashed while written, then
    /// admitted through [`Package::open`] before the path is returned.
    pub fn download_workbook_auto(
        &self,
        workbook_id: &str,
        include_extract: bool,
        directory: &Path,
        limits: &Limits,
    ) -> Result<(PathBuf, DownloadReceipt)> {
        require(
            directory.is_dir(),
            "PATH",
            "Download directory does not exist",
        )?;

        let mut url = self.site_url(&["workbooks", workbook_id, "content"])?;
        url.query_pairs_mut().append_pair(
            "includeExtract",
            if include_extract { "true" } else { "false" },
        );

        let response = self.request(Method::GET, url)?.send()?;
        let mut response = require_success_response(response, "Workbook download")?;

        let mut temp = tempfile::NamedTempFile::new_in(directory)?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut prefix = [0u8; 4];
        let mut prefix_len = 0usize;
        let mut buffer = [0u8; 64 * 1024];

        loop {
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| Error::new("LIMIT", "Download size overflow"))?;
            require(
                total <= limits.file_bytes,
                "LIMIT",
                "Downloaded workbook exceeds configured limit",
            )?;

            if prefix_len < prefix.len() {
                let take = count.min(prefix.len() - prefix_len);
                prefix[prefix_len..prefix_len + take].copy_from_slice(&buffer[..take]);
                prefix_len += take;
            }

            hash.update(&buffer[..count]);
            temp.write_all(&buffer[..count])?;
        }
        require(total > 0, "REST_SHAPE", "Downloaded workbook is empty")?;

        let format = if prefix_len == 4 && prefix == *b"PK\x03\x04" {
            "twbx"
        } else {
            "twb"
        };
        temp.as_file_mut().flush()?;
        temp.as_file().sync_all()?;

        let output = directory.join(format!(
            "{}-{}.{}",
            workbook_id,
            uuid::Uuid::new_v4().simple(),
            format
        ));
        temp.persist_noclobber(&output)
            .map_err(|error| Error::new("OUTPUT_EXISTS", error.error.to_string()))?;
        Package::open(&output, limits)?;

        Ok((
            output,
            DownloadReceipt {
                bytes: total,
                sha256: format!("{:x}", hash.finalize()),
                format: if format == "twbx" { "twbx" } else { "twb" },
            },
        ))
    }

    /// Downloads to an explicit non-existing path and verifies its extension matches the payload.
    ///
    /// The file becomes visible at `output` only after bounded streaming, hashing,
    /// package admission, and a no-clobber persist succeed.
    pub fn download_workbook(
        &self,
        workbook_id: &str,
        include_extract: bool,
        output: &Path,
        limits: &Limits,
    ) -> Result<DownloadReceipt> {
        require(
            !output.exists(),
            "OUTPUT_EXISTS",
            "Refusing to overwrite output",
        )?;
        let extension = output
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        require(
            matches!(extension, "twb" | "twbx"),
            "FILE_TYPE",
            "Download output must end in .twb or .twbx",
        )?;

        let mut url = self.site_url(&["workbooks", workbook_id, "content"])?;
        url.query_pairs_mut().append_pair(
            "includeExtract",
            if include_extract { "true" } else { "false" },
        );

        let response = self.request(Method::GET, url)?.send()?;
        let mut response = require_success_response(response, "Workbook download")?;

        let parent = output
            .parent()
            .ok_or_else(|| Error::new("PATH", "Download output has no parent"))?;
        let mut temp = tempfile::Builder::new()
            .suffix(if extension == "twbx" { ".twbx" } else { ".twb" })
            .tempfile_in(parent)?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut prefix = Vec::new();
        let mut buffer = [0u8; 64 * 1024];

        loop {
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| Error::new("LIMIT", "Download size overflow"))?;
            require(
                total <= limits.file_bytes,
                "LIMIT",
                "Downloaded workbook exceeds configured limit",
            )?;
            if prefix.len() < 8 {
                prefix.extend_from_slice(&buffer[..count.min(8 - prefix.len())]);
            }
            hash.update(&buffer[..count]);
            temp.write_all(&buffer[..count])?;
        }
        require(total > 0, "REST_SHAPE", "Downloaded workbook is empty")?;

        let returned = if prefix.starts_with(b"PK\x03\x04") {
            "twbx"
        } else {
            "twb"
        };
        require(
            returned == extension,
            "FILE_TYPE",
            format!("Tableau returned {returned}; output path uses .{extension}"),
        )?;

        temp.as_file_mut().flush()?;
        temp.as_file().sync_all()?;
        Package::open(temp.path(), limits)?;
        let sha256 = format!("{:x}", hash.finalize());
        temp.persist_noclobber(output)
            .map_err(|error| Error::new("OUTPUT_EXISTS", error.error.to_string()))?;

        Ok(DownloadReceipt {
            bytes: total,
            sha256,
            format: if returned == "twbx" { "twbx" } else { "twb" },
        })
    }

    /// Sends the admitted TWB XML to Tableau's authoritative validation endpoint.
    ///
    /// For TWBX input only the embedded TWB is submitted. HTTP 422 is represented
    /// as `Ok(Validation { valid: false, .. })`; transport/auth/server failures are
    /// errors. Packaged resources are outside this endpoint's validation scope.
    pub fn validate_workbook(&self, path: &Path, limits: &Limits) -> Result<Validation> {
        let (_package, twb) = Package::open(path, limits)?;
        let part = multipart::Part::bytes(twb)
            .file_name("candidate.twb")
            .mime_str("application/xml")
            .map_err(|_| Error::new("HTTP", "Failed to build validation multipart"))?;
        let form = multipart::Form::new().part("tableau_workbook", part);
        let response = self
            .request(
                Method::POST,
                self.site_url(&["workbooks", "validateWorkbook"])?,
            )?
            .multipart(form)
            .send()?;

        let status = response.status();
        let body = read_json_any_status(response)?;
        if status.is_success() {
            Ok(Validation {
                valid: true,
                response: body,
            })
        } else if status.as_u16() == 422 {
            Ok(Validation {
                valid: false,
                response: body,
            })
        } else {
            Err(tableau_rejection(
                "Workbook validation",
                status.as_u16(),
                &body,
            ))
        }
    }

    /// Starts an asynchronous publish from an admitted, stable package snapshot.
    ///
    /// Upload blocks are 8 MiB. The source is hashed before upload and again
    /// afterward; any concurrent change fails with `STALE_BASE` before commit.
    /// The returned JSON is Tableau's publish/job response; this method does not
    /// wait for job completion.
    pub fn publish_workbook(
        &self,
        path: &Path,
        name: &str,
        project_id: &str,
        overwrite: bool,
        limits: &Limits,
    ) -> Result<Value> {
        require(
            !name.trim().is_empty(),
            "ARGUMENT",
            "Publish workbook name cannot be empty",
        )?;
        let file_type = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        require(
            matches!(file_type, "twb" | "twbx"),
            "FILE_TYPE",
            "Publish supports TWB/TWBX",
        )?;
        Package::open(path, limits)?;
        let source_hash = package::hash_file(path, limits.file_bytes)?;

        let upload = self.send_json(
            "Start workbook upload",
            self.request(Method::POST, self.site_url(&["fileUploads"])?)?,
        )?;
        let session = upload
            .pointer("/fileUpload/uploadSessionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("REST_SHAPE", "Upload session id is missing"))?
            .to_owned();

        let mut input = File::open(path)?;
        let mut buffer = vec![0u8; 8 << 20];
        let mut uploaded_hash = Sha256::new();
        let mut total = 0u64;
        let mut sequence_id = 0u64;
        loop {
            let mut count = 0usize;
            while count < buffer.len() {
                let read = input.read(&mut buffer[count..])?;
                if read == 0 {
                    break;
                }
                count += read;
            }
            if count == 0 {
                break;
            }
            total += count as u64;
            require(
                total <= limits.file_bytes,
                "LIMIT",
                "Publish input exceeds configured limit",
            )?;
            uploaded_hash.update(&buffer[..count]);
            let (body, content_type) = mixed("", Some(("tableau_file", "file", &buffer[..count])));
            let mut append_url = self.site_url(&["fileUploads", &session])?;
            append_url
                .query_pairs_mut()
                .append_pair("sequenceID", &sequence_id.to_string());
            self.send_json(
                "Upload workbook block",
                self.request(Method::PUT, append_url)?
                    .header(CONTENT_TYPE, content_type)
                    .body(body),
            )?;
            sequence_id = sequence_id
                .checked_add(1)
                .ok_or_else(|| Error::new("LIMIT", "Upload sequence overflow"))?;
        }

        require(
            format!("{:x}", uploaded_hash.finalize()) == source_hash
                && package::hash_file(path, limits.file_bytes)? == source_hash,
            "STALE_BASE",
            "Workbook changed during upload",
        )?;

        let payload = format!(
            "<tsRequest><workbook name=\"{}\"><project id=\"{}\" /></workbook></tsRequest>",
            escape_attr(name),
            escape_attr(project_id)
        );
        let (body, content_type) = mixed(&payload, None);
        let mut url = self.site_url(&["workbooks"])?;
        url.query_pairs_mut()
            .append_pair("uploadSessionId", &session)
            .append_pair("workbookType", file_type)
            .append_pair("overwrite", if overwrite { "true" } else { "false" })
            .append_pair("asJob", "true");

        self.send_json(
            "Commit workbook publish",
            self.request(Method::POST, url)?
                .header(CONTENT_TYPE, content_type)
                .body(body),
        )
    }

    /// Reads one background-job snapshot; it does not poll or wait for a terminal state.
    pub fn job(&self, job_id: &str) -> Result<Value> {
        self.send_json(
            "Read Tableau background job",
            self.request(Method::GET, self.site_url(&["jobs", job_id])?)?,
        )
    }

    /// Consumes the client and asks Tableau to invalidate its session token.
    pub fn sign_out(self) -> Result<()> {
        let url = api_url(&self.config, None, &["auth", "signout"])?;
        let token = HeaderValue::from_str(&self.token)
            .map_err(|_| Error::new("AUTH", "Invalid Tableau session token"))?;
        let response = self
            .client
            .post(url)
            .header("X-Tableau-Auth", token)
            .send()?;
        require_success_response(response, "Tableau sign-out").map(|_| ())
    }

    fn site_url(&self, parts: &[&str]) -> Result<Url> {
        api_url(&self.config, Some(&self.site_id), parts)
    }

    fn request(&self, method: Method, url: Url) -> Result<RequestBuilder> {
        let mut token = HeaderValue::from_str(&self.token)
            .map_err(|_| Error::new("AUTH", "Invalid Tableau session token"))?;
        token.set_sensitive(true);
        Ok(self
            .client
            .request(method, url)
            .header("X-Tableau-Auth", token)
            .header(ACCEPT, "application/json"))
    }

    fn send_json(&self, action: &str, request: RequestBuilder) -> Result<Value> {
        read_json(request.send()?, action)
    }
}

fn workbook_infos(value: Value) -> Result<Vec<WorkbookInfo>> {
    let Some(workbooks) = value.pointer("/workbooks/workbook") else {
        return Ok(Vec::new());
    };

    let items: Vec<&Value> = match workbooks {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![workbooks],
        _ => {
            return Err(Error::new(
                "REST_SHAPE",
                "Tableau workbook list has an unexpected shape",
            ));
        }
    };

    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("REST_SHAPE", "Workbook result lacks id"))?;
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("REST_SHAPE", "Workbook result lacks name"))?;

        result.push(WorkbookInfo {
            id: id.to_owned(),
            name: name.to_owned(),
            content_url: item
                .get("contentUrl")
                .and_then(Value::as_str)
                .map(str::to_owned),
            project_id: item
                .pointer("/project/id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            project_name: item
                .pointer("/project/name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            updated_at: item
                .get("updatedAt")
                .and_then(Value::as_str)
                .map(str::to_owned),
            has_extracts: item.get("hasExtracts").and_then(|value| {
                value
                    .as_bool()
                    .or_else(|| value.as_str().and_then(|text| text.parse::<bool>().ok()))
            }),
        });
    }
    Ok(result)
}

fn api_url(config: &Config, site_id: Option<&str>, parts: &[&str]) -> Result<Url> {
    let mut url = Url::parse(&config.server)
        .map_err(|_| Error::new("CONFIG", "Invalid Tableau server URL"))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| Error::new("CONFIG", "Invalid Tableau server base URL"))?;
        path.pop_if_empty().push("api").push(&config.api_version);
        if let Some(site_id) = site_id {
            path.push("sites").push(site_id);
        }
        for part in parts {
            path.push(part);
        }
    }
    Ok(url)
}

fn require_success_response(mut response: Response, action: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = read_json_body(&mut response).unwrap_or_else(|_| json!({}));
    Err(tableau_rejection(action, status.as_u16(), &body))
}

fn tableau_rejection(action: &str, status: u16, body: &Value) -> Error {
    let code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(compact_tableau_text);
    let summary = body
        .pointer("/error/summary")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(compact_tableau_text);
    let detail = body
        .pointer("/error/detail")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(compact_tableau_text);

    let mut message = format!("{action} was rejected by Tableau (HTTP {status}");
    if let Some(code) = &code {
        message.push_str(", code ");
        message.push_str(code);
    }
    message.push(')');

    if let Some(summary) = &summary {
        message.push_str(": ");
        message.push_str(summary);
    }
    if let Some(detail) = &detail
        && summary.as_deref() != Some(detail.as_str())
    {
        message.push_str(": ");
        message.push_str(detail);
    }

    Error::new("REST_REJECTED", message)
}

fn compact_tableau_text(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_BYTES: usize = 512;
    if compact.len() <= MAX_BYTES {
        return compact;
    }

    let mut end = MAX_BYTES;
    while !compact.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &compact[..end])
}

fn read_json(mut response: Response, action: &str) -> Result<Value> {
    let status = response.status();
    if status.is_success() {
        return read_json_body(&mut response);
    }

    let body = read_json_body(&mut response).unwrap_or_else(|_| json!({}));
    Err(tableau_rejection(action, status.as_u16(), &body))
}

fn read_json_any_status(mut response: Response) -> Result<Value> {
    read_json_body(&mut response)
}

fn read_json_body(response: &mut Response) -> Result<Value> {
    let mut data = Vec::new();
    response.take(16 << 20).read_to_end(&mut data)?;
    if data.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&data)
        .map_err(|_| Error::new("REST_SHAPE", "Expected a JSON Tableau response"))
}

fn mixed(payload: &str, file: Option<(&str, &str, &[u8])>) -> (Vec<u8>, String) {
    let boundary = format!("tabkit-{}", uuid::Uuid::new_v4().simple());
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: name=\"request_payload\"\r\nContent-Type: text/xml\r\n\r\n{payload}\r\n"
    )
    .into_bytes();
    if let Some((part, name, data)) = file {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: name=\"{part}\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/mixed; boundary={boundary}"))
}

fn escape_attr(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
    output
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_query_workbooks_project_fields() -> Result<()> {
        let items = workbook_infos(json!({
            "workbooks": {
                "workbook": [{
                    "id": "wb-1",
                    "name": "Sales",
                    "contentUrl": "Sales",
                    "updatedAt": "2026-11-17T10:00:00Z",
                    "hasExtracts": "true",
                    "project": {
                        "id": "project-1",
                        "name": "Reporting"
                    }
                }]
            }
        }))?;

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "wb-1");
        assert_eq!(items[0].project_id.as_deref(), Some("project-1"));
        assert_eq!(items[0].project_name.as_deref(), Some("Reporting"));
        assert_eq!(items[0].has_extracts, Some(true));
        Ok(())
    }

    #[test]
    fn tableau_rejection_preserves_server_code_summary_and_detail() {
        let error = tableau_rejection(
            "Workbook download",
            404,
            &json!({
                "error": {
                    "code": "404002",
                    "summary": "Workbook not found",
                    "detail": "The requested workbook does not exist or is not visible."
                }
            }),
        );
        assert_eq!(error.code, "REST_REJECTED");
        assert_eq!(
            error.message,
            "Workbook download was rejected by Tableau (HTTP 404, code 404002): Workbook not found: The requested workbook does not exist or is not visible."
        );
    }

    #[test]
    fn tableau_rejection_compacts_multiline_detail() {
        let error = tableau_rejection(
            "Workbook publish",
            403,
            &json!({
                "error": {
                    "summary": "Permission denied",
                    "detail": "Project access\n  is not available"
                }
            }),
        );
        assert_eq!(
            error.message,
            "Workbook publish was rejected by Tableau (HTTP 403): Permission denied: Project access is not available"
        );
    }

    #[test]
    fn accepts_rest_3_29_and_rejects_3_30_for_2026_2_4_target() -> Result<()> {
        let config = |api_version: &str| Config {
            server: "https://tableau.example.test".into(),
            site: String::new(),
            api_version: api_version.into(),
            pat_name: "test-pat".into(),
            pat_secret_env: "TABLEAU_PAT_SECRET".into(),
            timeout: Duration::from_secs(30),
        };

        config("3.29").validate()?;
        let rejected = config("3.30")
            .validate()
            .expect_err("REST 3.30 must be rejected by the 2026.2.4 target gate");
        assert_eq!(rejected.code, "CONFIG");
        Ok(())
    }
}
