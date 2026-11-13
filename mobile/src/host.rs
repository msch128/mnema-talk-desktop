//! The state of the phone host: the verified bundles, and the one server that
//! is open. With no server open the page is the server picker of the bundle
//! (the desktop's "selector" role); with one it is that server's page.
//!
//! One server at a time: the web app keeps the device key of the end-to-end
//! media in the browser storage of the app origin, and the phone has one such
//! storage until the shell gives every server a data store of its own
//! (doc/mobile.md, "Several servers").
use crate::web_bundle::{Held, ServerInfo, Store, trusted_keys};
use crate::web_proxy::{Instance, Jar, Limits, client, now};
use crate::web_vault::Os;
use crate::web_ws::Sockets;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{Manager, WebviewUrl, webview::WebviewWindowBuilder};
use url::Url;

/// The signed bundle compiled into the app.
static BUILTIN_BUNDLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/builtin-bundle.zip"));

/// The label of the one webview.
pub const MAIN: &str = "main";

/// The open server: its origin, the bundle it was matched with, its session
/// and its sockets.
pub struct Active {
    pub held: Held,
    pub instance: Arc<Instance>,
    pub sockets: Arc<Sockets>,
}

pub struct Host {
    pub store: Mutex<Store>,
    data_dir: Option<PathBuf>,
    pub active: Mutex<Option<Arc<Active>>>,
}

impl Host {
    fn new(data_dir: Option<PathBuf>) -> Host {
        let store = Store::load(
            data_dir.as_ref().map(|d| d.join("web-bundles")).as_deref(),
            (!BUILTIN_BUNDLE.is_empty()).then_some(BUILTIN_BUNDLE),
            &trusted_keys(),
        );
        Host {
            store: Mutex::new(store),
            data_dir,
            active: Mutex::new(None),
        }
    }

    pub fn active(&self) -> Option<Arc<Active>> {
        self.active.lock().ok()?.clone()
    }

    /// The bundle the picker runs: the newest one held.
    pub fn picker_bundle(&self) -> Option<Held> {
        self.store.lock().ok()?.newest().cloned()
    }

    fn select(&self, server: &ServerInfo) -> Result<Held, String> {
        let store = self.store.lock().map_err(|_| "busy")?;
        store.select(server).map_err(|why| why.to_string())
    }

    /// The file the session rests in where the system has no keystore the
    /// shared vault can use yet: private to the app on both systems.
    fn jar_file(&self, origin: &Url) -> Option<PathBuf> {
        let name = hex::encode(&Sha256::digest(origin.as_str().as_bytes())[..8]);
        Some(
            self.data_dir
                .as_ref()?
                .join("sessions")
                .join(format!("{name}.json")),
        )
    }

    fn last_server_file(&self) -> Option<PathBuf> {
        Some(self.data_dir.as_ref()?.join("server.txt"))
    }

    fn remember(&self, origin: &Url) {
        if let Some(file) = self.last_server_file() {
            let _ = crate::web_bundle::write_atomic(&file, origin.as_str().as_bytes());
        }
    }

    fn remembered(&self) -> Option<Url> {
        let text = std::fs::read_to_string(self.last_server_file()?).ok()?;
        crate::web_client::address(text.trim()).ok()
    }
}

/// Checks a server and opens it: the address is read, the server asked what
/// it is, the verified bundle for it chosen (never its own JavaScript), and
/// only then does the page switch to it.
pub async fn open_server(app: &tauri::AppHandle, address: &str) -> Result<(), String> {
    let origin = crate::web_client::address(address)?;
    let (server, _community, _icon) = crate::web_client::discover(&origin).await?;
    let host = app.state::<Host>();
    let held = host.select(&server)?;
    let instance = Arc::new(Instance {
        client: client().map_err(|_| crate::web_client::NO_HTTPS)?,
        jar: Jar::in_vault(host.jar_file(&origin), Arc::new(Os), now()),
        origin: origin.clone(),
        limits: Limits::default(),
    });
    let active = Arc::new(Active {
        held,
        instance,
        sockets: Arc::new(Sockets::default()),
    });
    let previous = host
        .active
        .lock()
        .map_err(|_| "busy")?
        .replace(Arc::clone(&active));
    if let Some(previous) = previous {
        previous.sockets.close_all();
    }
    host.remember(&origin);
    show_app(app);
    Ok(())
}

/// Loads the app origin again, which shows what the host's state says now.
pub fn show_app(app: &tauri::AppHandle) {
    if let Some(page) = app.get_webview_window(MAIN) {
        let _ = page.navigate(crate::web_client::app_url());
    }
}

fn create_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let url = WebviewUrl::CustomProtocol(crate::web_client::app_url());
    let builder = WebviewWindowBuilder::new(app, MAIN, url)
        .use_https_scheme(true)
        .on_navigation(crate::web_client::bundled);
    // Android's WebView hands the host no request body: the page sends those
    // requests over IPC instead (page-shim/ipc-bodies.js, `mobile_api`).
    #[cfg(target_os = "android")]
    let builder = builder.initialization_script(crate::protocol::IPC_BODIES);
    builder.build()?;
    Ok(())
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    crate::web_tls::install_provider();
    #[cfg(target_os = "android")]
    crate::android_tls::init();
    let data_dir = app.path().app_local_data_dir().ok();
    let host = Host::new(data_dir);
    let reopen = host.remembered();
    app.manage(host);
    create_window(app.handle())?;
    #[cfg(feature = "mobile-selftest")]
    crate::selftest::start(app.handle());
    #[cfg(not(feature = "mobile-selftest"))]
    if let Some(origin) = reopen {
        let app = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            // A server that cannot be reached now leaves the picker showing.
            let _ = open_server(&app, origin.as_str()).await;
        });
    }
    #[cfg(feature = "mobile-selftest")]
    let _ = reopen;
    Ok(())
}

pub fn run() {
    let result = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("mnema", crate::protocol::serve)
        .invoke_handler(tauri::generate_handler![
            crate::commands::desktop_open_instance,
            crate::commands::desktop_ws_open,
            crate::commands::desktop_ws_send,
            crate::commands::desktop_ws_close,
            crate::commands::mobile_api,
        ])
        .setup(setup)
        .run(tauri::generate_context!());
    if let Err(why) = result {
        eprintln!("Mnema Talk could not start: {why}");
    }
}
