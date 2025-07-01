use crate::{
    error::TproxyError,
    status::{handle_error, Status, StatusSender},
    utils::{message_from_frame, ShutdownMessage},
};
use async_channel::{Receiver, Sender};
use codec_sv2::{HandshakeRole, Initiator, StandardEitherFrame, StandardSv2Frame};
use key_utils::Secp256k1PublicKey;
use network_helpers_sv2::noise_connection::Connection;
use roles_logic_sv2::{
    common_messages_sv2::{Protocol, SetupConnection},
    handlers::common::ParseCommonMessagesFromUpstream,
    parsers::{AnyMessage, Mining},
    utils::Mutex,
};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::{sleep, Duration},
};
use tracing::{debug, error, info, warn};
pub type Message = AnyMessage<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type EitherFrame = StandardEitherFrame<Message>;

#[derive(Debug, Clone)]
pub struct UpstreamData;

#[derive(Debug, Clone)]
struct UpstreamChannelState {
    /// Receiver for the SV2 Upstream role
    pub upstream_receiver: Receiver<EitherFrame>,
    /// Sender for the SV2 Upstream role
    pub upstream_sender: Sender<EitherFrame>,
    /// Sender for the ChannelManager thread
    pub channel_manager_sender: Sender<EitherFrame>,
    /// Receiver for the ChannelManager thread
    pub channel_manager_receiver: Receiver<EitherFrame>,
}

impl UpstreamChannelState {
    fn new(
        channel_manager_sender: Sender<EitherFrame>,
        channel_manager_receiver: Receiver<EitherFrame>,
        upstream_receiver: Receiver<EitherFrame>,
        upstream_sender: Sender<EitherFrame>,
    ) -> Self {
        Self {
            channel_manager_sender,
            channel_manager_receiver,
            upstream_receiver,
            upstream_sender,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Upstream {
    upstream_channel_state: UpstreamChannelState,
    upstream_channel_data: Arc<Mutex<UpstreamData>>,
}

impl Upstream {
    /// Attempts to connect to the SV2 Upstream role with retry.
    pub async fn new(
        upstream_address: SocketAddr,
        upstream_authority_public_key: Secp256k1PublicKey,
        channel_manager_sender: Sender<EitherFrame>,
        channel_manager_receiver: Receiver<EitherFrame>,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) -> Result<Self, TproxyError> {
        let socket = loop {
            match TcpStream::connect(upstream_address).await {
                Ok(socket) => {
                    info!("Successfully connected to upstream at {}", upstream_address);
                    break socket;
                }
                Err(e) => {
                    error!(
                        "Failed to connect to upstream at {}: {}. Retrying in 5s.",
                        upstream_address, e
                    );
                    sleep(Duration::from_secs(5)).await;
                    if notify_shutdown.subscribe().try_recv().is_ok() {
                        info!("Shutdown signal received during upstream connection attempt. Aborting.");
                        drop(shutdown_complete_tx);
                        return Err(TproxyError::Shutdown);
                    }
                }
            }
        };

        let initiator = Initiator::from_raw_k(upstream_authority_public_key.into_bytes())?;

        let (upstream_receiver, upstream_sender) =
            Connection::new(socket, HandshakeRole::Initiator(initiator))
                .await
                .map_err(|e| {
                    error!("Failed to establish Noise connection: {:?}", e);
                    e
                })
                .unwrap();
        let upstream_channel_state = UpstreamChannelState::new(
            channel_manager_sender,
            channel_manager_receiver,
            upstream_receiver,
            upstream_sender,
        );
        let upstream_channel_data = Arc::new(Mutex::new(UpstreamData));

        Ok(Self {
            upstream_channel_state,
            upstream_channel_data,
        })
    }

    pub async fn start(
        self,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: Sender<Status>,
    ) -> Result<(), TproxyError> {
        info!("Upstream starting...");
        let mut shutdown_rx = notify_shutdown.subscribe();
        tokio::select! {
            result = self.setup_connection() => {
                if let Err(e) = result {
                    error!("Failed to setup SV2 connection with upstream: {:?}", e);
                    drop(shutdown_complete_tx.clone());
                    return Err(e);
                }
            },
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received during upstream setup connection. Aborting.");
                drop(shutdown_complete_tx.clone());
                return Ok(());
            }
        }
        let status_sender = StatusSender::Upstream(status_sender);
        self.run_upstream_task(notify_shutdown, shutdown_complete_tx, status_sender)?;
        Ok(())
    }

    /// Handles SV2 handshake setup with the upstream.
    pub async fn setup_connection(&self) -> Result<(), TproxyError> {
        info!("Setting up SV2 connection with upstream.");

        let setup_connection = Self::get_setup_connection_message(2, 2, false)?;
        let sv2_frame: StdFrame = Message::Common(setup_connection.into()).try_into().unwrap();
        let either_frame = sv2_frame.into();

        info!("Sending SetupConnection message to upstream.");
        self.upstream_channel_state
            .upstream_sender
            .send(either_frame)
            .await
            .unwrap();

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

        let message_type = incoming
            .get_header()
            .ok_or_else(|| {
                error!("Expected handshake frame but no header found.");
                framing_sv2::Error::ExpectedHandshakeFrame
            })?
            .msg_type();

        let payload = incoming.payload();

        ParseCommonMessagesFromUpstream::handle_message_common(
            self.upstream_channel_data.clone(),
            message_type,
            payload,
        )
        .unwrap();

        Ok(())
    }

    pub async fn on_upstream_message(&self, message: EitherFrame) -> Result<(), TproxyError> {
        match message {
            EitherFrame::Sv2(sv2_frame) => {
                let mut std_frame: StdFrame = sv2_frame.try_into().unwrap();

                // Use message_from_frame to parse the message
                let mut frame: codec_sv2::Frame<AnyMessage<'static>, buffer_sv2::Slice> =
                    std_frame.clone().into();
                let (message_type, mut payload, parsed_message) =
                    message_from_frame(&mut frame).unwrap();

                match parsed_message {
                    AnyMessage::Common(_) => {
                        ParseCommonMessagesFromUpstream::handle_message_common(
                            self.upstream_channel_data.clone(),
                            message_type,
                            payload.as_mut_slice(),
                        )
                        .unwrap();
                    }
                    AnyMessage::Mining(_) => {
                        // Mining message - send to channel manager
                        let either_frame = EitherFrame::Sv2(std_frame.into());
                        self.upstream_channel_state
                            .channel_manager_sender
                            .send(either_frame)
                            .await
                            .map_err(|e| {
                                error!("Failed to send message to channel manager: {:?}", e);
                                // TproxyError::ChannelErrorSender
                                TproxyError::General("Channel sender Error".to_string())
                            });
                    }
                    _ => {
                        // Other message types - return error
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

    fn run_upstream_task(
        self,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: StatusSender,
    ) -> Result<(), TproxyError> {
        let mut shutdown_rx = notify_shutdown.subscribe();
        let shutdown_complete_tx = shutdown_complete_tx.clone();

        tokio::spawn(async move {
            info!("Upstream task started (combined sender + receiver).");

            loop {
                tokio::select! {
                    message = shutdown_rx.recv() => {
                        match message {
                            Ok(ShutdownMessage::ShutdownAll) => {
                                info!("Upstream task received shutdown signal. Exiting loop.");
                                break;
                            }
                            _ => {}
                        }
                    }
                    msg = self.upstream_channel_state.upstream_receiver.recv() => {
                        match msg {
                            Ok(frame) => {
                                debug!("Received frame from upstream.");
                                if let Err(e) = self.on_upstream_message(frame).await {
                                    error!("Error while processing upstream message: {:?}", e);
                                    handle_error(&status_sender, TproxyError::ChannelErrorSender);
                                }
                            }
                            Err(e) => {
                                error!("Upstream receiver channel error: {:?}. Exiting loop.", e);
                                handle_error(&status_sender, TproxyError::ChannelErrorReceiver(e));
                                break;
                            }
                        }
                    }

                    msg = self.upstream_channel_state.channel_manager_receiver.recv() => {
                        match msg {
                            Ok(msg) => {
                                debug!("Received message from channel manager to send upstream.");
                                if let Err(e) = self.send_upstream(msg).await {
                                    error!("Failed to send message upstream: {:?}", e);
                                    handle_error(&status_sender, TproxyError::ChannelErrorSender);
                                }
                            }
                            Err(e) => {
                                error!("Channel manager receiver channel error: {e:?}. Exiting loop.");
                                handle_error(&status_sender, TproxyError::ChannelErrorReceiver(e));
                                break;
                            }
                        }
                    }
                }
            }

            self.upstream_channel_state.upstream_receiver.close();
            self.upstream_channel_state.channel_manager_receiver.close();
            self.upstream_channel_state.channel_manager_sender.close();
            self.upstream_channel_state.upstream_sender.close();

            warn!("Upstream combined loop exited.");
            drop(shutdown_complete_tx);
        });

        Ok(())
    }

    /// Sends a mining message to upstream.
    pub async fn send_upstream(&self, sv2_frame: EitherFrame) -> Result<(), TproxyError> {
        debug!("Sending message to upstream.");
        let either_frame = sv2_frame.into();
        self.upstream_channel_state
            .upstream_sender
            .send(either_frame)
            .await
            .unwrap();
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
