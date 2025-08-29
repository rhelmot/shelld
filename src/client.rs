use clap::Args;
use futures::{SinkExt, StreamExt};
use http::Uri;
use rustix::termios;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async as ws_connect_async, MaybeTlsStream, WebSocketStream};
use tungstenite::ClientRequestBuilder;

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
use thiserror::Error;

/// Start a client to attach to an existing shelld server
#[derive(Args)]
pub struct ClientCli {
    /// The host and port of the shelld server to attach to
    #[arg(short, long, default_value = "localhost:3000")]
    address: String,
    /// The ID of the session to attach to
    session_id: u64,
}

type AttachConnection = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn request_attach(address: String, session_id: u64) -> Result<AttachConnection, Error> {
    let uri = Uri::builder()
        .scheme("ws")
        .authority(address)
        .path_and_query("/attach")
        .build()
        .map_err(|_| "Invalid address")?;
    let req = ClientRequestBuilder::new(uri)
        .with_header("Session", format!("{}", session_id));
    let (socket, _response) = ws_connect_async(req).await?;
    Ok(socket)
}

async fn send_terminal_size(address: String, session_id: u64) -> Result<(), Error> {
    let winsize = termios::tcgetwinsize(std::io::stdout())?;
    let uri = Uri::builder()
        .scheme("http")
        .authority(address)
        .path_and_query("/resize")
        .build()
        .map_err(|_| "Invalid address")?;
    // TODO: send
    Ok(())
}

pub async fn attach(options: ClientCli) -> Result<(), Error> {
    if !termios::isatty(std::io::stdout()) {
        return Err("stdout is not a tty, what are you thinking".into());
    }

    if !termios::isatty(std::io::stdin()) {
        return Err("stdin is not a tty, you monster".into());
    }

    let mut connection = match request_attach(options.address, options.session_id).await {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("Couldn't attach: {}", err);
            return Err(err);
        },
    };

    connection.close(None).await?;
    Ok(())
}
