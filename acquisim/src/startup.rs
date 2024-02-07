use std::sync::Arc;

use axum::routing::IntoMakeService;
use axum::serve::Serve;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::{
    bank::Bank,
    config::Settings,
    routes::{api::api_router, system::system_router},
};

type Server = Serve<IntoMakeService<Router>, Router>;

pub struct Application {
    port: u16,
    server: Server,
}

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub bank: Bank,
}

impl Application {
    pub async fn build(config: Settings) -> Result<Self, anyhow::Error> {
        let port = config.port;
        let addr = format!("{}:{}", config.addr, port);
        let listener = TcpListener::bind(addr).await?;

        let app_state = AppState {
            bank: Bank::new(
                &config.terminal_settings.password,
                &config.bank_username,
            ),
            settings: Arc::new(config),
        };

        let app = Router::new()
            .nest("/api", api_router(app_state.clone()))
            .nest("/system", system_router(app_state));

        let server = axum::serve(listener, app.into_make_service());

        Ok(Self { port, server })
    }

    pub async fn run_until_stopped(self) -> Result<(), std::io::Error> {
        self.server
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        Ok(())
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    let terminate = async {
        tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("failed to install signal handler")
        .recv()
        .await;
    };
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("Terminate signal received");
}
