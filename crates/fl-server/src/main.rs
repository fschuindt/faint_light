use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

use fl_server::{serve, Backend, Config};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env();
    tracing::info!(?cfg, "faint_light starting");
    let addr = SocketAddr::new(cfg.bind, cfg.port);

    let backend = Backend::new(cfg).unwrap_or_else(|e| {
        eprintln!("faint_light: {e}");
        eprintln!("hint: set FAINT_LIGHT_INDEX_DIR to a directory of astrometry.net index files");
        std::process::exit(1);
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let server = serve(rt.handle(), backend.state.clone(), addr).unwrap_or_else(|e| {
        eprintln!("faint_light: bind {addr}: {e}");
        std::process::exit(1);
    });

    rt.block_on(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    });
    server.stop();
}
