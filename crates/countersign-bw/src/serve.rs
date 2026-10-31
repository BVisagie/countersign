use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::client::conn::http1;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zeroize::{Zeroize, Zeroizing};

use crate::cli::{Cli, CliStatus};
use crate::error::{BwError, Result};
use crate::is_object_id;
use crate::session::SessionKey;

/// `bw serve` checks the Host header against its configured hostname and
/// port, even on a socket. With no `--port`, that is localhost:8087.
const SERVE_HOST: &str = "localhost:8087";
const JSON_LIMIT: usize = 128 << 20;
const FILE_LIMIT: usize = 600 << 20;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const LONG_TIMEOUT: Duration = Duration::from_secs(300);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const DIAGNOSTIC_LIMIT: usize = 8 << 10;

/// A running `bw serve` child connected to Countersign through a private
/// socket pair. No TCP port or filesystem socket exists, so no other process
/// can send it requests.
pub struct Serve {
    child: Child,
    client: ServeClient,
    connection: JoinHandle<()>,
    diagnostics: Arc<std::sync::Mutex<Vec<u8>>>,
}

/// Filters for `list items`.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    pub search: Option<String>,
    pub folder_id: Option<String>,
    pub collection_id: Option<String>,
    pub organization_id: Option<String>,
    pub url: Option<String>,
    pub trash: bool,
}

/// Options for the official password and passphrase generator.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GenerateOptions {
    pub passphrase: bool,
    pub length: Option<u32>,
    pub uppercase: bool,
    pub lowercase: bool,
    pub number: bool,
    pub special: bool,
    pub min_number: Option<u32>,
    pub min_special: Option<u32>,
    pub avoid_ambiguous: bool,
    pub words: Option<u32>,
    pub separator: Option<String>,
    pub capitalize: bool,
    pub include_number: bool,
}

impl Serve {
    /// Starts `bw serve` and waits until it answers. With a session key the
    /// server starts unlocked; this restores a session after the child exits.
    pub async fn start(cli: &Cli, session: Option<&SessionKey>) -> Result<Serve> {
        let mut serve = Serve::spawn(cli, session).await?;
        match serve.client.call(Method::GET, "/status", None, START_TIMEOUT).await {
            Ok(_) => Ok(serve),
            Err(BwError::Disconnected | BwError::Timeout) => {
                let _ = tokio::time::timeout(Duration::from_secs(5), serve.child.wait()).await;
                let text = serve.diagnostic_text();
                serve.shutdown().await;
                Err(match text.as_str() {
                    "" => BwError::Disconnected,
                    t if t.contains("You are not logged in.") => BwError::NotLoggedIn,
                    t => BwError::Cli(t.chars().take(300).collect()),
                })
            }
            Err(e) => {
                serve.shutdown().await;
                Err(e)
            }
        }
    }

    /// Starts `bw serve` without waiting for it to answer, so the caller can
    /// keep the child where it can be stopped while it starts. Check
    /// readiness with [`Serve::ready`].
    pub async fn spawn(cli: &Cli, session: Option<&SessionKey>) -> Result<Serve> {
        let (ours, theirs) = StdUnixStream::pair()?;
        let fd = theirs.as_raw_fd();
        let mut cmd = cli.command();
        cmd.arg("serve")
            .arg("--hostname")
            .arg(format!("fd+connected://{fd}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(session) = session {
            cmd.env("BW_SESSION", session.expose());
        }
        // SAFETY: the closure runs between fork and exec and only calls
        // fcntl, which is async-signal-safe. It clears close-on-exec on the
        // child's end of the socket pair so the CLI inherits it.
        unsafe {
            cmd.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => BwError::CliMissing(cli.path().to_owned()),
            _ => BwError::Io(e),
        })?;
        drop(theirs);

        let diagnostics = Arc::new(std::sync::Mutex::new(Vec::new()));
        for stream in [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ]
        .into_iter()
        .flatten()
        {
            tokio::spawn(collect_diagnostics(stream, diagnostics.clone()));
        }

        ours.set_nonblocking(true)?;
        let stream = tokio::net::UnixStream::from_std(ours)?;
        let (sender, connection) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| BwError::Protocol(format!("connection setup: {e}")))?;
        let connection = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::debug!("bw serve connection closed: {e}");
            }
        });
        Ok(Serve {
            child,
            client: ServeClient { sender: Arc::new(Mutex::new(sender)) },
            connection,
            diagnostics,
        })
    }

    /// Waits until a spawned child answers.
    pub async fn ready(client: &ServeClient) -> Result<()> {
        client.call(Method::GET, "/status", None, START_TIMEOUT).await.map(drop)
    }

    pub fn client(&self) -> &ServeClient {
        &self.client
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Returns true once the child has exited.
    pub fn has_exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits for the child to exit on its own.
    pub async fn wait(&mut self) {
        let _ = self.child.wait().await;
    }

    /// Closes the socket, which ends `bw serve`, and kills the child if it
    /// has not exited within three seconds.
    pub async fn shutdown(mut self) {
        self.connection.abort();
        if tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
        }
    }

    fn diagnostic_text(&self) -> String {
        let buf = self.diagnostics.lock().map(|b| b.clone()).unwrap_or_default();
        String::from_utf8_lossy(&buf).trim().to_owned()
    }
}

async fn collect_diagnostics(
    mut stream: impl tokio::io::AsyncRead + Unpin,
    sink: Arc<std::sync::Mutex<Vec<u8>>>,
) {
    let mut chunk = [0u8; 1024];
    while let Ok(n) = stream.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        if let Ok(mut buf) = sink.lock() {
            let room = DIAGNOSTIC_LIMIT.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..n.min(room)]);
        }
    }
}

/// A handle for sending requests to one `bw serve` child. Requests are
/// serialized: the private connection carries one exchange at a time.
#[derive(Clone)]
pub struct ServeClient {
    sender: Arc<Mutex<http1::SendRequest<Full<Bytes>>>>,
}

#[derive(Deserialize)]
struct Envelope {
    success: bool,
    message: Option<String>,
    data: Option<Value>,
}

impl ServeClient {
    pub async fn status(&self) -> Result<CliStatus> {
        let data = self.call(Method::GET, "/status", None, DEFAULT_TIMEOUT).await?;
        serde_json::from_value(take(data, "template")).map_err(|e| BwError::Protocol(format!("status: {e}")))
    }

    /// Unlocks the vault and returns the new session key. Unlocking again
    /// while unlocked re-checks the password and rotates the session key.
    pub async fn unlock(&self, password: &SecretString) -> Result<SessionKey> {
        let secret = password.expose_secret();
        let mut body = Zeroizing::new(Vec::with_capacity(secret.len() * 6 + 16));
        serde_json::to_writer(&mut *body, &serde_json::json!({ "password": secret }))
            .map_err(|e| BwError::Protocol(e.to_string()))?;
        let body = Bytes::from_owner(body);
        let data = self.call(Method::POST, "/unlock", Some(body), LONG_TIMEOUT).await?;
        let raw = match take(data, "raw") {
            Value::String(raw) => raw,
            _ => return Err(BwError::Protocol("unlock returned no session".into())),
        };
        SessionKey::from_raw(raw)
            .ok_or_else(|| BwError::Protocol("unlock returned a malformed session".into()))
    }

    pub async fn lock(&self) -> Result<()> {
        self.call(Method::POST, "/lock", None, DEFAULT_TIMEOUT).await.map(drop)
    }

    pub async fn sync(&self) -> Result<()> {
        self.call(Method::POST, "/sync", None, LONG_TIMEOUT).await.map(drop)
    }

    pub async fn list_items(&self, options: &ListOptions) -> Result<Vec<Value>> {
        let mut query = Query::default();
        query.opt("search", options.search.as_deref());
        query.opt("folderid", options.folder_id.as_deref());
        query.opt("collectionid", options.collection_id.as_deref());
        query.opt("organizationid", options.organization_id.as_deref());
        query.opt("url", options.url.as_deref());
        if options.trash {
            query.flag("trash");
        }
        self.list(&format!("/list/object/items{}", query.finish())).await
    }

    pub async fn list_folders(&self) -> Result<Vec<Value>> {
        self.list("/list/object/folders").await
    }

    pub async fn list_collections(&self) -> Result<Vec<Value>> {
        self.list("/list/object/collections").await
    }

    pub async fn list_organizations(&self) -> Result<Vec<Value>> {
        self.list("/list/object/organizations").await
    }

    pub async fn list_sends(&self) -> Result<Vec<Value>> {
        self.list("/send/list").await
    }

    pub async fn get_item(&self, id: &str) -> Result<Value> {
        self.call(Method::GET, &format!("/object/item/{}", object_id(id)?), None, DEFAULT_TIMEOUT).await
    }

    /// The current one-time code. Bitwarden requires Premium for this.
    pub async fn get_totp(&self, id: &str) -> Result<SecretString> {
        let data = self
            .call(Method::GET, &format!("/object/totp/{}", object_id(id)?), None, DEFAULT_TIMEOUT)
            .await?;
        string_data(data)
    }

    pub async fn generate(&self, options: &GenerateOptions) -> Result<SecretString> {
        let mut query = Query::default();
        if options.passphrase {
            query.flag("passphrase");
            query.num("words", options.words);
            query.opt("separator", options.separator.as_deref());
            if options.capitalize {
                query.flag("capitalize");
            }
            if options.include_number {
                query.flag("includeNumber");
            }
        } else {
            query.num("length", options.length);
            for (on, name) in [
                (options.uppercase, "uppercase"),
                (options.lowercase, "lowercase"),
                (options.number, "number"),
                (options.special, "special"),
                (options.avoid_ambiguous, "ambiguous"),
            ] {
                if on {
                    query.flag(name);
                }
            }
            query.num("minNumber", options.min_number);
            query.num("minSpecial", options.min_special);
        }
        let data =
            self.call(Method::GET, &format!("/generate{}", query.finish()), None, DEFAULT_TIMEOUT).await?;
        string_data(data)
    }

    pub async fn create_item(&self, item: &Value) -> Result<Value> {
        self.call(Method::POST, "/object/item", Some(json_body(item)?), LONG_TIMEOUT).await
    }

    /// Replaces an item. Send the complete object read from `get_item`, with
    /// changes applied, so fields Countersign does not edit are preserved.
    pub async fn edit_item(&self, id: &str, item: &Value) -> Result<Value> {
        let path = format!("/object/item/{}", object_id(id)?);
        self.call(Method::PUT, &path, Some(json_body(item)?), LONG_TIMEOUT).await
    }

    /// Moves an item to the trash, or deletes it for good.
    pub async fn delete_item(&self, id: &str, permanent: bool) -> Result<()> {
        let suffix = if permanent { "?permanent=true" } else { "" };
        let path = format!("/object/item/{}{suffix}", object_id(id)?);
        self.call(Method::DELETE, &path, None, LONG_TIMEOUT).await.map(drop)
    }

    pub async fn restore_item(&self, id: &str) -> Result<()> {
        let path = format!("/restore/item/{}", object_id(id)?);
        self.call(Method::POST, &path, None, LONG_TIMEOUT).await.map(drop)
    }

    pub async fn create_folder(&self, name: &str) -> Result<Value> {
        let body = json_body(&serde_json::json!({ "name": name }))?;
        self.call(Method::POST, "/object/folder", Some(body), LONG_TIMEOUT).await
    }

    pub async fn edit_folder(&self, id: &str, name: &str) -> Result<Value> {
        let body = json_body(&serde_json::json!({ "name": name }))?;
        let path = format!("/object/folder/{}", object_id(id)?);
        self.call(Method::PUT, &path, Some(body), LONG_TIMEOUT).await
    }

    pub async fn delete_folder(&self, id: &str) -> Result<()> {
        let path = format!("/object/folder/{}", object_id(id)?);
        self.call(Method::DELETE, &path, None, LONG_TIMEOUT).await.map(drop)
    }

    /// Moves a personal item into an organization and its collections.
    pub async fn move_item(
        &self,
        id: &str,
        organization_id: &str,
        collection_ids: &[String],
    ) -> Result<Value> {
        if !collection_ids.iter().all(|c| is_object_id(c)) {
            return Err(BwError::Protocol("invalid collection id".into()));
        }
        let path = format!("/move/{}/{}", object_id(id)?, object_id(organization_id)?);
        self.call(Method::POST, &path, Some(json_body(&serde_json::json!(collection_ids))?), LONG_TIMEOUT)
            .await
    }

    /// Downloads and decrypts an attachment.
    pub async fn get_attachment(&self, item_id: &str, attachment_id: &str) -> Result<Zeroizing<Vec<u8>>> {
        if attachment_id.is_empty() || !attachment_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(BwError::Protocol("invalid attachment id".into()));
        }
        let path = format!("/object/attachment/{attachment_id}?itemid={}", object_id(item_id)?);
        let (status, bytes) =
            self.exchange(Method::GET, &path, Full::default(), None, LONG_TIMEOUT, FILE_LIMIT).await?;
        if status == StatusCode::OK {
            return Ok(Zeroizing::new(bytes.to_vec()));
        }
        parse_envelope(status, bytes).map(drop)?;
        Err(BwError::Protocol("attachment download failed".into()))
    }

    pub async fn create_attachment(&self, item_id: &str, file_name: &str, data: &[u8]) -> Result<Value> {
        let boundary = format!("countersign-{:016x}", random_u64());
        let name: String = file_name.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect();
        let mut body = Vec::with_capacity(data.len() + 256);
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let path = format!("/attachment?itemid={}", object_id(item_id)?);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let (status, bytes) = self
            .exchange(
                Method::POST,
                &path,
                Full::new(Bytes::from_owner(Zeroizing::new(body))),
                Some(&content_type),
                LONG_TIMEOUT,
                JSON_LIMIT,
            )
            .await?;
        parse_envelope(status, bytes)
    }

    pub async fn delete_attachment(&self, item_id: &str, attachment_id: &str) -> Result<()> {
        if attachment_id.is_empty() || !attachment_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(BwError::Protocol("invalid attachment id".into()));
        }
        let path = format!("/object/attachment/{attachment_id}?itemid={}", object_id(item_id)?);
        self.call(Method::DELETE, &path, None, LONG_TIMEOUT).await.map(drop)
    }

    pub async fn get_send(&self, id: &str) -> Result<Value> {
        self.call(Method::GET, &format!("/object/send/{}", object_id(id)?), None, DEFAULT_TIMEOUT).await
    }

    /// Creates a text Send. File Sends are not supported by `bw serve`.
    pub async fn create_send(&self, send: &Value) -> Result<Value> {
        self.call(Method::POST, "/object/send", Some(json_body(send)?), LONG_TIMEOUT).await
    }

    pub async fn edit_send(&self, id: &str, send: &Value) -> Result<Value> {
        let path = format!("/object/send/{}", object_id(id)?);
        self.call(Method::PUT, &path, Some(json_body(send)?), LONG_TIMEOUT).await
    }

    pub async fn delete_send(&self, id: &str) -> Result<()> {
        let path = format!("/object/send/{}", object_id(id)?);
        self.call(Method::DELETE, &path, None, LONG_TIMEOUT).await.map(drop)
    }

    pub async fn remove_send_password(&self, id: &str) -> Result<Value> {
        let path = format!("/send/{}/remove-password", object_id(id)?);
        self.call(Method::POST, &path, None, LONG_TIMEOUT).await
    }

    async fn list(&self, path: &str) -> Result<Vec<Value>> {
        match take(self.call(Method::GET, path, None, LONG_TIMEOUT).await?, "data") {
            Value::Array(items) => Ok(items),
            _ => Err(BwError::Protocol(format!("{path} returned no list"))),
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Bytes>,
        timeout: Duration,
    ) -> Result<Value> {
        let content_type = body.is_some().then_some("application/json");
        let (status, bytes) = self
            .exchange(method, path, Full::new(body.unwrap_or_default()), content_type, timeout, JSON_LIMIT)
            .await?;
        parse_envelope(status, bytes)
    }

    async fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Full<Bytes>,
        content_type: Option<&str>,
        timeout: Duration,
        limit: usize,
    ) -> Result<(StatusCode, Bytes)> {
        let work = async {
            let mut sender = self.sender.lock().await;
            sender.ready().await.map_err(|_| BwError::Disconnected)?;
            let mut request = Request::builder().method(method).uri(path).header(HOST, SERVE_HOST);
            if let Some(content_type) = content_type {
                request = request.header(CONTENT_TYPE, content_type);
            }
            let request = request.body(body).map_err(|e| BwError::Protocol(e.to_string()))?;
            let response = sender.send_request(request).await.map_err(|_| BwError::Disconnected)?;
            let status = response.status();
            let bytes = Limited::new(response.into_body(), limit)
                .collect()
                .await
                .map_err(|e| BwError::Protocol(format!("response body: {e}")))?
                .to_bytes();
            Ok((status, bytes))
        };
        tokio::time::timeout(timeout, work).await.map_err(|_| BwError::Timeout)?
    }
}

fn parse_envelope(status: StatusCode, bytes: Bytes) -> Result<Value> {
    if status == StatusCode::FORBIDDEN {
        return Err(BwError::Protocol("the CLI refused the request".into()));
    }
    let parsed = serde_json::from_slice::<Envelope>(&bytes);
    // Responses can hold decrypted vault data; clear the buffer when it is
    // not shared.
    if let Ok(mut owned) = bytes.try_into_mut() {
        owned.zeroize();
    }
    let envelope = parsed.map_err(|e| BwError::Protocol(format!("HTTP {status}: {e}")))?;
    if !envelope.success {
        let message = envelope.message.unwrap_or_else(|| "The Bitwarden CLI reported an error.".into());
        return Err(BwError::from_cli_message(&message));
    }
    Ok(envelope.data.unwrap_or(Value::Null))
}

fn take(mut value: Value, key: &str) -> Value {
    value.get_mut(key).map(Value::take).unwrap_or(Value::Null)
}

fn string_data(data: Value) -> Result<SecretString> {
    match take(data, "data") {
        Value::String(s) => Ok(SecretString::from(s)),
        _ => Err(BwError::Protocol("expected a string".into())),
    }
}

fn json_body(value: &Value) -> Result<Bytes> {
    let mut buf = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *buf, value).map_err(|e| BwError::Protocol(e.to_string()))?;
    Ok(Bytes::from_owner(buf))
}

fn object_id(id: &str) -> Result<&str> {
    if is_object_id(id) { Ok(id) } else { Err(BwError::Protocol("invalid object id".into())) }
}

fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    // SAFETY: getrandom writes at most buf.len() bytes into the buffer.
    let n = unsafe { libc::getrandom(buf.as_mut_ptr().cast(), buf.len(), 0) };
    if n != 8 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        return t.as_nanos() as u64 ^ u64::from(std::process::id());
    }
    u64::from_ne_bytes(buf)
}

/// A query string builder that percent-encodes values.
#[derive(Default)]
struct Query(String);

impl Query {
    fn push(&mut self, name: &str) {
        self.0.push(if self.0.is_empty() { '?' } else { '&' });
        self.0.push_str(name);
    }

    fn flag(&mut self, name: &str) {
        self.push(name);
    }

    fn opt(&mut self, name: &str, value: Option<&str>) {
        if let Some(value) = value {
            self.push(name);
            self.0.push('=');
            for b in value.bytes() {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                    self.0.push(b as char);
                } else {
                    self.0.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }

    fn num(&mut self, name: &str, value: Option<u32>) {
        if let Some(value) = value {
            self.opt(name, Some(&value.to_string()));
        }
    }

    fn finish(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encodes_values() {
        let mut q = Query::default();
        q.opt("search", Some("git hub&x=1/é"));
        q.flag("trash");
        q.num("length", Some(24));
        assert_eq!(q.finish(), "?search=git%20hub%26x%3D1%2F%C3%A9&trash&length=24");
    }

    #[test]
    fn envelope_errors_are_typed() {
        let locked = Bytes::from_static(br#"{"success":false,"message":"Vault is locked."}"#);
        assert!(matches!(parse_envelope(StatusCode::BAD_REQUEST, locked), Err(BwError::Locked)));
        let ok = Bytes::from_static(br#"{"success":true,"data":{"object":"string","data":"x"}}"#);
        let data = parse_envelope(StatusCode::OK, ok).unwrap();
        assert_eq!(string_data(data).unwrap().expose_secret(), "x");
        assert!(matches!(parse_envelope(StatusCode::FORBIDDEN, Bytes::new()), Err(BwError::Protocol(_))));
    }

    #[test]
    fn rejects_path_injection() {
        assert!(object_id("../../unlock").is_err());
    }
}
