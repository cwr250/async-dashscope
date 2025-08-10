use crate::error::{DashScopeError, Result};

use backoff::ExponentialBackoff;
use futures_util::{SinkExt, StreamExt};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as WsMessage, handshake::client::Request},
};
use tracing::{error, info};

// Use a cloneable error type for broadcast channel
#[derive(Debug, Clone)]
pub enum WsError {
    WebSocket(String),
    #[allow(dead_code)]
    ConnectionClosed,
}

impl From<tokio_tungstenite::tungstenite::Error> for WsError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        WsError::WebSocket(e.to_string())
    }
}

pub type WsResult = std::result::Result<WsMessage, WsError>;

#[derive(Clone, Debug)]
pub struct WsPool {
    inner: Arc<PoolInner>,
}

#[derive(Debug)]
struct PoolInner {
    connections: Mutex<Vec<PooledConnection>>,
    config: PoolConfig,
}

#[derive(Clone, Debug)]
struct PoolConfig {
    request: Request,
    max_size: usize,
    backoff: ExponentialBackoff,
    write_cap: usize,
    read_broadcast_cap: usize,
    ping_interval: Option<Duration>,
}

#[derive(Debug)]
struct PooledConnection {
    conn: Connection,
    in_use: bool,
}

impl WsPool {
    pub async fn acquire(&self) -> Result<WsLease> {
        let mut connections = self.inner.connections.lock().await;

        // First, try to find an available healthy connection
        for pooled in connections.iter_mut() {
            if !pooled.in_use && pooled.conn.is_alive() {
                pooled.in_use = true;
                let conn_id = pooled.conn.conn_id;
                let write_tx = pooled.conn.write_tx.clone();
                let read_rx = pooled.conn.read_tx.subscribe();
                let pool = self.clone();

                return Ok(WsLease {
                    conn_id,
                    write_tx,
                    read_rx,
                    pool,
                });
            }
        }

        // Remove dead connections
        connections.retain(|pooled| pooled.conn.is_alive());

        // If we have room, create a new connection
        if connections.len() < self.inner.config.max_size {
            let conn = Connection::connect(
                self.inner.config.request.clone(),
                self.inner.config.backoff.clone(),
                self.inner.config.write_cap,
                self.inner.config.read_broadcast_cap,
                self.inner.config.ping_interval,
            )
            .await?;

            let conn_id = conn.conn_id;
            let write_tx = conn.write_tx.clone();
            let read_rx = conn.read_tx.subscribe();

            connections.push(PooledConnection { conn, in_use: true });

            let pool = self.clone();
            return Ok(WsLease {
                conn_id,
                write_tx,
                read_rx,
                pool,
            });
        }

        // Pool is full and no connections available
        Err(DashScopeError::WebSocketError(
            "Connection pool exhausted".into(),
        ))
    }

    async fn release_connection(&self, conn_id: usize) {
        let mut connections = self.inner.connections.lock().await;
        for pooled in connections.iter_mut() {
            if pooled.conn.conn_id == conn_id {
                pooled.in_use = false;
                break;
            }
        }
    }
}

pub struct WsLease {
    conn_id: usize,
    write_tx: mpsc::Sender<WsMessage>,
    read_rx: broadcast::Receiver<WsResult>,
    pool: WsPool,
}

impl WsLease {
    pub async fn send_text(&self, text_utf8: Vec<u8>) -> Result<()> {
        let s = String::from_utf8(text_utf8).map_err(DashScopeError::InvalidUtf8)?;
        self.write_tx
            .send(WsMessage::Text(s.into()))
            .await
            .map_err(|_| DashScopeError::WebSocketError("writer task closed".into()))
    }

    pub async fn send_binary(&self, data: Vec<u8>) -> Result<()> {
        self.write_tx
            .send(WsMessage::Binary(data.into()))
            .await
            .map_err(|_| DashScopeError::WebSocketError("writer task closed".into()))
    }

    pub fn try_send_text(&self, text_utf8: Vec<u8>) -> Result<()> {
        let s = String::from_utf8(text_utf8).map_err(DashScopeError::InvalidUtf8)?;
        self.write_tx
            .try_send(WsMessage::Text(s.into()))
            .map_err(|_| DashScopeError::WebSocketError("writer task closed or full".into()))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsResult> {
        self.read_rx.resubscribe()
    }
}

impl Drop for WsLease {
    fn drop(&mut self) {
        let pool = self.pool.clone();
        let conn_id = self.conn_id;
        tokio::spawn(async move {
            pool.release_connection(conn_id).await;
        });
    }
}

#[derive(Debug)]
struct Connection {
    conn_id: usize,
    write_tx: mpsc::Sender<WsMessage>,
    read_tx: broadcast::Sender<WsResult>,
    alive_rx: watch::Receiver<bool>,
    task_handle: tokio::task::JoinHandle<()>,
}

impl Connection {
    async fn connect(
        request: Request,
        backoff: ExponentialBackoff,
        write_cap: usize,
        read_broadcast_cap: usize,
        ping_interval: Option<Duration>,
    ) -> Result<Self> {
        let (write_tx, write_rx) = mpsc::channel(write_cap);
        let (read_tx, _) = broadcast::channel(read_broadcast_cap);
        let (alive_tx, alive_rx) = watch::channel(false);

        let task = tokio::spawn(connection_actor(
            request,
            backoff,
            write_rx,
            read_tx.clone(),
            alive_tx,
            ping_interval,
        ));

        Ok(Self {
            conn_id: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as usize)
                .unwrap_or(0),
            write_tx,
            read_tx,
            alive_rx,
            task_handle: task,
        })
    }

    fn is_alive(&self) -> bool {
        *self.alive_rx.borrow() && !self.task_handle.is_finished()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.task_handle.abort();
    }
}

async fn connection_actor(
    request: Request,
    _backoff: ExponentialBackoff,
    mut write_rx: mpsc::Receiver<WsMessage>,
    read_tx: broadcast::Sender<WsResult>,
    alive_tx: watch::Sender<bool>,
    ping_interval: Option<Duration>,
) {
    let _ = alive_tx.send(false);

    loop {
        info!("ws: connecting {}", request.uri());

        let ws_result = connect_async(request.clone()).await;

        match ws_result {
            Ok((ws, _resp)) => {
                let (mut sink, mut stream) = ws.split();
                let _ = alive_tx.send(true);
                info!("ws: connected {}", request.uri());

                let mut ping_ticker = ping_interval.map(tokio::time::interval);

                let read_fut = async {
                    while let Some(item) = stream.next().await {
                        let result = match item {
                            Ok(msg) => Ok(msg),
                            Err(e) => Err(WsError::from(e)),
                        };
                        let _ = read_tx.send(result);
                    }
                };

                let write_fut = async {
                    loop {
                        tokio::select! {
                            biased;
                            Some(msg) = write_rx.recv() => {
                                if let Err(_e) = sink.send(msg).await {
                                    break;
                                }
                            }
                            _ = async {
                                if let Some(t) = &mut ping_ticker {
                                    t.tick().await;
                                } else {
                                    futures_util::future::pending::<()>().await;
                                }
                            } => {
                                if ping_interval.is_some() {
                                    if let Err(_e) = sink.send(WsMessage::Ping(Vec::new().into())).await {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                };

                tokio::select! {
                    _ = read_fut => {
                        let _ = alive_tx.send(false);
                        error!("ws read stream ended, reconnecting...");
                    }
                    _ = write_fut => {
                        let _ = alive_tx.send(false);
                        error!("ws write stream ended, reconnecting...");
                    }
                }
            }
            Err(e) => {
                error!("ws connection failed: {e}, retrying...");
                let _ = alive_tx.send(false);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsPoolBuilder {
    request: Request,
    size: usize,
    backoff: ExponentialBackoff,
    write_cap: usize,
    read_broadcast_cap: usize,
    ping_interval: Option<Duration>,
}

impl WsPoolBuilder {
    pub fn new(request: Request, size: usize) -> Self {
        let mut backoff = ExponentialBackoff::default();
        if backoff.max_elapsed_time.is_none() {
            backoff.max_elapsed_time = Some(Duration::from_secs(120));
        }
        Self {
            request,
            size: size.max(1),
            backoff,
            write_cap: 64,
            read_broadcast_cap: 1024,
            ping_interval: Some(Duration::from_secs(30)),
        }
    }

    pub fn write_capacity(mut self, n: usize) -> Self {
        self.write_cap = n.max(16);
        self
    }

    pub fn read_broadcast_capacity(mut self, n: usize) -> Self {
        self.read_broadcast_cap = n.max(64);
        self
    }

    pub fn ping_interval(mut self, dur: Option<Duration>) -> Self {
        self.ping_interval = dur;
        self
    }

    pub fn backoff(mut self, b: ExponentialBackoff) -> Self {
        self.backoff = b;
        self
    }

    pub fn build(self) -> Result<WsPool> {
        let config = PoolConfig {
            request: self.request,
            max_size: self.size,
            backoff: self.backoff,
            write_cap: self.write_cap,
            read_broadcast_cap: self.read_broadcast_cap,
            ping_interval: self.ping_interval,
        };

        let inner = PoolInner {
            connections: Mutex::new(Vec::new()),
            config,
        };

        Ok(WsPool {
            inner: Arc::new(inner),
        })
    }
}
