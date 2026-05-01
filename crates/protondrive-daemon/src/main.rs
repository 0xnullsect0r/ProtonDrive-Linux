//! `protondrive-daemon` — headless background process that owns the
//! Proton bridge handle, the sync engine, and the SQLite state DB.
//! It exposes a UNIX-socket JSON-RPC API at
//! `$XDG_RUNTIME_DIR/protondrive.sock` for the UI and CLI to drive.

use anyhow::Result;
use protondrive_bridge::{Bridge, InitArgs, LoginArgs};
use protondrive_sync::{
    state::{default_state_path, State},
    SyncAgent,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

const APP_VERSION: &str = concat!(
    "external-drive-protondrive-linux@",
    env!("CARGO_PKG_VERSION"),
    "-stable"
);

#[derive(Default)]
struct AppState {
    bridge: Option<Bridge>,
    sync: Option<tokio::task::JoinHandle<()>>,
    sync_root: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let sock_path = socket_path();
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
    }
    if let Some(p) = sock_path.parent() {
        let _ = std::fs::create_dir_all(p);
    }

    let listener = UnixListener::bind(&sock_path)?;
    tracing::info!(socket = %sock_path.display(), "protondrive-daemon listening");

    let app = Arc::new(Mutex::new(AppState::default()));
    loop {
        let (stream, _) = listener.accept().await?;
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, app).await {
                tracing::warn!(error = %e, "client error");
            }
        });
    }
}

fn socket_path() -> PathBuf {
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(rt).join("protondrive.sock");
    }
    PathBuf::from("/tmp/protondrive.sock")
}

#[derive(Deserialize)]
#[serde(tag = "method", content = "params")]
enum Request {
    Status,
    Login(LoginParams),
    Resume(ResumeParams),
    Logout,
    SetSyncRoot { path: String },
    StartSync,
    Stop,
}

#[derive(Deserialize)]
struct LoginParams {
    username: String,
    password: String,
    #[serde(default)]
    mailbox_password: String,
    #[serde(default)]
    two_fa: String,
}

#[derive(Deserialize)]
struct ResumeParams {
    uid: String,
    access_token: String,
    refresh_token: String,
    salted_key_pass: String,
}

#[derive(Serialize)]
struct StatusResponse {
    authenticated: bool,
    syncing: bool,
    sync_root: Option<String>,
    version: &'static str,
}

#[derive(Serialize)]
struct LoginResponse {
    uid: String,
    access_token: String,
    refresh_token: String,
    salted_key_pass: String,
}

async fn handle_client(stream: UnixStream, app: Arc<Mutex<AppState>>) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    while reader.read_line(&mut line).await? != 0 {
        let response = match serde_json::from_str::<Request>(line.trim()) {
            Ok(req) => handle(req, &app).await,
            Err(e) => json_err(&format!("bad request: {e}")),
        };
        w.write_all(response.as_bytes()).await?;
        w.write_all(b"\n").await?;
        line.clear();
    }
    Ok(())
}

async fn handle(req: Request, app: &Arc<Mutex<AppState>>) -> String {
    match req {
        Request::Status => {
            let st = app.lock().await;
            ok(&StatusResponse {
                authenticated: st.bridge.is_some(),
                syncing: st.sync.is_some(),
                sync_root: st.sync_root.as_ref().map(|p| p.display().to_string()),
                version: env!("CARGO_PKG_VERSION"),
            })
        }
        Request::Login(p) => match do_login(p, app).await {
            Ok(c) => ok(&c),
            Err(e) => json_err(&e.to_string()),
        },
        Request::Resume(p) => match do_resume(p, app).await {
            Ok(c) => ok(&c),
            Err(e) => json_err(&e.to_string()),
        },
        Request::Logout => match do_logout(app).await {
            Ok(()) => ok(&"ok"),
            Err(e) => json_err(&e.to_string()),
        },
        Request::SetSyncRoot { path } => {
            app.lock().await.sync_root = Some(PathBuf::from(path));
            ok(&"ok")
        }
        Request::StartSync => match do_start_sync(app).await {
            Ok(()) => ok(&"started"),
            Err(e) => json_err(&e.to_string()),
        },
        Request::Stop => {
            std::process::exit(0);
        }
    }
}

async fn ensure_bridge(app: &Arc<Mutex<AppState>>) -> Result<Bridge> {
    let mut st = app.lock().await;
    if let Some(b) = &st.bridge {
        return Ok(b.clone());
    }
    let b = Bridge::init(InitArgs {
        app_version: APP_VERSION.into(),
        user_agent: format!("ProtonDrive-Linux/{}", env!("CARGO_PKG_VERSION")),
        enable_caching: true,
        concurrent_blocks: 5,
        concurrent_crypto: 3,
        replace_existing: true,
        ..Default::default()
    })
    .await?;
    st.bridge = Some(b.clone());
    Ok(b)
}

async fn do_login(p: LoginParams, app: &Arc<Mutex<AppState>>) -> Result<LoginResponse> {
    use protondrive_bridge::LoginOutcome;
    let bridge = ensure_bridge(app).await?;
    let outcome = bridge
        .login(LoginArgs {
            username: p.username,
            password: p.password,
            mailbox_password: p.mailbox_password,
            two_fa: p.two_fa,
        })
        .await?;
    let cred = match outcome {
        LoginOutcome::Success(c) => c,
        LoginOutcome::HvRequired { .. } => {
            return Err(anyhow::anyhow!(
                "Human verification required — use the GUI to complete CAPTCHA"
            ))
        }
    };
    Ok(LoginResponse {
        uid: cred.uid,
        access_token: cred.access_token,
        refresh_token: cred.refresh_token,
        salted_key_pass: cred.salted_key_pass,
    })
}

async fn do_resume(p: ResumeParams, app: &Arc<Mutex<AppState>>) -> Result<LoginResponse> {
    let bridge = ensure_bridge(app).await?;
    let cred = bridge
        .resume(protondrive_bridge::Credential {
            uid: p.uid,
            access_token: p.access_token,
            refresh_token: p.refresh_token,
            salted_key_pass: p.salted_key_pass,
        })
        .await?;
    Ok(LoginResponse {
        uid: cred.uid,
        access_token: cred.access_token,
        refresh_token: cred.refresh_token,
        salted_key_pass: cred.salted_key_pass,
    })
}

async fn do_logout(app: &Arc<Mutex<AppState>>) -> Result<()> {
    let mut st = app.lock().await;
    if let Some(h) = st.sync.take() {
        h.abort();
    }
    if let Some(b) = st.bridge.take() {
        b.logout().await.ok();
    }
    Ok(())
}

async fn do_start_sync(app: &Arc<Mutex<AppState>>) -> Result<()> {
    let bridge = {
        let st = app.lock().await;
        st.bridge
            .clone()
            .ok_or_else(|| anyhow::anyhow!("not logged in"))?
    };
    let root = {
        let st = app.lock().await;
        st.sync_root
            .clone()
            .unwrap_or_else(|| dirs_home().join("ProtonDrive"))
    };
    std::fs::create_dir_all(&root)?;
    let state = State::open(&default_state_path())?;
    let agent = SyncAgent::new(bridge, state, root.clone());
    let handle = tokio::spawn(async move {
        if let Err(e) = agent.run().await {
            tracing::error!(error = %e, "sync agent stopped");
        }
    });
    let mut st = app.lock().await;
    if let Some(h) = st.sync.take() {
        h.abort();
    }
    st.sync = Some(handle);
    st.sync_root = Some(root);
    Ok(())
}

fn dirs_home() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn ok<T: Serialize>(v: &T) -> String {
    serde_json::to_string(&serde_json::json!({ "ok": v })).unwrap_or_default()
}

fn json_err(msg: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "err": msg })).unwrap_or_default()
}
