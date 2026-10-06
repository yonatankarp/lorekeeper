use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use sync_server::{router, AppState, Config};

fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lorekeeper-sync: {e}");
            return ExitCode::FAILURE;
        }
    };
    if std::env::args().any(|a| a == "--healthcheck") {
        return healthcheck(config.addr);
    }
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    match runtime.block_on(serve(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lorekeeper-sync: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(config: Config) -> Result<(), String> {
    let addr = config.addr;
    let state = AppState::new(config)?;
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("can't listen on {addr}: {e}"))?;
    eprintln!("lorekeeper-sync {} listening on {addr}", env!("CARGO_PKG_VERSION"));
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            stop_signal().await;
            eprintln!("lorekeeper-sync shutting down");
            state.shut_down();
        })
        .await
        .map_err(|e| e.to_string())
}

/// SIGINT or SIGTERM (`docker stop`; the binary is PID 1 in the container).
async fn stop_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// `--healthcheck`: GET /healthz on the listen port (loopback when listening on all addresses);
/// exit 0 on 200. For the container, which has no curl.
fn healthcheck(addr: SocketAddr) -> ExitCode {
    let target = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), addr.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::new(Ipv6Addr::LOCALHOST.into(), addr.port()),
        _ => addr,
    };
    let ok = (|| -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&target, Duration::from_secs(3))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut response = String::new();
        stream.take(4096).read_to_string(&mut response)?;
        Ok(response.split_whitespace().nth(1) == Some("200"))
    })();
    if matches!(ok, Ok(true)) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
