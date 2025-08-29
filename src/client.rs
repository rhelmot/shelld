use std::time::Duration;

use clap::Args;
use futures::{SinkExt, StreamExt};
use http::Uri;
use hyper_util::rt::TokioIo;
use rustix::termios;
use signal_hook::consts::SIGWINCH;
use signal_hook_tokio::{Handle as SignalHandle, Signals};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
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

async fn request_attach(address: &str, session_id: u64) -> Result<AttachConnection, Error> {
    let uri = Uri::builder()
        .scheme("ws")
        .authority(address)
        .path_and_query("/attach")
        .build()
        .map_err(|_| "Invalid address")?;
    let req = ClientRequestBuilder::new(uri)
        .with_header("X-Session", format!("{}", session_id));
    let (socket, _response) = ws_connect_async(req).await?;
    Ok(socket)
}

async fn send_terminal_size(address: String, session_id: u64) -> Result<(), Error> {
    let winsize = termios::tcgetwinsize(std::io::stdout())?;

    let stream = TcpStream::connect(&address).await?;
    let io = TokioIo::new(stream);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            println!("Connection failed: {:?}", err);
        }
    });

    let req = hyper::Request::builder()
        .uri("/resize")
        .header(hyper::header::HOST, address)
        .header("X-Session", format!("{}", session_id))
        .header("X-Terminal-Columns", format!("{}", winsize.ws_col))
        .header("X-Terminal-Rows", format!("{}", winsize.ws_row))
        .body(http_body_util::Empty::<hyper::body::Bytes>::new())?;

    let mut res = sender.send_request(req).await?;

    Ok(())
}

struct ResizeHandle {
    signal_handle: SignalHandle,
    task_handle: JoinHandle<()>,
}

async fn handle_sigwinch(mut signals: Signals, address: String, session_id: u64) {
    while let Some(signal) = signals.next().await {
        match signal {
            SIGWINCH => {
                tokio::spawn(send_terminal_size(address.clone(), session_id));
            }
            _ => unreachable!(),
        }
    }
}

fn handle_resizes(address: &str, session_id: u64) -> Result<ResizeHandle, Error> {
    let signals = Signals::new(&[SIGWINCH])?;
    let signal_handle = signals.handle();
    let task_handle = tokio::spawn(handle_sigwinch(signals, address.into(), session_id));
    Ok(ResizeHandle { signal_handle, task_handle })
}

async fn clear_resize_listener(handles: ResizeHandle) -> Result<(), Error> {
    handles.signal_handle.close();
    Ok(handles.task_handle.await?)
}

pub async fn attach(options: ClientCli) -> Result<(), Error> {
    if !termios::isatty(std::io::stdout()) {
        return Err("stdout is not a tty, what are you thinking".into());
    }

    if !termios::isatty(std::io::stdin()) {
        return Err("stdin is not a tty, you monster".into());
    }

    let mut connection = match request_attach(&options.address, options.session_id).await {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("Couldn't attach: {}", err);
            return Err(err);
        },
    };

    let handle = handle_resizes(&options.address, options.session_id)?;

    tokio::time::sleep(Duration::from_secs(10)).await;

    connection.close(None).await?;
    clear_resize_listener(handle).await?;

    Ok(())
}
