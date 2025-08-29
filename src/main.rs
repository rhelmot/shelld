use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::process::Child;
use std::time::Duration;

use futures::sink::SinkExt;
use futures::stream::StreamExt;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use hyper_tungstenite::{tungstenite, HyperWebsocket};
use hyper_util::rt::TokioIo;
use tokio::time::sleep;
use tokio::sync::mpsc;
use tungstenite::Message;

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
use thiserror::Error;

static STATE: std::sync::OnceLock<std::sync::Mutex<BTreeMap<u64, Option<SessionState>>>> = std::sync::OnceLock::new();

pub enum SessionLifecycle {
    Prompt,
    Execute,
}

pub struct SessionState {
    id: u64,
    process: Child,
    status: SessionLifecycle,
    cmd_channel: mpsc::Sender<String>,
}

impl SessionState {
    pub async fn submit(&self, command: String) -> Result<(), Error> {
        self.cmd_channel.send(command).await?;
        Ok(())
    }
}

pub struct SessionStateGuard(Option<SessionState>);

impl Deref for SessionStateGuard {
    type Target = SessionState;

    fn deref(&self) -> &Self::Target {
        return self.0.as_ref().unwrap()
    }
}

impl DerefMut for SessionStateGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return self.0.as_mut().unwrap()
    }
}

impl Drop for SessionStateGuard {
    fn drop(&mut self) {
        if let Some(state) = self.0.take() {
            STATE.get_or_init(|| std::sync::Mutex::new(BTreeMap::new())).lock().unwrap().insert(state.id, Some(state));
        }
    }
}

//impl AsyncDrop for SessionStateGuard {
//    async fn drop(mut self: std::pin::Pin<&mut Self>) {
//        if let Some(state) = self.0.take() {
//            let mut map_guard = STATE.get_or_init(async || Arc::new(Mutex::new(BTreeMap::new()))).await.lock().await;
//            map_guard.insert(state.id, Some(state));
//        }
//    }
//}

#[derive(Error, Debug)]
pub enum SessionStateLookupError {
    #[error("No such session")]
    Missing,
    #[error("Session is busy")]
    Busy,
}

#[derive(Error, Debug)]
pub enum CommandSubmitError {
    #[error("No such session")]
    BadSession,
}

pub fn session_state_oneshot(session_id: u64) -> Result<SessionStateGuard, SessionStateLookupError> {
    let mut locked = STATE.get_or_init(|| std::sync::Mutex::new(BTreeMap::new())).lock().unwrap();
    let Some(state) = locked.get_mut(&session_id) else {
        return Err(SessionStateLookupError::Missing);
    };
    let Some(result) = state.take() else {
        return Err(SessionStateLookupError::Busy);
    };
    Ok(SessionStateGuard(Some(result)))
}

pub async fn session_state(session_id: u64) -> Option<SessionStateGuard> {
    loop {
        match session_state_oneshot(session_id) {
            Ok(state) => break Some(state),
            Err(SessionStateLookupError::Busy) => sleep(Duration::from_millis(5)).await,
            Err(SessionStateLookupError::Missing) => break None,
        }
    }
}

async fn submit_command(session_id: u64, command: String) -> Result<(), Error> {
    session_state(session_id).await
        .ok_or(CommandSubmitError::BadSession)?
        .submit(command).await?;
    Ok(())
}

async fn handle_request(mut request: Request<Incoming>) -> Result<Response<Full<Bytes>>, Error> {
    let headers = request.headers();
    let session_id = headers.get("Session")
        .and_then(|session| session.to_str().ok())
        .and_then(|session| session.parse::<u64>().ok());

    let command = headers.get("Command")
        .map(|s| s.as_bytes())
        .and_then(|s| String::from_utf8(s.to_vec()).ok());

    match request.uri().path() {
        "/list" => {
            todo!()
        },
        "/new" => {
            todo!()
        },
        "/prompt" => {
            todo!()
        },
        "/complete" => {
            todo!()
        },
        "/run" | "/attach" => {
            let Some(session_id) = session_id else {
                return Ok(Response::builder().status(404).body(Full::<Bytes>::from("No session"))?);
            };
            if request.uri().path() == "/run" {
                let command = command.unwrap_or("".to_string());
                submit_command(session_id, command).await?;
            }

            if hyper_tungstenite::is_upgrade_request(&request) {
                let (response, websocket) = hyper_tungstenite::upgrade(&mut request, None)?;

                // Spawn a task to handle the websocket connection.
                tokio::spawn(async move {
                    if let Err(e) = serve_websocket(websocket).await {
                        eprintln!("Error in websocket connection: {e}");
                    }
                });

                Ok(response)
            } else {
                Ok(Response::new(Full::<Bytes>::from("Ok")))
            }
        },
        "/close" => {
            todo!()
        },
        _ => {
            Ok(Response::builder().status(404).body(Full::<Bytes>::from("Not found"))?)
        },
    }

}

/// Handle a websocket connection.
async fn serve_websocket(websocket: HyperWebsocket) -> Result<(), Error> {
    let mut websocket = websocket.await?;
    while let Some(message) = websocket.next().await {
        match message? {
            Message::Text(msg) => {
                println!("Received text message: {msg}");
                websocket.send(Message::text("Thank you, come again.")).await?;
            },
            Message::Binary(msg) => {
                println!("Received binary message: {msg:02X?}");
                websocket.send(Message::binary(b"Thank you, come again.".to_vec())).await?;
            },
            Message::Ping(_msg) => {},
            Message::Pong(_msg) => {}
            Message::Close(_msg) => {},
            Message::Frame(_msg) => {
                unreachable!();
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let addr: std::net::SocketAddr = "[::1]:3000".parse()?;
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("Listening on http://{addr}");

    let mut http = hyper::server::conn::http1::Builder::new();
    http.keep_alive(true);

    loop {
        let (stream, _) = listener.accept().await?;
        let connection = http
            .serve_connection(TokioIo::new(stream), hyper::service::service_fn(handle_request))
            .with_upgrades();
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                println!("Error serving HTTP connection: {err:?}");
            }
        });
    }
}
