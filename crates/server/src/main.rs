//! cista-server 入口：读环境变量、开库（私有库 + 共享库）、监听。
//!
//! 运维只走 SSH + systemd；没有管理 API、没有热加载（`design.md` §4.4）。

use cista_server::app;
use cista_server::config::Config;
use cista_server::db::{PrivateStore, SharedStore};
use cista_server::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing(config.log_json);

    tokio::fs::create_dir_all(&config.data_dir).await?;

    // 一次性运维命令（SSH）：`cista-server gc` 回收过期链接/票据/共享副本。
    if std::env::args().nth(1).as_deref() == Some("gc") {
        let report = cista_server::gc::run(&config).await?;
        println!("{report}");
        return Ok(());
    }

    let private = PrivateStore::connect(&config.data_dir.join("private.db")).await?;
    let shared = SharedStore::connect(
        &config.data_dir.join("shared.db"),
        config.data_dir.join("blobs").join("shared"),
    )
    .await?;

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(
        bind = %config.bind,
        data_dir = %config.data_dir.display(),
        "cista-server listening"
    );

    let server = axum::serve(
        listener,
        app(AppState::new(config, private, shared))
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal());
    server.await?;
    Ok(())
}

fn init_tracing(json: bool) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry
            .with(fmt::layer().json().with_current_span(false))
            .init();
    } else {
        registry.with(fmt::layer()).init();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}
