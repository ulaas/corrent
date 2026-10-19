//! Reaching the server: LAN discovery (mDNS), Corrent Connect (iroh) and port mapping, started
//! and stopped as the remote-access settings change.

use std::sync::Arc;

use corrent_proto::server::{RemoteConfig, RemoteStatus};

use crate::config_file::ServerConfig;
use crate::state::AppState;

/// What is running (dropping a part stops it).
#[derive(Default)]
pub struct Running {
    advertiser: Option<crate::discovery::Advertiser>,
    #[cfg(feature = "p2p")]
    connect: Option<crate::p2p::Listener>,
    #[cfg(feature = "p2p")]
    mapper: Option<(portmapper::Client, tokio::task::JoinHandle<()>)>,
    /// The settings the parts were started with.
    applied: Option<(bool, bool, bool, bool, String)>,
}

pub fn config_of(c: &ServerConfig) -> RemoteConfig {
    RemoteConfig {
        remote_access: c.remote_access,
        discovery: c.discovery,
        connect: c.connect,
        connect_relay: c.connect_relay,
        port_mapping: c.port_mapping,
        allowed_networks: c.allowed_networks.clone(),
        trusted_proxies: c.trusted_proxies.clone(),
        hostnames: c.hostnames.clone(),
    }
}

pub fn with_config(mut c: ServerConfig, r: RemoteConfig) -> ServerConfig {
    c.remote_access = r.remote_access;
    c.discovery = r.discovery;
    c.connect = r.connect;
    c.connect_relay = r.connect_relay;
    c.port_mapping = r.port_mapping;
    c.allowed_networks = r.allowed_networks;
    c.trusted_proxies = r.trusted_proxies;
    c.hostnames = r.hostnames;
    c
}

fn loopback_only(bind: &str) -> bool {
    bind.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) || bind == "localhost"
}

fn update(st: &AppState, f: impl FnOnce(&mut RemoteStatus)) {
    // Several tasks update different fields: one at a time, so none is lost.
    static UPDATING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = UPDATING.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = (**st.remote_status.load()).clone();
    f(&mut s);
    st.remote_status.store(Arc::new(s));
}

/// Starts or stops the parts so they match the server's configuration. `port`: the HTTPS
/// listener's port (0 when the server isn't listening on the network, e.g. hosted in the app).
pub async fn apply(st: &Arc<AppState>, port: u16) {
    let cfg = st.cfg.load_full();
    let name = cfg.display_name();
    let key = (cfg.discovery, cfg.connect, cfg.connect_relay, cfg.port_mapping, name.clone());
    let mut run = st.remote.lock().await;
    if run.applied.as_ref() == Some(&key) {
        return;
    }
    let previous = run.applied.replace(key);
    // A part that fails to start is tried again on the next apply (same settings or not).
    let mut failed = false;
    update(st, |s| s.https_port = port);

    // LAN discovery: only when the server listens beyond loopback.
    let want_ad = cfg.discovery && port != 0 && !loopback_only(&cfg.bind);
    let name_changed = previous.as_ref().is_some_and(|p| p.4 != name);
    if !want_ad || name_changed {
        run.advertiser = None;
    }
    if want_ad && run.advertiser.is_none() {
        let ad = crate::discovery::Advert {
            id: st.server_id.clone(),
            name: name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            fingerprint: st.tls_fingerprint.clone().unwrap_or_default(),
            https: cfg.tls,
            port,
        };
        match crate::discovery::advertise(&ad) {
            Ok(a) => {
                run.advertiser = Some(a);
                update(st, |s| {
                    s.discovery = true;
                    s.discovery_error = None;
                });
            }
            Err(e) => {
                failed = true;
                tracing::warn!("mDNS: {e}");
                update(st, |s| {
                    s.discovery = false;
                    s.discovery_error = Some(e.to_string());
                });
            }
        }
    } else if !want_ad {
        update(st, |s| s.discovery = false);
    }

    #[cfg(feature = "p2p")]
    {
        // Corrent Connect.
        let relay_changed = previous.as_ref().is_some_and(|p| p.2 != cfg.connect_relay);
        if !cfg.connect || relay_changed {
            run.connect = None;
            update(st, |s| {
                s.connect_id = None;
                s.connect_relay = None;
                s.connect_error = None;
            });
        }
        if cfg.connect && run.connect.is_none() {
            let started = match crate::p2p::load_or_create_key(&st.paths.data_dir) {
                Ok(key) => crate::p2p::serve(crate::router(st.clone()), key, cfg.connect_relay, None).await,
                Err(e) => Err(e),
            };
            match started {
                Ok(l) => {
                    let id = l.id().to_string();
                    tracing::info!("Corrent Connect is on: {id}");
                    let ep = l.endpoint.clone();
                    let st2 = st.clone();
                    // The home relay is known once the endpoint is online.
                    tokio::spawn(async move {
                        if tokio::time::timeout(std::time::Duration::from_secs(30), ep.online()).await.is_ok() {
                            let relay = ep.addr().relay_urls().next().map(|u| u.to_string());
                            update(&st2, |s| s.connect_relay = relay);
                        }
                    });
                    run.connect = Some(l);
                    update(st, |s| {
                        s.connect_id = Some(id);
                        s.connect_error = None;
                    });
                }
                Err(e) => {
                    failed = true;
                    tracing::warn!("Corrent Connect: {e:#}");
                    update(st, |s| s.connect_error = Some(format!("{e:#}")));
                }
            }
        }

        // Port mapping of the HTTPS port.
        if !cfg.port_mapping || port == 0 {
            if let Some((c, task)) = run.mapper.take() {
                c.deactivate();
                task.abort();
            }
            update(st, |s| {
                s.mapped_address = None;
                s.port_mapping_error = None;
            });
        } else if run.mapper.is_none()
            && let Some(p) = std::num::NonZeroU16::new(port)
        {
            let client = portmapper::Client::new(portmapper::Config {
                protocol: portmapper::Protocol::Tcp,
                ..Default::default()
            });
            client.update_local_port(p);
            let mut watch = client.watch_external_address();
            let st2 = st.clone();
            let task = tokio::spawn(async move {
                loop {
                    let addr = *watch.borrow_and_update();
                    update(&st2, |s| {
                        s.mapped_address = addr.map(|a| a.to_string());
                        s.port_mapping_error = None;
                    });
                    if watch.changed().await.is_err() {
                        break;
                    }
                }
            });
            // No router answered within a while: say so (mapping keeps trying).
            let st3 = st.clone();
            let probe = client.probe();
            tokio::spawn(async move {
                match tokio::time::timeout(std::time::Duration::from_secs(10), probe).await {
                    Ok(Ok(Ok(p))) if !(p.upnp || p.pcp || p.nat_pmp) => {
                        update(&st3, |s| s.port_mapping_error = Some("no UPnP, NAT-PMP or PCP router found".into()))
                    }
                    Ok(Ok(Err(e))) => update(&st3, |s| s.port_mapping_error = Some(e.to_string())),
                    _ => {}
                }
            });
            run.mapper = Some((client, task));
        }
    }
    #[cfg(not(feature = "p2p"))]
    {
        let msg = (cfg.connect || cfg.port_mapping).then(|| "this build has no Corrent Connect".to_string());
        update(st, |s| {
            s.connect_error = msg.clone();
            s.port_mapping_error = msg;
        });
    }
    if failed {
        run.applied = None;
    }
}

/// Stops everything (the server shuts down).
pub async fn stop(st: &AppState) {
    let mut run = st.remote.lock().await;
    #[cfg(feature = "p2p")]
    if let Some((c, task)) = run.mapper.take() {
        c.deactivate();
        task.abort();
    }
    *run = Running::default();
}
