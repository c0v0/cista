//! cista-server 入口：读环境变量、开库（私有库 + 共享库）、监听。
//!
//! 运维只走 SSH + systemd；没有管理 API、没有热加载（`design.md` §4.4）。

use cista_server::app;
use cista_server::config::Config;
use cista_server::db::{PrivateStore, SharedStore};
use cista_server::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command = std::env::args().nth(1);
    let is_gc = command.as_deref() == Some("gc");
    let is_reset_bootstrap = command.as_deref() == Some("reset-bootstrap");
    let config = if is_gc || is_reset_bootstrap {
        Config::from_env_for_gc()?
    } else {
        Config::from_env()?
    };
    init_tracing(config.log_json);

    tokio::fs::create_dir_all(&config.data_dir).await?;

    // 一次性运维命令（SSH）：`cista-server gc` 回收过期链接/票据/共享副本。
    if is_gc {
        let report = cista_server::gc::run(&config).await?;
        println!("{report}");
        return Ok(());
    }

    // 一次性运维命令（SSH）：Web 临时端刷新/换设备时，bootstrap 已消费会无法重新入网。
    // 清除标记后必须轮换 CISTA_BOOTSTRAP_TOKEN，再重启服务完成配对。
    if is_reset_bootstrap {
        let private = PrivateStore::connect(&config.data_dir.join("private.db")).await?;
        let removed = sqlx::query("DELETE FROM meta WHERE key = 'bootstrap_used'")
            .execute(private.pool())
            .await?
            .rows_affected();
        println!(
            "bootstrap_used removed={removed}; rotate CISTA_BOOTSTRAP_TOKEN and restart before enrolling"
        );
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

    let state = AppState::new(config, private, shared);
    tokio::spawn(maintenance(state.clone()));

    let server = axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal());
    server.await?;
    Ok(())
}

/// 后台维护：回收空闲 Yjs 文档与没有订阅者的广播通道（每分钟一次）。
async fn maintenance(state: AppState) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let unloaded = state.documents.unload_idle(600);
        state.events.retain_active();
        state.ws_events.retain_active();
        if unloaded > 0 {
            tracing::info!(unloaded, "idle yjs documents unloaded");
        }
    }
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
