//! HTTP API server for the slicer.

use std::process::ExitCode;
use std::time::Duration;

use serde_json::json;
use slicer_server::{Config, log, serve};

/// Resolves on Ctrl-C or, on Unix, on SIGTERM (what `docker stop` sends).
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => tokio::select! {
                () = interrupt => {}
                _ = terminate.recv() => {}
            },
            Err(_) => interrupt.await,
        }
    }
    #[cfg(not(unix))]
    interrupt.await;
}

fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            log("error", "server.config", json!({ "message": message }));
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            log(
                "error",
                "server.runtime",
                json!({ "message": error.to_string() }),
            );
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(serve(config, shutdown_signal()));
    // Open requests have been answered; slices still running get a moment
    // to finish before the process exits.
    runtime.shutdown_timeout(Duration::from_secs(10));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log(
                "error",
                "server.io",
                json!({ "message": error.to_string() }),
            );
            ExitCode::FAILURE
        }
    }
}
