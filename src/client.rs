use clap::Args;
use futures::{SinkExt, StreamExt};
use http::Uri;
use hyper::Request;
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

pub async fn request_attach(address: String, session_id: u64) -> Result<AttachConnection, Error> {
    let uri: Uri = Uri::builder()
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

pub async fn attach(options: ClientCli) -> Result<(), Error> {
    let mut connection = request_attach(options.address, options.session_id).await?;
    connection.send(tungstenite::Message::Binary(b"hello"[..].into())).await?;
    println!("got {:?}", &connection.next().await.unwrap()?.into_data()[..]);
    connection.close(None).await?;
    Ok(())
}
