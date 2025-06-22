use crate::{
    error::{Error, ProxyResult},
    utils::message_from_frame,
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
    time::{sleep, Duration},
};
use tracing::{debug, error, info, warn};
pub type Message = AnyMessage<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type EitherFrame = StandardEitherFrame<Message>;

#[derive(Debug, Clone)]
pub struct Upstream {
    /// Receiver for the SV2 Upstream role
    pub upstream_receiver: Receiver<EitherFrame>,
    /// Sender for the SV2 Upstream role
    pub upstream_sender: Sender<EitherFrame>,
    /// Sender for the ChannelManager thread
    pub upstream_to_channel_manager_sender: Sender<EitherFrame>,
    /// Receiver for the ChannelManager thread
    pub channel_manager_to_upstream_receiver: Receiver<EitherFrame>,
}

impl Upstream {
    /// Attempts to connect to the SV2 Upstream role with retry.
    pub async fn new(
        upstream_address: SocketAddr,
        upstream_authority_public_key: Secp256k1PublicKey,
        upstream_to_channel_manager_sender: Sender<EitherFrame>,
        channel_manager_to_upstream_receiver: Receiver<EitherFrame>,
    ) -> ProxyResult<'static, Self> {
        info!("Attempting to connect to upstream at {}", upstream_address);

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
                }
            }
        };

        let initiator = Initiator::from_raw_k(upstream_authority_public_key.into_bytes())?;
        info!("I am the initiator");
        let (upstream_receiver, upstream_sender) =
            Connection::new(socket, HandshakeRole::Initiator(initiator))
                .await
                .map_err(|e| {
                    error!("Failed to establish Noise connection: {:?}", e);
                    e
                })
                .unwrap();

        info!("Noise handshake with upstream completed.");

        Ok(Self {
            upstream_receiver,
            upstream_sender,
            upstream_to_channel_manager_sender,
            channel_manager_to_upstream_receiver,
        })
    }

    pub async fn start(&mut self) -> ProxyResult<'static, ()> {
        info!("Starting upstream connection.");

        self.setup_connection().await?;
        self.spawn_upstream_receiver()?;
        self.spawn_upstream_sender()?;

        info!("Upstream fully initialized.");
        Ok(())
    }

    /// Handles SV2 handshake setup with the upstream.
    pub async fn setup_connection(&mut self) -> ProxyResult<'static, ()> {
        info!("Setting up SV2 connection with upstream.");

        let sender = self.upstream_sender.clone();
        let receiver = self.upstream_receiver.clone();

        let setup_connection = Self::get_setup_connection_message(2, 2, false)?;
        let sv2_frame: StdFrame = Message::Common(setup_connection.into()).try_into()?;
        let either_frame = sv2_frame.into();

        info!("Sending SetupConnection message to upstream.");
        sender.send(either_frame).await?;

        let mut incoming: StdFrame = match receiver.recv().await {
            Ok(frame) => {
                debug!("Received handshake response from upstream.");
                frame.try_into()?
            }
            Err(e) => {
                error!("Failed to receive handshake response from upstream: {}", e);
                return Err(Error::CodecNoise(
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

        let self_mutex = Arc::new(Mutex::new(self.clone()));
        ParseCommonMessagesFromUpstream::handle_message_common(self_mutex, message_type, payload)?;

        info!("SV2 SetupConnection handshake completed successfully.");
        Ok(())
    }

    pub async fn on_upstream_message(&self, message: EitherFrame) -> Result<(), Error> {
        self.upstream_to_channel_manager_sender
            .send(message)
            .await
            .map_err(|_| Error::ChannelErrorSender);
        Ok(())
    }

    /// Spawns the upstream receiver task.
    fn spawn_upstream_receiver(&self) -> ProxyResult<'static, ()> {
        info!("Spawning upstream receiver task.");
        let upstream = self.clone();

        tokio::spawn(async move {
            while let Ok(message) = upstream.upstream_receiver.recv().await {
                debug!("Received frame from upstream.");
                if let Err(e) = upstream.on_upstream_message(message).await {
                    error!("Error while processing upstream message: {:?}", e);
                }
            }

            warn!("Upstream receiver loop exited.");
        });

        Ok(())
    }

    /// Spawns the upstream sender task.
    fn spawn_upstream_sender(&self) -> ProxyResult<'static, ()> {
        info!("Spawning upstream sender task.");
        let upstream = self.clone();

        tokio::spawn(async move {
            while let Ok(message) = upstream.channel_manager_to_upstream_receiver.recv().await {
                debug!("Received message from channel manager to send upstream.");
                if let Err(e) = upstream.send_upstream(message.try_into().unwrap()).await {
                    error!("Failed to send message upstream: {:?}", e);
                }
            }

            warn!("Upstream sender loop exited.");
        });

        Ok(())
    }

    /// Sends a mining message to upstream.
    pub async fn send_upstream(&self, sv2_frame: EitherFrame) -> ProxyResult<'static, ()> {
        debug!("Sending message to upstream.");
        let either_frame = sv2_frame.into();
        self.upstream_sender.send(either_frame).await?;
        Ok(())
    }

    /// Constructs the `SetupConnection` message.
    #[allow(clippy::result_large_err)]
    fn get_setup_connection_message(
        min_version: u16,
        max_version: u16,
        is_work_selection_enabled: bool,
    ) -> ProxyResult<'static, SetupConnection<'static>> {
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
