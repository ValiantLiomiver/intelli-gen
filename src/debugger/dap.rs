//! Content-Length DAP framing. One reader owns partial frames, so cancelling a
//! request never discards bytes already read from the TCP stream.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};

use super::DebugError;

const MAX_BODY: usize = 16 * 1024 * 1024;
const MAX_PENDING: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DapMessage {
    pub seq: u64,
    #[serde(flatten)] pub message: DapPayload,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DapPayload {
    Request { command: String, #[serde(default)] arguments: Value },
    Response { request_seq: u64, success: bool, command: String,
        #[serde(default)] message: Option<String>, #[serde(default)] body: Value },
    Event { event: String, #[serde(default)] body: Value },
}
#[derive(Clone, Debug)]
pub struct DapEvent { pub event: String, pub body: Value }
struct Request {
    command: String,
    arguments: Value,
    timeout: Duration,
    reply: oneshot::Sender<Result<Value, DebugError>>,
}
struct Pending {
    command: String,
    deadline: Instant,
    reply: oneshot::Sender<Result<Value, DebugError>>,
}

#[derive(Clone)]
pub struct DapClient { requests: mpsc::Sender<Request> }
impl DapClient {
    pub async fn request(&self, command: &str, arguments: Value) -> Result<Value, DebugError> {
        self.request_timeout(command, arguments, Duration::from_secs(20)).await
    }
    pub async fn request_timeout(&self, command: &str, arguments: Value, timeout: Duration) -> Result<Value, DebugError> {
        self.start_request(command, arguments, timeout)?.wait().await
    }
    /// Enqueue immediately, even if the response future is not polled yet. DAP
    /// launch must reach the transport before configuration requests are sent.
    pub fn start_request(&self, command: &str, arguments: Value, timeout: Duration) -> Result<DapResponse, DebugError> {
        let (reply, response) = oneshot::channel();
        self.requests.try_send(Request { command: command.into(), arguments, timeout, reply })
            .map_err(|error| DebugError::Transport(format!("Debug request queue unavailable: {error}")))?;
        Ok(DapResponse { response, command: command.into(), timeout })
    }
}
pub struct DapResponse {
    response: oneshot::Receiver<Result<Value, DebugError>>,
    command: String,
    timeout: Duration,
}
impl DapResponse {
    pub async fn wait(self) -> Result<Value, DebugError> {
        tokio::time::timeout(self.timeout + Duration::from_secs(4), self.response).await
            .map_err(|_| DebugError::Transport(format!("{} response timed out", self.command)))?
            .map_err(|_| DebugError::Transport(format!("DAP session ended during {}", self.command)))?
    }
}

pub struct DapConnection {
    client: DapClient,
    events: mpsc::Receiver<Result<DapEvent, DebugError>>,
    shutdown: watch::Sender<bool>,
    worker: Option<tokio::task::JoinHandle<()>>,
}
impl DapConnection {
    /// The port supplied by the current JDT LS session is always contacted on
    /// IPv4 loopback; adapter output can never redirect the client to a host.
    pub async fn connect(port: u16) -> Result<Self, DebugError> {
        if port == 0 { return Err(DebugError::Transport("JDT LS returned port zero".into())); }
        let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))).await
            .map_err(|_| DebugError::Transport("Connecting to Java debug server timed out".into()))??;
        stream.set_nodelay(true)?;
        let (requests, receiver) = mpsc::channel(64);
        // Session startup pumps this bounded event queue while awaiting every
        // request; cancellation also interrupts a backpressured event send.
        let (events, event_receiver) = mpsc::channel(256);
        let (shutdown, cancellation) = watch::channel(false);
        let worker = tokio::spawn(drive(stream, receiver, events, cancellation));
        Ok(Self { client: DapClient { requests }, events: event_receiver, shutdown, worker: Some(worker) })
    }
    pub fn client(&self) -> DapClient { self.client.clone() }
    pub async fn next_event(&mut self) -> Option<Result<DapEvent, DebugError>> { self.events.recv().await }
    pub async fn close(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(worker) = self.worker.take() { let _ = worker.await; }
    }
}
impl Drop for DapConnection {
    fn drop(&mut self) { let _ = self.shutdown.send(true); }
}

async fn write_message(writer: &mut (impl AsyncWrite + Unpin), message: DapMessage) -> Result<(), DebugError> {
    let body = serde_json::to_vec(&message).map_err(|error| DebugError::Transport(format!("Cannot encode DAP message: {error}")))?;
    if body.len() > MAX_BODY { return Err(DebugError::Transport("Outgoing DAP message exceeds 16 MiB".into())); }
    tokio::time::timeout(Duration::from_secs(3), async {
        writer.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
        writer.write_all(&body).await?;
        writer.flush().await
    }).await.map_err(|_| DebugError::Transport("DAP write timed out".into()))??;
    Ok(())
}
async fn read_message(reader: &mut (impl AsyncRead + Unpin)) -> Result<DapMessage, DebugError> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= 8192 { return Err(DebugError::Transport("DAP header exceeds 8 KiB".into())); }
        header.push(reader.read_u8().await?);
    }
    let header = std::str::from_utf8(&header).map_err(|_| DebugError::Transport("DAP header is not ASCII/UTF-8".into()))?;
    let mut length = None;
    for line in header.split("\r\n").filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(|| DebugError::Transport("Malformed DAP header".into()))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() { return Err(DebugError::Transport("Duplicate DAP Content-Length".into())); }
            length = Some(value.trim().parse::<usize>().map_err(|_| DebugError::Transport("Invalid DAP Content-Length".into()))?);
        }
    }
    let length = length.filter(|length| *length > 0 && *length <= MAX_BODY)
        .ok_or_else(|| DebugError::Transport("Missing or oversized DAP Content-Length".into()))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(|error| DebugError::Transport(format!("Malformed DAP JSON: {error}")))
}

async fn drive(stream: TcpStream, mut requests: mpsc::Receiver<Request>,
    events: mpsc::Sender<Result<DapEvent, DebugError>>, mut cancellation: watch::Receiver<bool>) {
    let (mut reader, mut writer) = stream.into_split();
    let (incoming, mut messages) = mpsc::channel(64);
    let reader_task = tokio::spawn(async move {
        loop {
            let message = read_message(&mut reader).await;
            let failed = message.is_err();
            if incoming.send(message).await.is_err() || failed { break; }
        }
    });
    let mut sequence = 1u64;
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let failure = 'connection: loop {
        if *cancellation.borrow() { break None; }
        tokio::select! {
            _ = cancellation.changed() => break None,
            request = requests.recv() => {
                let Some(request) = request else { break None; };
                if request.reply.is_closed() { continue; }
                if pending.len() >= MAX_PENDING {
                    let _ = request.reply.send(Err(DebugError::Transport("Too many DAP requests pending".into()))); continue;
                }
                let seq = sequence; sequence += 1;
                let message = DapMessage { seq, message: DapPayload::Request { command: request.command.clone(), arguments: request.arguments } };
                pending.insert(seq, Pending { command: request.command, deadline: Instant::now() + request.timeout, reply: request.reply });
                if let Err(error) = write_message(&mut writer, message).await { break Some(error); }
            }
            message = messages.recv() => {
                let message = match message {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => break Some(error),
                    None => break Some(DebugError::Transport("DAP reader stopped".into())),
                };
                match message.message {
                    DapPayload::Response { request_seq, command, success, message, body } => {
                        if let Some(request) = pending.remove(&request_seq) {
                            if command != request.command {
                                let error = DebugError::Transport(format!("DAP response command mismatch for request {request_seq}"));
                                let _ = request.reply.send(Err(error.clone())); break Some(error);
                            }
                            let result = if success { Ok(body) } else {
                                let detail = message.or_else(|| body["error"]["format"].as_str().map(str::to_owned))
                                    .unwrap_or_else(|| body.to_string());
                                Err(DebugError::Adapter { command, message: detail })
                            };
                            let _ = request.reply.send(result);
                        }
                        // Expired/cancelled requests can receive late replies.
                    }
                    DapPayload::Event { event, body } => {
                        let event = Ok(DapEvent { event, body });
                        tokio::select! {
                            _ = cancellation.changed() => break 'connection None,
                            result = events.send(event) => if result.is_err() { break 'connection None; },
                        }
                    }
                    DapPayload::Request { command, .. } => {
                        // internalConsole launch needs no client-owned terminal.
                        // Report unsupported reverse requests; never leave the adapter waiting.
                        let response = DapMessage { seq: sequence, message: DapPayload::Response { request_seq: message.seq,
                            success: false, command, message: Some("Client reverse requests are unsupported; use internalConsole".into()), body: json!({}) } };
                        sequence += 1;
                        if let Err(error) = write_message(&mut writer, response).await { break Some(error); }
                    }
                }
            }
            _ = tick.tick() => {
                let expired: Vec<_> = pending.iter().filter(|(_, request)| request.reply.is_closed() || Instant::now() >= request.deadline).map(|(seq, _)| *seq).collect();
                for seq in expired {
                    if let Some(request) = pending.remove(&seq) {
                        let _ = request.reply.send(Err(DebugError::Transport(format!("{} request timed out or was cancelled", request.command))));
                    }
                }
            }
        }
    };
    let error = failure.clone().unwrap_or(DebugError::Cancelled);
    for (_, request) in pending { let _ = request.reply.send(Err(error.clone())); }
    requests.close();
    while let Ok(request) = requests.try_recv() { let _ = request.reply.send(Err(error.clone())); }
    let _ = writer.shutdown().await;
    reader_task.abort(); let _ = reader_task.await;
    if let Some(error) = failure { let _ = events.try_send(Err(error)); }
}
