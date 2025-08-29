use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::process::{Child, Command};
use std::time::Duration;

use futures::sink::SinkExt;
use futures::stream::StreamExt;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use hyper_tungstenite::{tungstenite, HyperWebsocket};
use hyper_util::rt::TokioIo;
use tokio::time::sleep;
use tokio::sync::{mpsc, broadcast};
use tungstenite::Message;

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
use thiserror::Error;

static STATE: std::sync::OnceLock<std::sync::Mutex<BTreeMap<u64, Option<SessionState>>>> = std::sync::OnceLock::new();

fn get_state() -> std::sync::MutexGuard<'static, BTreeMap<u64, Option<SessionState>>> {
    STATE.get_or_init(|| std::sync::Mutex::new(BTreeMap::new())).lock().unwrap()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SessionLifecycle {
    Detached,
    Seized,
    Prompt,
    Execute,
}

pub struct SessionState {
    id: u64,
    process: Child,
    lifecycle: SessionLifecycle,
    cmd_sender: mpsc::Sender<String>,
    screen_sender: broadcast::Sender<Vec<u8>>,
    keyboard_sender: mpsc::Sender<Vec<u8>>,
    current_terminal: String,
    current_size_x: u64,
    current_size_y: u64,
}

impl SessionState {
    pub async fn submit(&self, command: String) -> Result<(), Error> {
        self.cmd_sender.send(command).await?;
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
            get_state().insert(state.id, Some(state));
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

pub fn with_session_state_oneshot<T>(session_id: u64, func: impl FnOnce(&mut SessionState) -> T) -> Result<T, SessionStateLookupError> {
    let mut locked = get_state();
    let Some(state) = locked.get_mut(&session_id) else {
        return Err(SessionStateLookupError::Missing);
    };
    let Some(stateref) = state.as_mut() else {
        return Err(SessionStateLookupError::Busy);
    };
    Ok(func(stateref))
}

pub fn session_state_oneshot(session_id: u64) -> Result<SessionStateGuard, SessionStateLookupError> {
    let mut locked = get_state();
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

pub async fn with_session_state<T>(session_id: u64, mut func: impl FnMut(&mut SessionState) -> T) -> Option<T> {
    loop {
        match with_session_state_oneshot(session_id, &mut func) {
            Ok(r) => break Some(r),
            Err(SessionStateLookupError::Busy) => sleep(Duration::from_millis(5)).await,
            Err(SessionStateLookupError::Missing) => break None,
        }
    }
}

pub async fn session_lifecycle(session_id: u64) -> Option<SessionLifecycle> {
    with_session_state(session_id, |s| s.lifecycle).await
}

pub fn session_list() -> Vec<u64> {
    get_state().keys().copied().collect()
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

    let terminal = headers.get("Terminal")
        .map(|s| s.as_bytes())
        .and_then(|s| String::from_utf8(s.to_vec()).ok());
    let term_size_y = headers.get("Terminal-Rows")
        .and_then(|s| str::from_utf8(s.as_bytes()).ok())
        .and_then(|s| s.parse::<u64>().ok());
    let term_size_x = headers.get("Terminal-Columns")
        .and_then(|s| str::from_utf8(s.as_bytes()).ok())
        .and_then(|s| s.parse::<u64>().ok());

    match request.uri().path() {
        "/list" => {
            let response_string = session_list().into_iter().map(|x| format!("{}\n", x)).collect::<Vec<_>>().join("");
            Ok(Response::new(Full::<Bytes>::from(response_string)))
        },
        "/new" => {
            let command = command.unwrap_or("bash".to_owned());
            tokio::spawn(async move {
                Command::new(command).spawn();
            });
            Ok(Response::new(Full::<Bytes>::from("Ok")))
        },
        "/prompt" => {
            todo!()
        },
        "/complete" => {
            todo!()
        },
        "/run-command" | "/attach-command" | "/attach-shell" => {
            let Some(session_id) = session_id else {
                return Ok(Response::builder().status(405).body(Full::<Bytes>::from("No session"))?);
            };
            let Some(mut session) = session_state(session_id).await else {
                return Ok(Response::builder().status(404).body(Full::<Bytes>::from("No such session"))?);
            };
            let stop_at_prompt = match request.uri().path() {
                "/run-command" => {
                    let command = command.unwrap_or("".to_string());
                    submit_command(session_id, command).await?;
                    if session.lifecycle != SessionLifecycle::Prompt {
                        return Ok(Response::builder().status(503).body(Full::<Bytes>::from("Shell busy"))?);
                    }
                    true
                }
                "/attach-command" => {
                    if session.lifecycle != SessionLifecycle::Execute {
                        return Ok(Response::builder().status(503).body(Full::<Bytes>::from("No command running"))?);
                    }
                    true
                }
                "/attach-shell" => {
                    if session.lifecycle != SessionLifecycle::Detached {
                        return Ok(Response::builder().status(503).body(Full::<Bytes>::from("Shell busy"))?);
                    }
                    false
                }
                _ => unreachable!()
            };

            if let Some(terminal) = terminal {
                session.current_terminal = terminal;
            }
            if let Some(term_size_y) = term_size_y {
                session.current_size_y = term_size_y;
            }
            if let Some(term_size_x) = term_size_x {
                session.current_size_x = term_size_x;
            }

            if hyper_tungstenite::is_upgrade_request(&request) {
                let (response, websocket) = hyper_tungstenite::upgrade(&mut request, None)?;

                // Spawn a task to handle the websocket connection.
                tokio::spawn(async move {
                    if let Err(e) = serve_websocket(websocket, session, stop_at_prompt).await {
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
async fn serve_websocket(websocket: HyperWebsocket, session: SessionStateGuard, stop_at_prompt: bool) -> Result<(), Error> {
    let mut screen = session.screen_sender.subscribe();
    let keyboard = session.keyboard_sender.clone();
    drop(session);  // byeeeeeeee

    let mut websocket = websocket.await?;
    loop {
        tokio::select! {
            screen_data = screen.recv() => {
                let Ok(screen_data) = screen_data else { break };
                let Ok(()) = websocket.feed(Message::binary(screen_data)).await else { break };
            }
            ws_packet = websocket.next() => {
                let Some(Ok(ws_packet)) = ws_packet else { break };
                let Message::Binary(ws_packet) = ws_packet else { continue };
                keyboard.try_send(ws_packet.into()).ok();
            }
        }
    }
    websocket.close(None).await?;
    Ok(())
}

pub async fn serve() -> Result<(), Error> {
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
