//! MCP servers end to end: a real daemon, a client engine, the fake agent, and for HTTP
//! servers a fake MCP server that also plays its own OAuth authorization server.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use daemon::{AgentLauncher, Config, ConfigHome, Daemon};
use engine::{Connection, Engine};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use protocol::{
    Command, CommandOutput, DaemonState, MachineId, McpAuth, McpServer, McpTransport,
    PermissionMode, SessionEventKind, SessionRef, SessionStatus, Topic,
};
use sha2::{Digest, Sha256};

const TIMEOUT: Duration = Duration::from_secs(20);

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    daemon: Daemon,
    engine: Engine,
    machine: MachineId,
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

fn empty_home(root: &Path) -> ConfigHome {
    ConfigHome {
        home: Some(root.join("home")),
        codex_home: None,
        xdg_config: None,
        opencode_config: None,
        opencode_content: None,
        claude_code: None,
    }
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let config = Config {
            agents: BTreeMap::from([(
                "fake".to_owned(),
                AgentLauncher::Command {
                    program: env!("CARGO_BIN_EXE_fake-agent").into(),
                    args: vec![],
                },
            )]),
            keep_awake: None,
            mcp_home: empty_home(&root),
            ..Config::new(root.join("data"), 0)
        };
        let daemon = Daemon::start(config).await.unwrap();
        let engine = Engine::new("mcp-e2e");
        engine.add_daemon(&format!("ws://{}", daemon.addr()));
        engine.wait_until_settled().await;
        assert_eq!(engine.devices()[0].connection, Connection::Online);
        let machine = daemon.machine_id().clone();
        Self {
            _dir: dir,
            root,
            repo,
            daemon,
            engine,
            machine,
        }
    }

    async fn run(&self, command: Command) -> Result<CommandOutput, String> {
        self.engine
            .request(&self.machine, command)
            .await
            .map_err(|error| error.to_string())
    }

    fn state(&self) -> DaemonState {
        self.engine.devices()[0].state.clone().unwrap()
    }

    fn server(&self, name: &str) -> McpServer {
        self.state()
            .mcp_servers
            .into_iter()
            .find(|server| server.name == name)
            .unwrap_or_else(|| panic!("no server called {name}"))
    }

    async fn save(
        &self,
        name: &str,
        topic_id: Option<&Topic>,
        transport: McpTransport,
        enabled: bool,
    ) -> Result<McpServer, String> {
        let output = self
            .run(Command::SaveMcpServer {
                id: None,
                name: name.to_owned(),
                topic_id: topic_id.map(|t| t.id.clone()),
                transport,
                enabled,
            })
            .await?;
        match output {
            CommandOutput::McpServer { server } => Ok(server),
            other => panic!("unexpected output {other:?}"),
        }
    }

    async fn topic(&self) -> Topic {
        let workspace = match self
            .run(Command::AddWorkspace {
                path: self.repo.display().to_string(),
            })
            .await
            .unwrap()
        {
            CommandOutput::Workspace { workspace } => workspace,
            other => panic!("unexpected output {other:?}"),
        };
        match self
            .run(Command::CreateTopic {
                workspace_id: workspace.id,
                name: None,
                at_root: false,
            })
            .await
            .unwrap()
        {
            CommandOutput::Topic { topic, .. } => topic,
            other => panic!("unexpected output {other:?}"),
        }
    }

    async fn session(&self, topic: &Topic) -> SessionRef {
        let output = self
            .run(Command::StartSession {
                topic_id: topic.id.clone(),
                agent: "fake".to_owned(),
                mode: PermissionMode::Auto,
                model: None,
                effort: None,
            })
            .await
            .unwrap();
        let CommandOutput::Session { session, .. } = output else {
            panic!("unexpected output {output:?}");
        };
        let session = SessionRef {
            machine_id: self.machine.clone(),
            session_id: session.id,
        };
        self.engine.open_session(&session);
        self.wait_for_status(&session, SessionStatus::Ready).await;
        session
    }

    fn status(&self, session: &SessionRef) -> Option<SessionStatus> {
        self.state()
            .session(&session.session_id)
            .map(|session| session.status)
    }

    async fn wait_for_status(&self, session: &SessionRef, wanted: SessionStatus) {
        wait_for(&format!("status {wanted:?}"), || {
            (self.status(session) == Some(wanted)).then_some(())
        })
        .await;
    }

    fn messages(&self, session: &SessionRef) -> Vec<String> {
        self.engine
            .events_after(session, 0)
            .into_iter()
            .filter_map(|event| match event.kind {
                SessionEventKind::AgentMessage { text } => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Sends a script and returns what the agent said back.
    async fn ask(&self, session: &SessionRef, text: &str) -> String {
        let before = self.messages(session).len();
        self.run(Command::SendPrompt {
            session_id: session.session_id.clone(),
            text: text.to_owned(),
            images: vec![],
        })
        .await
        .unwrap();
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            if let Some(answer) = self.messages(session).get(before).cloned() {
                return answer;
            }
            if tokio::time::Instant::now() > deadline {
                let events: Vec<_> = self
                    .engine
                    .events_after(session, 0)
                    .into_iter()
                    .map(|event| event.kind)
                    .collect();
                panic!("no answer to {text}: {events:#?}");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

async fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> McpTransport {
    McpTransport::Stdio {
        command: command.to_owned(),
        args: args.iter().map(|a| a.to_string()).collect(),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn http(url: &str) -> McpTransport {
    McpTransport::Http {
        url: url.to_owned(),
        headers: BTreeMap::new(),
        oauth_client_id: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_topic_s_servers_reach_its_agent_and_a_change_restarts_it() {
    let f = Fixture::new().await;
    let home = std::env::var("HOME").unwrap();
    f.save(
        "files",
        None,
        stdio(
            "fake-mcp",
            &["--root", "${HOME}"],
            &[("TOKEN", "${TECTR_SURELY_UNSET}x")],
        ),
        true,
    )
    .await
    .unwrap();
    let remote = f
        .save("remote", None, http("https://example.invalid/mcp"), false)
        .await
        .unwrap();
    let topic = f.topic().await;
    let session = f.session(&topic).await;

    let report = f.ask(&session, "mcp").await;
    assert!(
        report.starts_with(&format!(
            "mcp=files:stdio:fake-mcp:--root,{home}:TOKEN=x meta="
        )),
        "{report}"
    );

    // An idle agent stops, and its next prompt starts it with the new servers.
    f.run(Command::SetTopicMcp {
        topic_id: topic.id.clone(),
        server_id: remote.id.clone(),
        on: Some(true),
    })
    .await
    .unwrap();
    f.wait_for_status(&session, SessionStatus::Stopped).await;
    let report = f.ask(&session, "mcp").await;
    let proxy = format!("http://127.0.0.1:{}/mcp/", f.daemon.addr().port());
    assert!(report.contains(" remote:http:"), "{report}");
    assert!(report.contains(&proxy), "{report}");
    assert!(
        report.contains(&format!("/{}", remote.id)),
        "the proxy address names the server: {report}"
    );

    // A working agent finishes its turn first.
    f.run(Command::SendPrompt {
        session_id: session.session_id.clone(),
        text: "hang".to_owned(),
        images: vec![],
    })
    .await
    .unwrap();
    f.wait_for_status(&session, SessionStatus::Working).await;
    let files = f.server("files");
    f.run(Command::SaveMcpServer {
        id: Some(files.id.clone()),
        name: "files".to_owned(),
        topic_id: None,
        transport: files.transport.clone(),
        enabled: false,
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(f.status(&session), Some(SessionStatus::Working));
    f.run(Command::CancelTurn {
        session_id: session.session_id.clone(),
    })
    .await
    .unwrap();
    f.wait_for_status(&session, SessionStatus::Stopped).await;
    let report = f.ask(&session, "mcp").await;
    assert!(report.starts_with("mcp=remote:http:"), "{report}");
    assert!(!report.contains("files"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn names_are_checked_and_an_archived_topic_keeps_its_own_servers() {
    let f = Fixture::new().await;
    let topic = f.topic().await;
    let tool = stdio("x", &[], &[]);

    let error = f
        .save("has space", None, tool.clone(), true)
        .await
        .unwrap_err();
    assert!(error.contains("letters, digits"), "{error}");
    let error = f
        .save("bad", None, http("ftp://x"), true)
        .await
        .unwrap_err();
    assert!(error.contains("http://"), "{error}");

    f.save("db", None, tool.clone(), true).await.unwrap();
    let error = f
        .save("db", Some(&topic), tool.clone(), true)
        .await
        .unwrap_err();
    assert!(error.contains("already"), "{error}");
    let own = f
        .save("own", Some(&topic), tool.clone(), true)
        .await
        .unwrap();
    let error = f
        .run(Command::SaveMcpServer {
            id: Some(own.id.clone()),
            name: "own".to_owned(),
            topic_id: None,
            transport: tool.clone(),
            enabled: true,
        })
        .await
        .unwrap_err();
    assert!(error.contains("can't move"), "{error}");

    let gets = |state: &DaemonState| -> Vec<String> {
        let topic = state.topic(&topic.id).unwrap();
        state
            .topic_mcp_servers(topic)
            .map(|s| s.name.clone())
            .collect()
    };
    assert_eq!(gets(&f.state()), ["db", "own"]);

    // An archived topic keeps its own servers, so restoring it brings them back.
    f.run(Command::ArchiveTopic {
        topic_id: topic.id.clone(),
        force: true,
    })
    .await
    .unwrap();
    assert!(f.state().topic(&topic.id).is_none());
    let names: Vec<_> = f.state().mcp_servers.into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["db", "own"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn import_copies_other_agents_servers_once() {
    let f = Fixture::new().await;
    let home = f.root.join("home");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"github":{"command":"gh-mcp"},"old":{"type":"sse","url":"https://o"}}}"#,
    )
    .unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        "[mcp_servers.sentry]\ncommand = \"sentry-mcp\"\nenabled = false\n",
    )
    .unwrap();

    let CommandOutput::McpImport { added, skipped } =
        f.run(Command::ImportMcpServers).await.unwrap()
    else {
        panic!("unexpected output");
    };
    assert_eq!(added, ["github", "sentry"]);
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(!f.server("sentry").enabled, "an off server stays off");

    let CommandOutput::McpImport { added, skipped } =
        f.run(Command::ImportMcpServers).await.unwrap()
    else {
        panic!("unexpected output");
    };
    assert!(added.is_empty());
    assert_eq!(skipped.len(), 3, "{skipped:?}");
}

/// A fake MCP server that is also its own authorization server, as the spec allows.
#[derive(Default)]
struct Upstream {
    /// The only token the MCP endpoint accepts.
    valid_token: Option<String>,
    challenge: Option<String>,
    seen: Vec<Option<String>>,
    refreshed: usize,
}

type Shared = Arc<Mutex<Upstream>>;

async fn start_upstream() -> (SocketAddr, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let shared: Shared = Arc::default();
    let state = shared.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request| {
                    upstream(addr, state.clone(), request)
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (addr, shared)
}

fn respond(status: StatusCode, body: serde_json::Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    response
}

fn query(uri: &hyper::Uri) -> BTreeMap<String, String> {
    reqwest::Url::parse(&format!("http://x{uri}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

async fn upstream(
    addr: SocketAddr,
    state: Shared,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let base = format!("http://{addr}");
    let path = request.uri().path().to_owned();
    let auth = request
        .headers()
        .get("authorization")
        .map(|v| v.to_str().unwrap().to_owned());
    let uri = request.uri().clone();
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let form: BTreeMap<String, String> = url_form(&body);
    let response = match path.as_str() {
        "/.well-known/oauth-protected-resource/mcp" => respond(
            StatusCode::OK,
            serde_json::json!({
                "resource": format!("{base}/mcp"),
                "authorization_servers": [base],
                "scopes_supported": ["read", "write"],
            }),
        ),
        "/.well-known/oauth-authorization-server" => respond(
            StatusCode::OK,
            serde_json::json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "code_challenge_methods_supported": ["S256"],
            }),
        ),
        "/register" => respond(
            StatusCode::CREATED,
            serde_json::json!({ "client_id": "client-1" }),
        ),
        "/authorize" => {
            let params = query(&uri);
            assert_eq!(params["client_id"], "client-1");
            assert_eq!(params["code_challenge_method"], "S256");
            assert_eq!(params["resource"], format!("{base}/mcp"));
            assert_eq!(params["scope"], "read write");
            state.lock().unwrap().challenge = Some(params["code_challenge"].clone());
            let mut response = Response::new(Full::new(Bytes::new()));
            *response.status_mut() = StatusCode::FOUND;
            let location = format!(
                "{}?code=code-1&state={}",
                params["redirect_uri"], params["state"]
            );
            response
                .headers_mut()
                .insert("location", location.parse().unwrap());
            response
        }
        "/token" => {
            let mut state = state.lock().unwrap();
            match form["grant_type"].as_str() {
                "authorization_code" => {
                    let hashed = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .encode(Sha256::digest(form["code_verifier"].as_bytes()));
                    assert_eq!(Some(hashed), state.challenge, "the verifier matches");
                    assert_eq!(form["code"], "code-1");
                    respond(
                        StatusCode::OK,
                        serde_json::json!({
                            "access_token": "a1",
                            "refresh_token": "r1",
                            "expires_in": 3600,
                            "token_type": "Bearer",
                        }),
                    )
                }
                _ => {
                    assert_eq!(form["refresh_token"], "r1");
                    state.refreshed += 1;
                    respond(
                        StatusCode::OK,
                        serde_json::json!({ "access_token": "a2", "expires_in": "3600" }),
                    )
                }
            }
        }
        "/mcp" => {
            let mut state = state.lock().unwrap();
            state.seen.push(auth.clone());
            let wanted = state.valid_token.as_ref().map(|t| format!("Bearer {t}"));
            if wanted.is_some() && auth == wanted {
                let mut response = respond(
                    StatusCode::OK,
                    serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": { "ok": true } }),
                );
                response
                    .headers_mut()
                    .insert("mcp-session-id", "s-1".parse().unwrap());
                response
                    .headers_mut()
                    .insert("set-cookie", "leak=1".parse().unwrap());
                response
            } else {
                let mut response = respond(StatusCode::UNAUTHORIZED, serde_json::json!({}));
                let challenge = format!(
                    "Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\""
                );
                response
                    .headers_mut()
                    .insert("www-authenticate", challenge.parse().unwrap());
                response
            }
        }
        _ => respond(StatusCode::NOT_FOUND, serde_json::json!({})),
    };
    Ok(response)
}

fn url_form(body: &[u8]) -> BTreeMap<String, String> {
    reqwest::Url::parse(&format!("http://x/?{}", String::from_utf8_lossy(body)))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_proxy_logs_in_refreshes_and_keeps_tokens_from_the_agent() {
    let f = Fixture::new().await;
    let (addr, upstream) = start_upstream().await;
    let server = f
        .save("linear", None, http(&format!("http://{addr}/mcp")), true)
        .await
        .unwrap();
    assert_eq!(
        server.auth,
        McpAuth::NeedsLogin,
        "saving waits to find out, so a client can log in straight away"
    );

    // Plays the browser: the authorization server sends it back to the daemon.
    let CommandOutput::McpLogin { url } = f
        .run(Command::LoginMcpServer {
            id: server.id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("unexpected output");
    };
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let redirect = browser.get(&url).send().await.unwrap();
    assert_eq!(redirect.status(), reqwest::StatusCode::FOUND);
    let back = redirect.headers()["location"].to_str().unwrap().to_owned();
    assert!(
        back.starts_with(&format!(
            "http://localhost:{}/oauth/callback?",
            f.daemon.addr().port()
        )),
        "{back}"
    );
    let back = back.replace("localhost", "127.0.0.1");
    let page = browser.get(&back).send().await.unwrap();
    assert_eq!(page.status(), reqwest::StatusCode::OK);
    assert!(page.text().await.unwrap().contains("Logged in to linear"));
    wait_for("the login to reach the client", || {
        (f.server("linear").auth == McpAuth::LoggedIn).then_some(())
    })
    .await;
    let used_again = browser.get(&back).send().await.unwrap();
    assert_eq!(used_again.status(), reqwest::StatusCode::BAD_REQUEST);

    // The server has since revoked the first token, so the proxy refreshes and goes again.
    upstream.lock().unwrap().valid_token = Some("a2".to_owned());
    let topic = f.topic().await;
    let session = f.session(&topic).await;
    let report = f.ask(&session, "mcp-call linear").await;
    assert!(
        report.starts_with("status=200 session=s-1 body="),
        "{report}"
    );
    {
        let upstream = upstream.lock().unwrap();
        assert_eq!(upstream.refreshed, 1);
        let seen: Vec<_> = upstream.seen.iter().rev().take(2).rev().cloned().collect();
        assert_eq!(
            seen,
            [Some("Bearer a1".to_owned()), Some("Bearer a2".to_owned())]
        );
    }
    let report = f.ask(&session, "mcp").await;
    assert!(
        report.contains(&format!("/{}: meta=", server.id)),
        "the agent gets no headers, so it never sees a token: {report}"
    );

    // Only the session's own secret gets through, and never from a browser.
    let proxy = format!("http://{}/mcp", f.daemon.addr());
    let plain = reqwest::Client::new();
    let wrong = plain
        .post(format!("{proxy}/not-a-secret/{}", server.id))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), reqwest::StatusCode::NOT_FOUND);
    let from_page = plain
        .post(format!("{proxy}/x/{}", server.id))
        .header("origin", "https://evil.example")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(from_page.status(), reqwest::StatusCode::FORBIDDEN);

    // After logging out, the server turns the agent away and says it needs a login.
    f.run(Command::LogoutMcpServer {
        id: server.id.clone(),
    })
    .await
    .unwrap();
    assert_eq!(f.server("linear").auth, McpAuth::NeedsLogin);
    let report = f.ask(&session, "mcp-call linear").await;
    assert!(report.starts_with("status=401"), "{report}");
    assert!(report.contains("needs a login"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_a_login_is_proxied_with_its_own_headers() {
    let f = Fixture::new().await;
    let (addr, upstream) = start_upstream().await;
    upstream.lock().unwrap().valid_token = Some("static".to_owned());
    f.save(
        "keyed",
        None,
        McpTransport::Http {
            url: format!("http://{addr}/mcp"),
            headers: BTreeMap::from([("Authorization".to_owned(), "Bearer static".to_owned())]),
            oauth_client_id: None,
        },
        true,
    )
    .await
    .unwrap();
    let topic = f.topic().await;
    let session = f.session(&topic).await;
    let report = f.ask(&session, "mcp-call keyed").await;
    assert!(report.starts_with("status=200"), "{report}");
    assert_eq!(f.server("keyed").auth, McpAuth::None);
}
