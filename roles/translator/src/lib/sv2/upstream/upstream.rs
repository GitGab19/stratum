use crate::{
    error::TproxyError,
    status::{handle_error, Status, StatusSender},
    sv2::upstream::{channel::UpstreamChannelState, data::UpstreamData},
    task_manager::TaskManager,
    utils::{message_from_frame, ShutdownMessage},
};
use async_channel::{Receiver, Sender};
use codec_sv2::{HandshakeRole, Initiator, StandardEitherFrame, StandardSv2Frame};
use key_utils::Secp256k1PublicKey;
use network_helpers_sv2::noise_connection::Connection;
use roles_logic_sv2::{
    common_messages_sv2::{Protocol, SetupConnection},
    handlers::common::ParseCommonMessagesFromUpstream,
    parsers::AnyMessage,
    utils::Mutex,
};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::{sleep, Duration},
};
use tracing::{debug, error, info, warn};

/// Type alias for SV2 messages with static lifetime
pub type Message = AnyMessage<'static>;
/// Type alias for standard SV2 frames
pub type StdFrame = StandardSv2Frame<Message>;
/// Type alias for either handshake or SV2 frames
pub type EitherFrame = StandardEitherFrame<Message>;

/// Manages the upstream SV2 connection to a mining pool or proxy.
///
/// This struct handles the SV2 protocol communication with upstream servers,
/// including:
/// - Connection establishment with multiple upstream fallbacks
/// - SV2 handshake and setup procedures
/// - Message routing between channel manager and upstream
/// - Connection monitoring and error handling
/// - Graceful shutdown coordination
///
/// The upstream connection supports automatic failover between multiple
/// configured upstream servers and implements retry logic for connection
/// establishment.
#[derive(Debug, Clone)]
pub struct Upstream {
    upstream_channel_state: UpstreamChannelState,
    upstream_channel_data: Arc<Mutex<UpstreamData>>,
}

impl Upstream {
    /// Creates a new upstream connection by attempting to connect to configured servers.
    ///
    /// This method tries to establish a connection to one of the provided upstream
    /// servers, implementing retry logic and fallback behavior. It will attempt
    /// to connect to each server multiple times before giving up.
    ///
    /// # Arguments
    /// * `upstreams` - List of (address, public_key) pairs for upstream servers
    /// * `channel_manager_sender` - Channel to send messages to the channel manager
    /// * `channel_manager_receiver` - Channel to receive messages from the channel manager
    /// * `notify_shutdown` - Broadcast channel for shutdown coordination
    /// * `shutdown_complete_tx` - Channel to signal shutdown completion
    ///
    /// # Returns
    /// * `Ok(Upstream)` - Successfully connected to an upstream server
    /// * `Err(TproxyError)` - Failed to connect to any upstream server
    pub async fn new(
        upstreams: &[(SocketAddr, Secp256k1PublicKey)],
        channel_manager_sender: Sender<EitherFrame>,
        channel_manager_receiver: Receiver<EitherFrame>,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) -> Result<Self, TproxyError> {
        let mut shutdown_rx = notify_shutdown.subscribe();
        const RETRIES_PER_UPSTREAM: u8 = 3;

        for (index, (addr, pubkey)) in upstreams.iter().enumerate() {
            info!("Trying to connect to upstream {} at {}", index, addr);

            for attempt in 1..=RETRIES_PER_UPSTREAM {
                if shutdown_rx.try_recv().is_ok() {
                    info!("Shutdown signal received during upstream connection attempt. Aborting.");
                    drop(shutdown_complete_tx);
                    return Err(TproxyError::Shutdown);
                }

                match TcpStream::connect(addr).await {
                    Ok(socket) => {
                        info!(
                            "Connected to upstream at {} (attempt {}/{})",
                            addr, attempt, RETRIES_PER_UPSTREAM
                        );

                        let initiator = Initiator::from_raw_k(pubkey.into_bytes())?;
                        match Connection::new(socket, HandshakeRole::Initiator(initiator)).await {
                            Ok((receiver, sender)) => {
                                let upstream_channel_state = UpstreamChannelState::new(
                                    channel_manager_sender,
                                    channel_manager_receiver,
                                    receiver,
                                    sender,
                                );
                                let upstream_channel_data = Arc::new(Mutex::new(UpstreamData));
                                info!("Successfully initialized upstream channel with {}", addr);

                                return Ok(Self {
                                    upstream_channel_state,
                                    upstream_channel_data,
                                });
                            }
                            Err(e) => {
                                error!(
                                    "Failed Noise handshake with {}: {:?}. Retrying...",
                                    addr, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to connect to {}: {}. Retry {}/{}...",
                            addr, e, attempt, RETRIES_PER_UPSTREAM
                        );
                    }
                }

                sleep(Duration::from_secs(5)).await;
            }

            warn!("Exhausted retries for upstream {} at {}", index, addr);
        }

        error!("Failed to connect to any configured upstream.");
        drop(shutdown_complete_tx);
        Err(TproxyError::Shutdown)
    }

    /// Starts the upstream connection and begins message processing.
    ///
    /// This method:
    /// - Completes the SV2 handshake with the upstream server
    /// - Spawns the main message processing task
    /// - Handles graceful shutdown coordination
    ///
    /// The method will first attempt to complete the SV2 setup connection
    /// handshake. If successful, it spawns a task to handle bidirectional
    /// message flow between the channel manager and upstream server.
    ///
    /// # Arguments
    /// * `notify_shutdown` - Broadcast channel for shutdown coordination
    /// * `shutdown_complete_tx` - Channel to signal shutdown completion
    /// * `status_sender` - Channel for sending status updates
    /// * `task_manager` - Manager for spawned async tasks
    ///
    /// # Returns
    /// * `Ok(())` - Upstream started successfully
    /// * `Err(TproxyError)` - Error during startup or handshake
    pub async fn start(
        self,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: Sender<Status>,
        task_manager: Arc<TaskManager>,
    ) -> Result<(), TproxyError> {
        info!("Upstream: starting...");

        let mut shutdown_rx = notify_shutdown.subscribe();

        // Wait for connection setup or shutdown signal
        tokio::select! {
            result = self.setup_connection() => {
                if let Err(e) = result {
                    error!("Upstream: failed to set up SV2 connection: {:?}", e);
                    drop(shutdown_complete_tx);
                    return Err(e);
                }
                info!("Upstream: SV2 connection setup successful.");
            }
            message = shutdown_rx.recv() => {
                match message {
                    Ok(ShutdownMessage::ShutdownAll) => {
                        info!("Upstream: shutdown signal received during connection setup.");
                        drop(shutdown_complete_tx);
                        return Ok(());
                    }
                    Ok(_) => {}

                    Err(e) => {
                        error!("Upstream: failed to receive shutdown signal: {e}");
                        drop(shutdown_complete_tx);
                        return Ok(());
                    }
                }
            }
        }

        // Wrap status sender and start upstream task
        let wrapped_status_sender = StatusSender::Upstream(status_sender);

        self.run_upstream_task(
            notify_shutdown,
            shutdown_complete_tx,
            wrapped_status_sender,
            task_manager,
        )?;

        Ok(())
    }

    /// Performs the SV2 handshake setup with the upstream server.
    ///
    /// This method handles the initial SV2 protocol handshake by:
    /// - Creating and sending a SetupConnection message
    /// - Waiting for the handshake response
    /// - Validating and processing the response
    ///
    /// The handshake establishes the protocol version, capabilities, and
    /// other connection parameters needed for SV2 communication.
    ///
    /// # Returns
    /// * `Ok(())` - Handshake completed successfully
    /// * `Err(TproxyError)` - Handshake failed or connection error
    pub async fn setup_connection(&self) -> Result<(), TproxyError> {
        info!("Upstream: initiating SV2 handshake...");

        // Build SetupConnection message
        let setup_conn_msg = Self::get_setup_connection_message(2, 2, false)?;
        let sv2_frame: StdFrame =
            Message::Common(setup_conn_msg.into())
                .try_into()
                .map_err(|e| {
                    error!("Failed to serialize SetupConnection message: {:?}", e);
                    TproxyError::RolesSv2LogicError(e)
                })?;

        // Send SetupConnection message to upstream
        info!("Upstream: sending SetupConnection...");
        self.upstream_channel_state
            .upstream_sender
            .send(sv2_frame.into())
            .await
            .map_err(|e| {
                error!("Failed to send SetupConnection to upstream: {:?}", e);
                TproxyError::ChannelErrorSender
            })?;

        let mut incoming: StdFrame =
            match self.upstream_channel_state.upstream_receiver.recv().await {
                Ok(frame) => {
                    debug!("Received handshake response from upstream.");
                    frame.try_into()?
                }
                Err(e) => {
                    error!("Failed to receive handshake response from upstream: {}", e);
                    return Err(TproxyError::CodecNoise(
                        codec_sv2::noise_sv2::Error::ExpectedIncomingHandshakeMessage,
                    ));
                }
            };

        let msg_type = incoming
            .get_header()
            .ok_or_else(|| {
                error!("Expected handshake frame but no header found.");
                framing_sv2::Error::ExpectedHandshakeFrame
            })?
            .msg_type();

        let payload = incoming.payload();

        // Handle the parsed handshake message
        ParseCommonMessagesFromUpstream::handle_message_common(
            self.upstream_channel_data.clone(),
            msg_type,
            payload,
        )
        .map_err(|e| {
            error!("Failed to handle handshake message from upstream: {:?}", e);
            TproxyError::UnexpectedMessage
        })?;

        info!("Upstream: handshake completed successfully.");
        Ok(())
    }

    /// Processes incoming messages from the upstream SV2 server.
    ///
    /// This method handles different types of frames received from upstream:
    /// - SV2 frames: Parses and routes mining/common messages appropriately
    /// - Handshake frames: Logs for debugging (shouldn't occur during normal operation)
    ///
    /// Common messages are handled directly, while mining messages are forwarded
    /// to the channel manager for processing and distribution to downstream connections.
    ///
    /// # Arguments
    /// * `message` - The frame received from the upstream server
    ///
    /// # Returns
    /// * `Ok(())` - Message processed successfully
    /// * `Err(TproxyError)` - Error processing the message
    pub async fn on_upstream_message(&self, message: EitherFrame) -> Result<(), TproxyError> {
        match message {
            EitherFrame::Sv2(sv2_frame) => {
                // Convert to standard frame
                let std_frame: StdFrame = sv2_frame
                    .try_into()
                    .map_err(|_| TproxyError::General("Infalliable message".to_string()))?;

                // Parse message from frame
                let mut frame: codec_sv2::Frame<AnyMessage<'static>, buffer_sv2::Slice> =
                    std_frame.clone().into();

                let (msg_type, mut payload, parsed_message) = message_from_frame(&mut frame)?;

                match parsed_message {
                    AnyMessage::Common(_) => {
                        // Handle common upstream messages
                        ParseCommonMessagesFromUpstream::handle_message_common(
                            self.upstream_channel_data.clone(),
                            msg_type,
                            payload.as_mut_slice(),
                        )
                        .map_err(|e| {
                            error!("Error handling common upstream message: {:?}", e);
                            TproxyError::UnexpectedMessage
                        })?;
                    }

                    AnyMessage::Mining(_) => {
                        // Forward mining message to channel manager
                        let frame_to_forward = EitherFrame::Sv2(std_frame.into());
                        self.upstream_channel_state
                            .channel_manager_sender
                            .send(frame_to_forward)
                            .await
                            .map_err(|e| {
                                error!("Failed to send mining message to channel manager: {:?}", e);
                                TproxyError::ChannelErrorSender
                            })?;
                    }

                    _ => {
                        error!("Received unsupported message type from upstream.");
                        return Err(TproxyError::UnexpectedMessage);
                    }
                }
            }

            EitherFrame::HandShake(handshake_frame) => {
                debug!("Received handshake frame: {:?}", handshake_frame);
            }
        }
        Ok(())
    }

    /// Spawns a unified task to handle upstream message I/O and shutdown logic.
    fn run_upstream_task(
        self,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: StatusSender,
        task_manager: Arc<TaskManager>,
    ) -> Result<(), TproxyError> {
        let mut shutdown_rx = notify_shutdown.subscribe();
        let shutdown_complete_tx = shutdown_complete_tx.clone();

        task_manager.spawn(async move {
            info!("Upstream task started (combined sender + receiver loop).");

            loop {
                tokio::select! {
                    // Handle shutdown signals
                    shutdown = shutdown_rx.recv() => {
                        match shutdown {
                            Ok(ShutdownMessage::ShutdownAll) => {
                                info!("Upstream: received ShutdownAll signal. Exiting loop.");
                                break;
                            }
                            Ok(_) => {
                                // Ignore other shutdown variants for upstream
                            }
                            Err(e) => {
                                error!("Upstream: failed to receive shutdown signal: {e}");
                                break;
                            }
                        }
                    }

                    // Handle incoming SV2 messages from upstream
                    result = self.upstream_channel_state.upstream_receiver.recv() => {
                        match result {
                            Ok(frame) => {
                                debug!("Upstream: received frame.");
                                if let Err(e) = self.on_upstream_message(frame).await {
                                    error!("Upstream: error while processing message: {e:?}");
                                    handle_error(&status_sender, TproxyError::ChannelErrorSender).await;
                                }
                            }
                            Err(e) => {
                                error!("Upstream: receiver channel closed unexpectedly: {e}");
                                handle_error(&status_sender, TproxyError::ChannelErrorReceiver(e)).await;
                                break;
                            }
                        }
                    }

                    // Handle messages from channel manager to send upstream
                    result = self.upstream_channel_state.channel_manager_receiver.recv() => {
                        match result {
                            Ok(msg) => {
                                info!("Upstream: sending message from channel manager.");
                                if let Err(e) = self.send_upstream(msg).await {
                                    error!("Upstream: failed to send message: {e:?}");
                                    handle_error(&status_sender, TproxyError::ChannelErrorSender).await;
                                }
                            }
                            Err(e) => {
                                error!("Upstream: channel manager receiver closed: {e}");
                                handle_error(&status_sender, TproxyError::ChannelErrorReceiver(e)).await;
                                break;
                            }
                        }
                    }
                }
            }

            self.upstream_channel_state.drop();
            warn!("Upstream: task shutting down cleanly.");
            drop(shutdown_complete_tx);
        });

        Ok(())
    }

    /// Sends a message to the upstream SV2 server.
    ///
    /// This method forwards messages from the channel manager to the upstream
    /// server. Messages are typically mining-related (share submissions, channel
    /// requests, etc.) that need to be sent upstream.
    ///
    /// # Arguments
    /// * `sv2_frame` - The SV2 frame to send to the upstream server
    ///
    /// # Returns
    /// * `Ok(())` - Message sent successfully
    /// * `Err(TproxyError)` - Error sending the message
    pub async fn send_upstream(&self, sv2_frame: EitherFrame) -> Result<(), TproxyError> {
        debug!("Sending message to upstream.");

        self.upstream_channel_state
            .upstream_sender
            .send(sv2_frame.into())
            .await
            .map_err(|e| {
                error!("Failed to send message to upstream: {:?}", e);
                TproxyError::ChannelErrorSender
            })?;

        Ok(())
    }

    /// Constructs the `SetupConnection` message.
    #[allow(clippy::result_large_err)]
    fn get_setup_connection_message(
        min_version: u16,
        max_version: u16,
        is_work_selection_enabled: bool,
    ) -> Result<SetupConnection<'static>, TproxyError> {
        let endpoint_host = "0.0.0.0".to_string().into_bytes().try_into()?;
        let vendor = "SRI".to_string().try_into()?;
        let hardware_version = "Translator Proxy".to_string().try_into()?;
        let firmware = String::new().try_into()?;
        let device_id = String::new().try_into()?;
        let flags = if is_work_selection_enabled {
            0b110
        } else {
            0b100
        };

        Ok(SetupConnection {
            protocol: Protocol::MiningProtocol,
            min_version,
            max_version,
            flags,
            endpoint_host,
            endpoint_port: 50,
            vendor,
            hardware_version,
            firmware,
            device_id,
        })
    }
}
