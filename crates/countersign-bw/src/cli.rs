use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::error::{BwError, Result};
use crate::version::{CliVersion, MINIMUM_VERSION};

/// Environment variables passed through to the CLI so private certificate
/// authorities and proxies keep working. Everything else is dropped,
/// including `LD_PRELOAD` and `NODE_OPTIONS`.
const PASSTHROUGH_ENV: &[&str] = &[
    "NODE_EXTRA_CA_CERTS",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// Output above this size is treated as a protocol failure.
const OUTPUT_LIMIT: u64 = 1 << 20;

/// One official CLI executable bound to one private profile folder.
#[derive(Debug, Clone)]
pub struct Cli {
    path: PathBuf,
    appdata: PathBuf,
    passthrough: Vec<(OsString, OsString)>,
}

/// Sign-in state reported by `bw status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthStatus {
    Unauthenticated,
    Locked,
    Unlocked,
}

/// The metadata-only result of `bw status`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    pub server_url: Option<String>,
    pub last_sync: Option<String>,
    pub user_email: Option<String>,
    pub user_id: Option<String>,
    pub status: AuthStatus,
}

pub(crate) struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Cli {
    pub fn new(path: impl Into<PathBuf>, appdata: impl Into<PathBuf>) -> Self {
        let passthrough = PASSTHROUGH_ENV
            .iter()
            .filter_map(|name| std::env::var_os(name).map(|v| (OsString::from(name), v)))
            .collect();
        Cli { path: path.into(), appdata: appdata.into(), passthrough }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn appdata(&self) -> &Path {
        &self.appdata
    }

    /// The complete environment the CLI runs with. Callers that launch the
    /// CLI themselves, such as an embedded sign-in terminal, use this too.
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut env = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("LANG"), OsString::from("C.UTF-8")),
            (OsString::from("BITWARDENCLI_APPDATA_DIR"), self.appdata.clone().into_os_string()),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            env.push((OsString::from("HOME"), home));
        }
        env.extend(self.passthrough.iter().cloned());
        env
    }

    /// A command for this CLI with a scrubbed environment and no stdin.
    pub(crate) fn command(&self) -> Command {
        let mut cmd = Command::new(&self.path);
        cmd.env_clear()
            .envs(self.environment())
            .env("BW_NOINTERACTION", "true")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }

    /// Reads the CLI version and checks it against Countersign's minimum.
    pub async fn version(&self) -> Result<CliVersion> {
        if !self.path.is_file() {
            return Err(BwError::CliMissing(self.path.clone()));
        }
        let out = self.run(self.args(["--version"]), Duration::from_secs(30)).await?;
        let text = String::from_utf8_lossy(&out.stdout);
        let version = CliVersion::parse(&text).ok_or_else(|| BwError::Unsupported {
            found: text.trim().chars().take(40).collect(),
            minimum: MINIMUM_VERSION,
        })?;
        if !version.is_supported() {
            return Err(BwError::Unsupported { found: version.to_string(), minimum: MINIMUM_VERSION });
        }
        Ok(version)
    }

    /// Reads the sign-in state. The CLI prints it within about half a
    /// second, then idles for about two more before it exits, so this returns
    /// as soon as the answer is complete. `bw status` only reads the profile.
    pub async fn status(&self) -> Result<CliStatus> {
        self.answer(self.args(["status"]), Duration::from_secs(30), "status").await
    }

    /// Points the profile at a Bitwarden server. `None` restores the default
    /// US cloud. Only allowed while logged out.
    pub async fn config_server(&self, url: Option<&str>) -> Result<()> {
        let url = url.unwrap_or("https://vault.bitwarden.com");
        self.checked(self.args(["config", "server", url]), Duration::from_secs(30)).await
    }

    /// Logs in with a personal API key. The key travels in the child's
    /// environment, never its arguments.
    pub async fn login_api_key(&self, client_id: &SecretString, client_secret: &SecretString) -> Result<()> {
        let mut cmd = self.args(["login", "--apikey"]);
        cmd.env("BW_CLIENTID", client_id.expose_secret())
            .env("BW_CLIENTSECRET", client_secret.expose_secret());
        self.checked(cmd, Duration::from_secs(120)).await
    }

    pub async fn logout(&self) -> Result<()> {
        match self.checked(self.args(["logout"]), Duration::from_secs(60)).await {
            Err(BwError::NotLoggedIn) => Ok(()),
            other => other,
        }
    }

    /// Creates a file Send, which `bw serve` cannot. The request, which may
    /// hold the Send's password, goes to the CLI on stdin, never in its
    /// arguments; `send["file"]["fileName"]` names the file to upload.
    pub async fn send_create_file(
        &self,
        session: &crate::SessionKey,
        send: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        use base64::Engine;
        use tokio::io::AsyncWriteExt;

        let json =
            zeroize::Zeroizing::new(serde_json::to_vec(send).map_err(|e| BwError::Protocol(e.to_string()))?);
        let encoded = zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(&*json));
        let mut cmd = self.args(["send", "create"]);
        cmd.env("BW_SESSION", session.expose())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(BwError::Io)?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let work = async {
            stdin.write_all(encoded.as_bytes()).await?;
            drop(stdin);
            let (out, err, status) = tokio::join!(read_bounded(stdout), read_bounded(stderr), child.wait());
            Ok::<_, BwError>((status?, out?, err?))
        };
        let (status, out, err) = match tokio::time::timeout(Duration::from_secs(600), work).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = child.start_kill();
                return Err(BwError::Timeout);
            }
        };
        if !status.success() {
            let text = if err.is_empty() { &out } else { &err };
            return Err(BwError::from_cli_message(&String::from_utf8_lossy(text)));
        }
        serde_json::from_slice(&out).map_err(|e| BwError::Protocol(format!("send create: {e}")))
    }

    /// Lists Sends. `bw serve` 2026.10.1 answers `/send/list` from a cache it
    /// never refreshes, so creates, edits, deletes and syncs never show
    /// there; this one-off command reads the stored vault each time.
    pub async fn send_list(&self, session: &crate::SessionKey) -> Result<Vec<serde_json::Value>> {
        let mut cmd = self.args(["send", "list"]);
        cmd.env("BW_SESSION", session.expose());
        let out = self.run(cmd, Duration::from_secs(60)).await?;
        let stdout = zeroize::Zeroizing::new(out.stdout);
        if !out.status.success() {
            let text = if out.stderr.is_empty() { &*stdout } else { &out.stderr };
            return Err(BwError::from_cli_message(&String::from_utf8_lossy(text)));
        }
        serde_json::from_slice(&stdout).map_err(|e| BwError::Protocol(format!("send list: {e}")))
    }

    pub async fn lock(&self) -> Result<()> {
        self.checked(self.args(["lock"]), Duration::from_secs(30)).await
    }

    fn args<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.command();
        cmd.args(args);
        cmd
    }

    /// Runs a command that prints a message on success and an error on stderr
    /// or stdout on failure.
    async fn checked(&self, cmd: Command, timeout: Duration) -> Result<()> {
        let out = self.run(cmd, timeout).await?;
        if out.status.success() {
            return Ok(());
        }
        let text = if out.stderr.is_empty() { &out.stdout } else { &out.stderr };
        Err(BwError::from_cli_message(&String::from_utf8_lossy(text)))
    }

    pub(crate) async fn run(&self, cmd: Command, timeout: Duration) -> Result<Output> {
        let mut child = self.spawn(cmd)?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let work = async {
            let (out, err, status) = tokio::join!(read_bounded(stdout), read_bounded(stderr), child.wait());
            Ok::<_, BwError>(Output { status: status?, stdout: out?, stderr: err? })
        };
        match tokio::time::timeout(timeout, work).await {
            Ok(result) => result,
            Err(_) => {
                let _ = child.start_kill();
                Err(BwError::Timeout)
            }
        }
    }

    /// Runs a command that only reads the profile and prints one JSON
    /// document, and returns the document as soon as it is complete. The CLI
    /// is left to exit on its own and reaped in the background; it is killed
    /// only if it outlives `timeout`. Without a complete document, the exit
    /// decides, as with [`Cli::run`].
    async fn answer<T: DeserializeOwned>(&self, cmd: Command, timeout: Duration, what: &str) -> Result<T> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut child = self.spawn(cmd)?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let stderr = tokio::spawn(read_bounded(child.stderr.take().expect("piped stderr")));
        let read = async {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = stdout.read(&mut chunk).await?;
                if n == 0 {
                    return Ok::<_, BwError>((buf, None));
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() as u64 > OUTPUT_LIMIT {
                    return Err(BwError::Protocol("the CLI produced too much output".into()));
                }
                if let Ok(value) = serde_json::from_slice::<T>(&buf) {
                    return Ok((buf, Some(value)));
                }
            }
        };
        let Ok(read) = tokio::time::timeout_at(deadline, read).await else {
            let _ = child.start_kill();
            return Err(BwError::Timeout);
        };
        let (out, value) = read?;
        if let Some(value) = value {
            tokio::spawn(async move {
                // Dropping the child at the deadline kills it.
                let _ = tokio::time::timeout_at(deadline, async move {
                    let _ = read_bounded(stdout).await;
                    let _ = child.wait().await;
                })
                .await;
            });
            return Ok(value);
        }
        let Ok(status) = tokio::time::timeout_at(deadline, child.wait()).await else {
            let _ = child.start_kill();
            return Err(BwError::Timeout);
        };
        let status = status?;
        let err = stderr.await.map_err(std::io::Error::other)??;
        if !status.success() {
            let text = if err.is_empty() { &out } else { &err };
            return Err(BwError::from_cli_message(&String::from_utf8_lossy(text)));
        }
        serde_json::from_slice(&out).map_err(|e| BwError::Protocol(format!("{what}: {e}")))
    }

    fn spawn(&self, mut cmd: Command) -> Result<tokio::process::Child> {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => BwError::CliMissing(self.path.clone()),
            _ => BwError::Io(e),
        })
    }
}

async fn read_bounded(stream: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    stream.take(OUTPUT_LIMIT + 1).read_to_end(&mut buf).await?;
    if buf.len() as u64 > OUTPUT_LIMIT {
        return Err(BwError::Protocol("the CLI produced too much output".into()));
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parses_cli_json() {
        let s: CliStatus = serde_json::from_str(
            r#"{"serverUrl":"https://vault.bitwarden.eu","lastSync":null,"userEmail":"a@example.test","userId":"x","status":"locked"}"#,
        )
        .unwrap();
        assert_eq!(s.status, AuthStatus::Locked);
        assert_eq!(s.server_url.as_deref(), Some("https://vault.bitwarden.eu"));
        let s: CliStatus =
            serde_json::from_str(r#"{"serverUrl":null,"lastSync":null,"status":"unauthenticated"}"#).unwrap();
        assert_eq!(s.status, AuthStatus::Unauthenticated);
    }

    #[test]
    fn environment_is_scrubbed() {
        let cli = Cli::new("/nonexistent/bw", "/tmp/profile");
        let env = cli.environment();
        assert!(env.iter().any(|(k, v)| k == "BITWARDENCLI_APPDATA_DIR" && v == "/tmp/profile"));
        assert!(!env.iter().any(|(k, _)| k == "LD_PRELOAD" || k == "NODE_OPTIONS"));
    }

    /// Writes an executable shell script standing in for the CLI.
    fn fake_cli(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    // One test, so no other test forks while a script is still open for
    // writing (which fails its exec with "text file busy").
    #[tokio::test]
    async fn status_returns_once_the_answer_is_complete() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");

        // The answer arrives in two writes, then the CLI idles, as the
        // official one does after answering.
        let idles = fake_cli(
            dir.path(),
            "idles",
            "printf '{\"serverUrl\":null,'\nsleep 0.2\nprintf '\"lastSync\":null,\"status\":\"locked\"}'\nexec sleep 5\n",
        );
        let started = std::time::Instant::now();
        let status = Cli::new(&idles, &profile).status().await.unwrap();
        assert_eq!(status.status, AuthStatus::Locked);
        assert!(started.elapsed() < Duration::from_secs(3), "waited for the CLI to exit");

        let fails = fake_cli(dir.path(), "fails", "echo 'You are not logged in.' >&2\nexit 1\n");
        assert!(matches!(Cli::new(&fails, &profile).status().await, Err(BwError::NotLoggedIn)));

        let garbled = fake_cli(dir.path(), "garbled", "printf '{\"status\":'\n");
        assert!(matches!(Cli::new(&garbled, &profile).status().await, Err(BwError::Protocol(_))));
    }

    #[tokio::test]
    async fn missing_cli_is_reported() {
        let cli = Cli::new("/nonexistent/bw", "/tmp/profile");
        assert!(matches!(cli.version().await, Err(BwError::CliMissing(_))));
    }
}
