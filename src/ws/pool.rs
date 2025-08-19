use crate::error::{DashScopeError, Result};

use backoff::{ExponentialBackoff, backoff::Backoff};
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, broadcast, mpsc, watch};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as WsMessage, handshake::client::Request},
};
use tracing::{error, info};

static CONNECTION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Pool status for observability
#[derive(Debug, Clone)]
pub struct PoolStatus {
    pub total: usize,
    pub available: usize,
    pub in_use: usize,
    pub reconnecting: usize,
    pub max_size: usize,
}

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

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsError::WebSocket(msg) => write!(f, "WebSocket error: {}", msg),
            WsError::ConnectionClosed => write!(f, "WebSocket connection closed"),
        }
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
    initial_size: usize,
    connect_timeout: Duration,
    message_timeout: Duration,
    /// Notification for when connections become alive
    alive_notify: Arc<Notify>,
}

#[derive(Debug)]
struct PooledConnection {
    conn: Connection,
    in_use: bool,
}

impl WsPool {
    /// Warm up the pool by creating the specified number of connections
    pub async fn warm_up(&self, target: usize) -> Result<()> {
        let target = target.min(self.inner.config.max_size);

        let mut remaining = target;
        let mut attempt = 0;
        let max_retries = 3;
        let mut last_error: Option<String> = None;

        while remaining > 0 && attempt < max_retries {
            // Check existing healthy connections count without holding lock during await
            let existing = {
                let g = self.inner.connections.lock().await;
                g.iter().filter(|p| p.conn.is_alive()).count()
            };
            let need = remaining.saturating_sub(existing);
            if need == 0 {
                tracing::info!("ws warmup completed: target {} reached", target);
                return Ok(());
            }

            if attempt > 0 {
                tracing::info!(
                    "ws warmup retry attempt {}/{}: need {} more connections",
                    attempt + 1,
                    max_retries,
                    need
                );
            }

            // Create connections concurrently to avoid holding lock during await
            let mut tasks = Vec::with_capacity(need);
            for _ in 0..need {
                let cfg = self.inner.config.clone();
                tasks.push(tokio::spawn({
                    let alive_notify = self.inner.config.alive_notify.clone();
                    async move {
                        Connection::connect(
                            cfg.request.clone(),
                            cfg.backoff.clone(),
                            cfg.write_cap,
                            cfg.read_broadcast_cap,
                            cfg.ping_interval,
                            cfg.connect_timeout,
                            cfg.message_timeout,
                            alive_notify,
                        )
                        .await
                    }
                }));
            }

            let mut new_conns = Vec::with_capacity(need);
            let mut errors = 0usize;
            for t in tasks {
                match t.await {
                    Ok(Ok(conn)) => new_conns.push(conn),
                    Ok(Err(e)) => {
                        errors += 1;
                        last_error = Some(format!("connection error: {}", e));
                    }
                    Err(e) => {
                        errors += 1;
                        last_error = Some(format!("task join error: {}", e));
                    }
                }
            }

            let successful = new_conns.len();
            let mut g = self.inner.connections.lock().await;
            for conn in new_conns {
                g.push(PooledConnection {
                    conn,
                    in_use: false,
                });
            }

            remaining = remaining.saturating_sub(successful);

            if successful > 0 {
                tracing::info!(
                    "ws warmup attempt {}: {} successful, {} failed, {} remaining",
                    attempt + 1,
                    successful,
                    errors,
                    remaining
                );
            }

            if remaining == 0 {
                tracing::info!("ws warmup completed successfully: {} connections", target);
                return Ok(());
            }

            attempt += 1;

            // If not the last attempt and we had failures, wait before retry
            if attempt < max_retries && errors > 0 {
                let backoff_duration = Duration::from_millis(100 * (1 << attempt.min(4))); // Exponential backoff: 200ms, 400ms, 800ms
                tracing::debug!(
                    "ws warmup backing off for {:?} before retry",
                    backoff_duration
                );
                tokio::time::sleep(backoff_duration).await;
            }
        }

        // Final check after all retries
        let final_check = {
            let g = self.inner.connections.lock().await;
            g.iter().filter(|p| p.conn.is_alive()).count()
        };

        if final_check == 0 {
            return Err(DashScopeError::WebSocketError(
                "warmup failed after retries: zero live connections".into(),
            ));
        }

        if final_check < target {
            let error_detail = last_error.unwrap_or_else(|| "unknown error".to_string());
            tracing::warn!(
                "ws warmup partial success after {} attempts: {} successful out of {} target. Last error: {}",
                attempt,
                final_check,
                target,
                error_detail
            );
        }

        Ok(())
    }

    /// Get current pool status for observability
    pub async fn status(&self) -> PoolStatus {
        let connections = self.inner.connections.lock().await;

        // Filter out finished connections for accurate statistics
        let active_connections: Vec<_> = connections
            .iter()
            .filter(|p| !p.conn.task_handle.is_finished())
            .collect();

        let total = active_connections.len();
        let in_use = active_connections.iter().filter(|p| p.in_use).count();
        let alive = active_connections
            .iter()
            .filter(|p| p.conn.is_alive())
            .count();
        let available = active_connections
            .iter()
            .filter(|p| !p.in_use && p.conn.is_alive())
            .count();
        // Reconnecting = active connections that are not alive
        let reconnecting = total - alive;

        PoolStatus {
            total,
            available,
            in_use,
            reconnecting,
            max_size: self.inner.config.max_size,
        }
    }

    /// Close all connections in the pool
    pub async fn close_all(&self) {
        let mut g = self.inner.connections.lock().await;
        g.clear(); // Connection Drop will abort connection actors
    }

    pub async fn acquire(&self) -> Result<WsLease> {
        // Phase 1: Fast path - find available healthy connection under short lock
        {
            let mut connections = self.inner.connections.lock().await;

            // Try to find an available healthy connection
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

            // Clean up finished connections to avoid leaks
            connections.retain(|pooled| !pooled.conn.task_handle.is_finished());

            // Check if we can create a new connection (decide under lock, create outside)
            if connections.len() >= self.inner.config.max_size {
                // Check if we have reconnecting connections - if so, wait briefly
                let reconnecting_count = connections
                    .iter()
                    .filter(|p| !p.conn.is_alive() && !p.conn.task_handle.is_finished())
                    .count();

                if reconnecting_count > 0 {
                    tracing::debug!(
                        "Pool full but {} connections reconnecting, waiting briefly...",
                        reconnecting_count
                    );
                    // Drop lock before waiting
                    drop(connections);

                    // Wait for up to 200ms for a connection to become available
                    let wait_result = tokio::time::timeout(
                        Duration::from_millis(200),
                        self.inner.config.alive_notify.notified(),
                    )
                    .await;

                    if wait_result.is_ok() {
                        // A connection became alive, retry fast path
                        let mut connections = self.inner.connections.lock().await;
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
                    }
                }

                return Err(DashScopeError::WebSocketError(
                    "Connection pool exhausted".into(),
                ));
            }
        }

        // Phase 2: Create connection outside lock (this can take seconds)
        let conn = Connection::connect(
            self.inner.config.request.clone(),
            self.inner.config.backoff.clone(),
            self.inner.config.write_cap,
            self.inner.config.read_broadcast_cap,
            self.inner.config.ping_interval,
            self.inner.config.connect_timeout,
            self.inner.config.message_timeout,
            self.inner.config.alive_notify.clone(),
        )
        .await?;

        let conn_id = conn.conn_id;
        let write_tx = conn.write_tx.clone();
        let read_rx = conn.read_tx.subscribe();

        // Phase 3: Insert back into pool under short lock
        {
            let mut connections = self.inner.connections.lock().await;

            // Race condition check: pool might be full now
            if connections.len() >= self.inner.config.max_size {
                // Drop the connection (will terminate actor) and return error
                drop(conn);
                return Err(DashScopeError::WebSocketError(
                    "Connection pool exhausted".into(),
                ));
            }

            connections.push(PooledConnection { conn, in_use: true });
        }

        let pool = self.clone();
        Ok(WsLease {
            conn_id,
            write_tx,
            read_rx,
            pool,
        })
    }

    /// Acquire a connection with retry logic for transient failures
    ///
    /// This method retries the acquire operation for up to the specified duration
    /// when encountering "Connection pool exhausted" errors, which can happen
    /// when all connections are temporarily reconnecting.
    pub async fn acquire_with_timeout(&self, timeout: Duration) -> Result<WsLease> {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            match self.acquire().await {
                Ok(lease) => return Ok(lease),
                Err(e) => {
                    if tokio::time::Instant::now() < deadline {
                        // Check if this is a "pool exhausted" error worth retrying
                        if let DashScopeError::WebSocketError(msg) = &e {
                            if msg.contains("Connection pool exhausted") {
                                tracing::debug!("Pool exhausted, retrying in 20ms...");
                                tokio::time::sleep(Duration::from_millis(20)).await;
                                continue;
                            }
                        }
                        // For other errors, return immediately
                        return Err(e);
                    } else {
                        // Timeout reached
                        return Err(e);
                    }
                }
            }
        }
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

    /// 发送文本消息 (接受字符串切片) - 推荐使用此接口
    pub async fn send_text_str(&self, s: &str) -> Result<()> {
        self.write_tx
            .send(WsMessage::Text(s.to_owned().into()))
            .await
            .map_err(|_| DashScopeError::WebSocketError("writer task closed".into()))
    }

    /// 尝试发送文本消息 (接受字符串切片) - 推荐使用此接口
    pub fn try_send_text_str(&self, s: &str) -> Result<()> {
        self.write_tx
            .try_send(WsMessage::Text(s.to_owned().into()))
            .map_err(|_| DashScopeError::WebSocketError("writer task closed or full".into()))
    }

    /// 发送文本消息 (接受拥有的字符串)
    pub async fn send_text_string(&self, s: String) -> Result<()> {
        self.write_tx
            .send(WsMessage::Text(s.into()))
            .await
            .map_err(|_| DashScopeError::WebSocketError("writer task closed".into()))
    }

    /// 尝试发送文本消息 (接受拥有的字符串)
    pub fn try_send_text_string(&self, s: String) -> Result<()> {
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
        connect_timeout: Duration,
        message_timeout: Duration,
        alive_notify: Arc<Notify>,
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
            connect_timeout,
            message_timeout,
            alive_notify,
        ));

        Ok(Self {
            conn_id: CONNECTION_ID_COUNTER.fetch_add(1, Ordering::Relaxed) as usize,
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
    mut backoff: ExponentialBackoff,
    mut write_rx: mpsc::Receiver<WsMessage>,
    read_tx: broadcast::Sender<WsResult>,
    alive_tx: watch::Sender<bool>,
    ping_interval: Option<Duration>,
    connect_timeout: Duration,
    message_timeout: Duration,
    alive_notify: Arc<Notify>,
) {
    let _ = alive_tx.send(false);

    loop {
        info!("ws: connecting {}", request.uri());

        let ws_result = tokio::time::timeout(connect_timeout, connect_async(request.clone())).await;

        match ws_result {
            Ok(Ok((ws, _resp))) => {
                backoff.reset();
                let (mut sink, mut stream) = ws.split();
                let _ = alive_tx.send(true);
                alive_notify.notify_waiters(); // Notify waiters that connection is alive
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
                                if tokio::time::timeout(
                                    message_timeout,
                                    sink.send(msg)
                                ).await.is_err() {
                                    tracing::warn!("ws message send timeout after {:?} to {}, closing connection", message_timeout, request.uri());
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
                                    if tokio::time::timeout(
                                        message_timeout,
                                        sink.send(WsMessage::Ping(Vec::new().into()))
                                    ).await.is_err() {
                                        tracing::warn!("ws ping send timeout after {:?} to {}, closing connection", message_timeout, request.uri());
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
            Ok(Err(e)) => {
                if let Some(sleep) = backoff.next_backoff() {
                    error!(
                        "ws connection failed to {}: {}, retrying after {:?}...",
                        request.uri(),
                        e,
                        sleep
                    );
                    let _ = alive_tx.send(false);
                    tokio::time::sleep(sleep).await;
                } else {
                    error!(
                        "ws connection failed to {}: {}, max elapsed time exceeded, stopping retries",
                        request.uri(),
                        e
                    );
                    let _ = alive_tx.send(false);
                    break;
                }
            }
            Err(_) => {
                if let Some(sleep) = backoff.next_backoff() {
                    error!(
                        "ws connection timeout to {} after {:?}, retrying after {:?}...",
                        request.uri(),
                        connect_timeout,
                        sleep
                    );
                    let _ = alive_tx.send(false);
                    tokio::time::sleep(sleep).await;
                } else {
                    error!(
                        "ws connection timeout to {} after {:?}, max elapsed time exceeded, stopping retries",
                        request.uri(),
                        connect_timeout
                    );
                    let _ = alive_tx.send(false);
                    break;
                }
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
    initial_size: usize,
    connect_timeout: Duration,
    message_timeout: Duration,
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
            initial_size: 0,
            connect_timeout: Duration::from_secs(15),
            message_timeout: Duration::from_secs(5),
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

    pub fn initial_size(mut self, n: usize) -> Self {
        self.initial_size = n;
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn message_timeout(mut self, timeout: Duration) -> Self {
        self.message_timeout = timeout;
        self
    }

    pub async fn build(self) -> Result<WsPool> {
        let config = PoolConfig {
            request: self.request,
            max_size: self.size,
            backoff: self.backoff,
            write_cap: self.write_cap,
            read_broadcast_cap: self.read_broadcast_cap,
            ping_interval: self.ping_interval,
            initial_size: self.initial_size,
            connect_timeout: self.connect_timeout,
            message_timeout: self.message_timeout,
            alive_notify: Arc::new(Notify::new()),
        };

        let inner = PoolInner {
            connections: Mutex::new(Vec::new()),
            config,
        };

        let pool = WsPool {
            inner: Arc::new(inner),
        };

        if pool.inner.config.initial_size > 0 {
            pool.warm_up(pool.inner.config.initial_size).await?;
        }

        Ok(pool)
    }
}
