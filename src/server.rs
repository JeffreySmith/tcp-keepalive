use color_eyre::eyre::{Result, eyre};
use nix::cmsg_space;
use nix::sys::socket::{ControlMessageOwned, MsgFlags, recvmsg};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::io::IoSliceMut;
use std::os::fd::RawFd;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::signal;
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
pub struct Server {
    id: u32,
    tcp_listener: TcpListener,
    unix_socket_path: String,
    active_connections: Arc<RwLock<HashMap<u64, TcpStream>>>,
    next_connection_id: Arc<RwLock<u64>>,
    shutdown_token: CancellationToken,
    active_connections_count: Arc<AtomicUsize>,
}

impl Server {
    pub fn new(
        id: u32,
        tcp_address: &str,
        unix_socket_path: String,
        handoff_target: Option<String>,
    ) -> Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_port(true)?;
        socket.set_reuse_address(true)?;
        let address: std::net::SocketAddr = tcp_address.parse()?;
        socket.bind(&address.into())?;
        socket.listen(128)?;
        socket.set_nonblocking(true)?;
        let std_listener: std::net::TcpListener = socket.into();
        let tcp_listener = TcpListener::from_std(std_listener)?;

        if let Some(ref target) = handoff_target {
            println!("[Server {id}] Will hand off connections to {target} on shutdown")
        }

        let unix_socket_path = format!(
            "{unix_socket_path}/socket-forward_{}_{}.sock",
            address.port(),
            id
        );

        Ok(Server {
            id,
            tcp_listener,
            unix_socket_path,
            active_connections: Arc::new(RwLock::new(HashMap::new())),
            next_connection_id: Arc::new(RwLock::new(0)),
            shutdown_token: CancellationToken::new(),
            active_connections_count: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        let server_clone = Arc::clone(&self);
        tokio::spawn(async move {
            if let Err(e) = server_clone.unix_socket_listener().await {
                eprintln!("[Server {}] Unix socket error: {}", server_clone.id, e);
            }
        });

        let server_clone = Arc::clone(&self);
        tokio::spawn(async move {
            server_clone.wait_for_shutdown().await;
        });

        loop {
            let cancel_token = self.shutdown_token.clone();
            tokio::select! {
                result = self.tcp_listener.accept() => {
                    match result {
                        Ok((tcp_stream, _)) => {
                            println!("[Server {}] New client connection from {:?}", self.id, tcp_stream.peer_addr());
                            let id = self.id;
                            let connections = Arc::clone(&self.active_connections);
                            let shutdown_token = self.shutdown_token.child_token();

                            let conn_id = {
                                let mut id = self.next_connection_id.write().await;
                                let current = *id;
                                *id += 1;
                                current
                            };
                            let value = self.clone();
                            println!("[Server {}] Spawning handler for connection {}", self.id, conn_id);
                            tokio::spawn(async move {
                                Self::handle_client(id, conn_id, tcp_stream, connections, shutdown_token, &value.active_connections_count).await;
                            });
                        }
                        Err(e) => {
                            eprintln!("[Server {}] TCP accept error: {}", self.id, e);
                        }
                    }
                }
                _ = cancel_token.cancelled() => {
                    println!("[Server {}] Main loop received shutdown", self.id);
                    println!("[Server {}] Waiting for handlers to save...", self.id);

                    let start = tokio::time::Instant::now();
                    let timeout = Duration::from_secs(30);


                    let expected_handles = self.active_connections_count.load(Ordering::SeqCst);
                    loop {
                        let saved = {
                            let conns = self.active_connections.read().await;
                            conns.len()
                        };
                        if saved >= expected_handles || start.elapsed() > timeout {
                            println!("[Server {}] {} out of {} connections saved", self.id, saved, expected_handles);
                            break;
                        }
                    }

                    self.handoff_all_connections().await;

                    println!("[Server {}] Exiting!", self.id);
                    return Ok(())
                }
            }
        }
    }

    fn discover_peers(&self) -> Vec<String> {
        let tcp_port = self
            .tcp_listener
            .local_addr()
            .ok()
            .and_then(|a| a.port().into())
            .unwrap_or(0);
        let pattern = format!("socket-forward_{}_", tcp_port);
        let my_socket = format!("socket-forward_{tcp_port}_{}.sock", self.id);

        let mut peers = Vec::new();

        if let Ok(entire_dir) = std::fs::read_dir("/tmp") {
            for entry in entire_dir.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && name.starts_with(&pattern)
                    && name.ends_with(".sock")
                    && name != my_socket
                {
                    peers.push(format!("/tmp/{}", name));
                }
            }
        }
        peers
    }

    async fn receive_tcp_connection(unix_stream: UnixStream) -> Result<TcpStream> {
        let std_unix_stream = unix_stream.into_std()?;
        let (tcp_stream, std_unix_stream, received_fd) = tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 1024];
            let mut iov = [IoSliceMut::new(&mut buf)];
            let mut cmsg_buffer = cmsg_space!([RawFd; 1]);

            let msg = recvmsg::<()>(
                std_unix_stream.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_buffer),
                MsgFlags::empty(),
            )?;

            let cmsgs = msg.cmsgs();
            while let Some(cmsg) = cmsgs?.next() {
                if let ControlMessageOwned::ScmRights(fds) = cmsg
                    && let Some(&fd) = fds.first()
                {
                    let std_stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
                    std_stream.set_nonblocking(true)?;
                    let tcp_stream = TcpStream::from_std(std_stream)?;
                    return Ok::<
                        (TcpStream, std::os::unix::net::UnixStream, RawFd),
                        color_eyre::eyre::Error,
                    >((tcp_stream, std_unix_stream, fd));
                }
            }
            Err(eyre!("No file descriptor received in control message"))
        })
        .await??;

        println!("Received FD {} via SCM_RIGHTS", received_fd);

        std_unix_stream.set_nonblocking(true)?;
        let mut unix_stream = UnixStream::from_std(std_unix_stream)?;

        println!("Sending ACK for FD {}", received_fd);
        unix_stream.write_all(b"ACK").await?;

        println!(
            "FD {} fully received and ACK sent, peer={:?}",
            received_fd,
            tcp_stream.peer_addr()
        );

        Ok(tcp_stream)
    }

    async fn handoff_all_connections(&self) {
        let mut connections = self.active_connections.write().await;
        let count = connections.len();

        if count == 0 {
            println!("[Server {}] No active connections to hand off", self.id);
            return;
        }

        let peers = self.discover_peers();

        if peers.is_empty() {
            println!("[Server {}] No peer servers found!", self.id);
            return;
        }

        println!("[Server {}] Found {} peer(s):", self.id, peers.len());
        for peer in &peers {
            println!("[Server {}]   - {}", self.id, peer);
        }

        println!("[Server {}] Handing off {} connection(s)", self.id, count);

        let mut peer_idx = 0;
        let mut handed_off = Vec::new();

        for (conn_id, stream) in connections.iter() {
            if let Ok(peer_addr) = stream.peer_addr() {
                let fd = stream.as_raw_fd();
                let target = &peers[peer_idx % peers.len()];

                println!(
                    "[Server {}] Conn {} (FD={}, peer={}) → {}",
                    self.id, conn_id, fd, peer_addr, target
                );

                match Self::handoff_connection_internal(stream, target).await {
                    Ok(_) => {
                        println!(
                            "[Server {}] ACK received for conn {} (FD={})",
                            self.id, conn_id, fd
                        );
                        handed_off.push(*conn_id);
                        peer_idx += 1;
                        sleep(Duration::from_millis(200)).await;
                    }
                    Err(e) => {
                        eprintln!("[Server {}] Failed conn {}: {}", self.id, conn_id, e);
                    }
                }
            }
        }

        println!(
            "[Server {}] Forgetting {} stream(s)",
            self.id,
            handed_off.len()
        );
        for conn_id in handed_off {
            if let Some(stream) = connections.remove(&conn_id) {
                let fd = stream.as_raw_fd();
                println!(
                    "[Server {}] Forgetting FD {} (conn {})",
                    self.id, fd, conn_id
                );
                std::mem::forget(stream);
                println!("[Server {}] FD {} forgotten", self.id, fd);
            }
        }

        drop(connections);
        println!("[Server {}] Handoff complete!", self.id);
        std::fs::remove_file(&self.unix_socket_path).ok();
    }

    async fn handoff_connection_internal(
        tcp_stream: &TcpStream,
        target_unix_socket: &str,
    ) -> std::io::Result<()> {
        use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};
        use std::io::IoSlice;

        let mut unix_stream = UnixStream::connect(target_unix_socket).await?;
        let fd = tcp_stream.as_raw_fd();

        let fds = [fd];
        let cmsg = ControlMessage::ScmRights(&fds);
        let iov = [IoSlice::new(b"HANDOFF")];

        sendmsg::<()>(
            unix_stream.as_raw_fd(),
            &iov,
            &[cmsg],
            MsgFlags::empty(),
            None,
        )
        .map_err(std::io::Error::other)?;

        let mut ack = [0u8; 3];
        unix_stream.read_exact(&mut ack).await?;

        if &ack != b"ACK" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid acknowledgment from peer server",
            ));
        }

        Ok(())
    }

    async fn handle_client(
        id: u32,
        conn_id: u64,
        mut stream: TcpStream,
        active_connections: Arc<RwLock<HashMap<u64, TcpStream>>>,
        shutdown_token: CancellationToken,
        active_connections_count: &Arc<AtomicUsize>,
    ) {
        println!("[Server {}] Handler {} started", id, conn_id);
        active_connections_count.fetch_add(1, Ordering::SeqCst);

        loop {
            tokio::select! {
                _ = shutdown_token.cancelled() => {
                    println!("[Server {}] Handler {} got shutdown!", id, conn_id);
                    println!("[Server {}] Saving connection {}", id, conn_id);
                    active_connections.write().await.insert(conn_id, stream);
                    println!("[Server {}] Connection {} saved", id, conn_id);
                    break;
                }
                result = async {
                    let message = format!(
                        "[Server {}] - {} - Hello from server!\n",
                        id,
                        chrono::Local::now().format("%H:%M:%S")
                    );
                    stream.write_all(message.as_bytes()).await
                } => {
                    match result {
                        Ok(_) => {
                            sleep(Duration::from_secs(1)).await;
                        }
                        Err(e) => {
                            println!("[Server {}] Client {} disconnected: {}", id, conn_id, e);
                            active_connections_count.fetch_sub(1, Ordering::SeqCst);
                            return;
                        }
                    }
                }
            }
        }
    }

    async fn unix_socket_listener(&self) -> Result<()> {
        let _ = std::fs::remove_file(&self.unix_socket_path);
        let unix_socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        let addr = socket2::SockAddr::unix(&self.unix_socket_path)?;

        if let Err(e) = unix_socket.bind(&addr) {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                println!(
                    "[Server {}] Unix socket {} already exists, removing and retrying...",
                    self.id, self.unix_socket_path
                );
                let _ = std::fs::remove_file(&self.unix_socket_path);
                unix_socket.bind(&addr)?;
            } else {
                return Err(e.into());
            }
        }
        unix_socket.listen(128)?;
        unix_socket.set_nonblocking(true)?;

        let std_listener: std::os::unix::net::UnixListener = unix_socket.into();
        let listener = UnixListener::from_std(std_listener)?;

        println!(
            "[Server {}] Unix socket listening on {}",
            self.id, self.unix_socket_path
        );

        loop {
            let cancel_token = self.shutdown_token.clone();
            let value = self.active_connections_count.clone();
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((unix_stream, _)) => {
                            println!("[Server {}] Receiving handed off connection...", self.id);
                            match Self::receive_tcp_connection(unix_stream).await {
                                Ok(tcp_stream) => {
                                    println!("[Server {}] Hand-off successful!", self.id);
                                    let id = self.id;
                                    let connections = Arc::clone(&self.active_connections);
                                    let shutdown_token = self.shutdown_token.child_token();

                                    let conn_id = {
                                        let mut id = self.next_connection_id.write().await;
                                        let current = *id;
                                        *id += 1;
                                        current
                                    };

                                    tokio::spawn(async move {
                                        Self::handle_client(id, conn_id, tcp_stream, connections, shutdown_token, &value).await;
                                    });
                                }
                                Err(e) => {
                                    eprintln!("[Server {}] Failed to receive connection: {}", self.id, e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[Server {}] Unix socket accept error: {}", self.id, e);
                        }
                    }
                }
                _ = cancel_token.cancelled() => {
                    println!("[Server {}] Unix listener shutting down", self.id);
                    break;
                }
            }
        }
        Ok(())
    }

    async fn wait_for_shutdown(&self) {
        let ctrl_c = async {
            signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        };

        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to listen for SIGTERM")
                .recv()
                .await;
        };

        tokio::select! {
            _ = ctrl_c => {
                println!("[Server {}] Received Ctrl+C", self.id);
            }
            _ = terminate => {
                println!("[Server {}] Received SIGTERM", self.id);
            }
        }

        println!("[Server {}] Cancelling all handlers", self.id);
        self.shutdown_token.cancel();
    }
}
