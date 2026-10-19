//! `correntd`: Corrent's headless server — the torrent engine, the API v1 with realtime events,
//! the qBittorrent-compatible API and the web interface, over HTTPS with its own certificate.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use corrent_core::config::{Paths, Settings};
use corrent_core::engine::{EngineHandle, Wake};
use corrent_server::identity::{HashCost, Identity};
use corrent_server::{ServerConfig, ServerParts, tls};

#[cfg(windows)]
mod winservice;

const HELP: &str = "\
Usage: correntd [OPTIONS]
       correntd service install [--data-dir <dir>] [--account <user> --password <pw>]
       correntd service uninstall | run [OPTIONS]      (Windows)

Options:
  --portable              keep all data in correntd-data next to the executable
  --data-dir <dir>        use this folder for the server's data
  --bind <address>        HTTPS address (default from server.toml: 0.0.0.0)
  --https-port <port>     HTTPS port (default 8443)
  --http-port <port>      loopback-only HTTP port (default 8088; 0 = off)
  --no-tls                serve plain HTTP on --https-port instead (testing only)
  --remote-access         accept clients from outside the local network
  --init-admin <name>     create the administrator (password from CORRENT_ADMIN_PASSWORD
                          or standard input) and exit
  --print-fingerprint     print the certificate fingerprint clients pin, and exit
  -V, --version           print the version
  -h, --help              show this help

Environment: CORRENT_LOG (log filter, e.g. \"debug\").
";

struct Args {
    portable: bool,
    data_dir: Option<PathBuf>,
    bind: Option<String>,
    https_port: Option<u16>,
    http_port: Option<u16>,
    no_tls: bool,
    remote_access: bool,
    init_admin: Option<String>,
    print_fingerprint: bool,
    service: Option<Service>,
    account: Option<String>,
    password: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Service {
    Install,
    Uninstall,
    Run,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        portable: false,
        data_dir: None,
        bind: None,
        https_port: None,
        http_port: None,
        no_tls: false,
        remote_access: false,
        init_admin: None,
        print_fingerprint: false,
        service: None,
        account: None,
        password: None,
    };
    let mut it = std::env::args().skip(1).peekable();
    if it.peek().map(String::as_str) == Some("service") {
        it.next();
        a.service = Some(match it.next().as_deref() {
            Some("install") => Service::Install,
            Some("uninstall") => Service::Uninstall,
            Some("run") => Service::Run,
            _ => return Err("service needs install, uninstall or run".into()),
        });
    }
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("correntd {}\n\n{HELP}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("correntd {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--portable" => a.portable = true,
            "--data-dir" => a.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--bind" => a.bind = Some(value("--bind")?),
            "--https-port" => a.https_port = Some(value("--https-port")?.parse().map_err(|_| "bad --https-port")?),
            "--http-port" => a.http_port = Some(value("--http-port")?.parse().map_err(|_| "bad --http-port")?),
            "--no-tls" => a.no_tls = true,
            "--remote-access" => a.remote_access = true,
            "--init-admin" => a.init_admin = Some(value("--init-admin")?),
            "--print-fingerprint" => a.print_fingerprint = true,
            "--account" => a.account = Some(value("--account")?),
            "--password" => a.password = Some(value("--password")?),
            other => return Err(format!("unknown option {other} (see --help)")),
        }
    }
    Ok(Some(a))
}

fn init_logging(paths: &Paths) {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_env("CORRENT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,librqbit=info,librqbit_dht=warn,tracing=warn"));
    let _ = std::fs::create_dir_all(paths.log_dir());
    let log_file = paths.log_dir().join("correntd.log");
    if std::fs::metadata(&log_file).is_ok_and(|m| m.len() > 10 * 1024 * 1024) {
        let _ = std::fs::rename(&log_file, paths.log_dir().join("correntd.old.log"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&log_file).ok();
    let file_layer =
        file.map(|f| tracing_subscriber::fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(f)));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .with(corrent_core::logbuf::LogCapture)
        .init();
}

/// Tells systemd (Type=notify) that the server is ready or stopping.
fn sd_notify(state: &str) {
    #[cfg(target_os = "linux")]
    if let Some(sock) = std::env::var_os("NOTIFY_SOCKET") {
        use std::os::unix::net::UnixDatagram;
        let path = PathBuf::from(&sock);
        if let Ok(s) = UnixDatagram::unbound() {
            let r = if let Some(name) = sock.to_str().and_then(|s| s.strip_prefix('@')) {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
                    .and_then(|a| s.send_to_addr(state.as_bytes(), &a))
            } else {
                s.send_to(state.as_bytes(), &path)
            };
            if let Err(e) = r {
                tracing::debug!("sd_notify: {e}");
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = state;
}

fn read_password() -> anyhow::Result<String> {
    if let Ok(p) = std::env::var("CORRENT_ADMIN_PASSWORD") {
        return Ok(p);
    }
    eprint!("Password for the administrator: ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn main() {
    let args = match parse_args() {
        Ok(Some(a)) => a,
        Ok(None) => return,
        Err(e) => {
            eprintln!("correntd: {e}");
            std::process::exit(2);
        }
    };
    let result = match args.service {
        None => run(args),
        Some(s) => service(s, args),
    };
    if let Err(e) = result {
        eprintln!("correntd: {e:#}");
        std::process::exit(1);
    }
}

/// Asks a running server to shut down (the Windows service manager's Stop).
static STOP: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[cfg(windows)]
fn service(s: Service, args: Args) -> anyhow::Result<()> {
    static ARGS: std::sync::OnceLock<std::sync::Mutex<Option<Args>>> = std::sync::OnceLock::new();
    match s {
        Service::Install => winservice::install(args.data_dir, args.account, args.password),
        Service::Uninstall => winservice::uninstall(),
        Service::Run => {
            let mut args = args;
            if args.data_dir.is_none() && !args.portable {
                args.data_dir = Some(winservice::default_data_dir());
            }
            let _ = ARGS.set(std::sync::Mutex::new(Some(args)));
            winservice::run(winservice::Hooks {
                run: || {
                    let args = ARGS.get().and_then(|a| a.lock().ok()?.take());
                    run(args.ok_or_else(|| anyhow::anyhow!("started twice"))?)
                },
                stop: || STOP.notify_one(),
            })
        }
    }
}

#[cfg(not(windows))]
fn service(_: Service, _: Args) -> anyhow::Result<()> {
    anyhow::bail!(
        "services are a Windows thing; on Linux use packaging/server/linux/correntd.service, on macOS \
         packaging/server/macos/io.github.ulaas.correntd.plist"
    )
}

fn run(args: Args) -> anyhow::Result<()> {
    let paths = Paths::resolve_server(args.portable, args.data_dir.clone());
    std::fs::create_dir_all(&paths.data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&paths.data_dir, std::fs::Permissions::from_mode(0o700));
    }
    if args.print_fingerprint {
        // Read-only: works next to a running server and never creates certificates.
        match tls::existing_fingerprint(&paths.data_dir.join("tls")) {
            Some(f) => println!("{f}\n{}", tls::short_fingerprint(&f)),
            None => println!("no certificate yet (the server makes one at its first start)"),
        }
        return Ok(());
    }
    init_logging(&paths);

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("correntd-rt").build()?;
    let db = corrent_db::Db::open(&paths.data_dir.join("corrent.db"))?;
    let identity = Identity::new(db.clone(), HashCost::STANDARD);

    if let Some(name) = &args.init_admin {
        // Works while the server runs (SQLite serializes the write).
        let password = read_password()?;
        let u = rt.block_on(identity.create_user(corrent_server::identity::new_admin(name, &password)))?;
        println!("created administrator {:?}", u.name);
        return Ok(());
    }

    // One server per profile: two engines on one session would corrupt it.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.data_dir.join("correntd.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("another correntd is already using {}", paths.data_dir.display())
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }

    let mut cfg = ServerConfig::load(&paths.data_dir)?;
    if !ServerConfig::file(&paths.data_dir).exists() {
        cfg.save(&paths.data_dir)?;
    }
    if let Some(b) = &args.bind {
        cfg.bind = b.clone();
    }
    if let Some(p) = args.https_port {
        cfg.https_port = p;
    }
    if let Some(p) = args.http_port {
        cfg.http_port = p;
    }
    if args.no_tls {
        cfg.tls = false;
    }
    if args.remote_access {
        cfg.remote_access = true;
    }

    let server_id = rt.block_on(identity.server_id()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let tls_material = if !cfg.tls {
        None
    } else if let (Some(c), Some(k)) = (&cfg.cert_path, &cfg.key_path) {
        Some(tls::load_custom(c, k)?)
    } else {
        let (names, ips) = tls::local_names(&cfg.hostnames);
        Some(tls::ensure(&paths.data_dir.join("tls"), &server_id, &names, &ips)?)
    };
    let fingerprint = tls_material.as_ref().map(|t| t.fingerprint.clone()).filter(|f| !f.is_empty());

    corrent_core::crash::install_hook(paths.log_dir());
    let previous = corrent_core::crash::start(&paths.log_dir());
    if previous.unclean {
        tracing::warn!("the previous run did not shut down cleanly");
    }
    if let Some(note) = corrent_core::backup::apply_pending(&paths) {
        tracing::info!("{note}");
    }
    let (settings, _first_run, warning) = Settings::load(&paths);
    if let Some(w) = warning {
        tracing::warn!("{w}");
    }
    corrent_i18n::set_language(&settings.appearance.language);

    let _guard = rt.enter();
    let hub: Arc<corrent_server::events::Hub> = Default::default();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel();
    let wake = {
        let hub = hub.clone();
        Wake::new(move || hub.wake())
    };
    let engine = EngineHandle::start(rt.handle(), paths.clone(), settings.clone(), wake, ev_tx);

    let result = rt.block_on(serve(
        ev_rx,
        ServerParts {
            engine: engine.clone(),
            settings: Arc::new(ArcSwap::from_pointee(settings)),
            paths: paths.clone(),
            identity,
            cfg,
            hub,
            tls_fingerprint: fingerprint,
            ca_pem: tls_material.as_ref().map(|t| t.ca_pem.clone()).filter(|p| !p.is_empty()),
            library: corrent_library::Library::new(
                db,
                &paths.data_dir,
                corrent_library::probe::Prober::new(corrent_library::probe::Prober::find_worker(), 3),
            ),
        },
        tls_material,
    ));
    sd_notify("STOPPING=1");
    tracing::info!("shutting down");
    let e2 = engine.clone();
    let _ = rt.block_on(tokio::task::spawn_blocking(move || e2.shutdown_blocking(std::time::Duration::from_secs(20))));
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
    corrent_core::crash::finish(&paths.log_dir());
    drop(lock);
    result
}

async fn serve(
    events: std::sync::mpsc::Receiver<corrent_core::engine::EngineEvent>,
    parts: ServerParts,
    tls_material: Option<tls::TlsMaterial>,
) -> anyhow::Result<()> {
    let cfg = parts.cfg.clone();
    let state = corrent_server::build_state(parts).await?;
    // Events queued while the server was starting are handled now.
    corrent_server::forward_engine_events(events, state.clone());
    corrent_server::start_wanted_scheduler(state.clone());
    let app = corrent_server::router(state.clone());
    let ip: IpAddr = cfg.bind.parse().map_err(|_| anyhow::anyhow!("bad bind address {:?}", cfg.bind))?;
    let main_addr = SocketAddr::new(ip, cfg.https_port);
    let mut listeners = Vec::new();
    let scheme = match &tls_material {
        Some(t) => {
            listeners.push(corrent_server::serve_https(app.clone(), main_addr, t).await?);
            "https"
        }
        None => {
            listeners.push(corrent_server::serve_http(app.clone(), main_addr).await?);
            "http"
        }
    };
    if cfg.http_port != 0 && cfg.tls {
        let local = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), cfg.http_port);
        match corrent_server::serve_http(app, local).await {
            Ok(l) => listeners.push(l),
            Err(e) => tracing::warn!("loopback HTTP port {} unavailable: {e}", cfg.http_port),
        }
    }
    let shown = if ip.is_unspecified() { "localhost".to_string() } else { ip.to_string() };
    println!("Corrent server {} is running.", env!("CARGO_PKG_VERSION"));
    println!("  {scheme}://{shown}:{}/", listeners[0].addr.port());
    if let Some(l) = listeners.get(1) {
        println!("  http://127.0.0.1:{}/   (this computer only)", l.addr.port());
    }
    if let Some(f) = &state.tls_fingerprint {
        println!("  certificate fingerprint: {}", tls::short_fingerprint(f));
    }
    // LAN discovery, Corrent Connect and port mapping, as configured.
    corrent_server::remote::apply(&state, listeners[0].addr.port()).await;
    if let Some(id) = &state.remote_status.load().connect_id {
        println!("  Corrent Connect: {id}");
    }
    if state.identity.user_count().await.unwrap_or(1) == 0 {
        println!("  first run: open the address above on this computer to create the administrator,");
        println!("  or run `correntd --init-admin <name>`.");
    }
    // Libraries: watch their folders and rescan everything every six hours.
    corrent_server::restart_watching(&state).await;
    let lib = state.library.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
        loop {
            tick.tick().await;
            for l in lib.list().await.unwrap_or_default() {
                lib.start_scan(l.id);
            }
        }
    });
    sd_notify("READY=1");
    wait_for_shutdown().await;
    state.hub.publish(corrent_proto_bye());
    corrent_server::remote::stop(&state).await;
    drop(listeners);
    Ok(())
}

fn corrent_proto_bye() -> corrent_server::events_api::Event {
    corrent_server::events_api::Event::Bye
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = STOP.notified() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = STOP.notified() => {}
    }
}
