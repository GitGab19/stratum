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
    pub receiver: Receiver<EitherFrame>,
    /// Sender for the SV2 Upstream role
    pub sender: Sender<EitherFrame>,
    /// Sender for the ChannelManager thread
    pub channel_manager_sender: Sender<Mining<'static>>,
    /// Receiver for the ChannelManager thread
    pub channel_manager_receiver: Receiver<Mining<'static>>,
}

impl Upstream {
    /// Attempts to connect to the SV2 Upstream role with retry.
    pub async fn new(
        upstream_address: SocketAddr,
        upstream_authority_public_key: Secp256k1PublicKey,
        channel_manager_sender: Sender<Mining<'static>>,
        channel_manager_receiver: Receiver<Mining<'static>>,
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
        let (receiver, sender) = Connection::new(socket, HandshakeRole::Initiator(initiator))
            .await
            .map_err(|e| {
                error!("Failed to establish Noise connection: {:?}", e);
                e
            })
            .unwrap();

        info!("Noise handshake with upstream completed.");

        Ok(Self {
            receiver,
            sender,
            channel_manager_sender,
            channel_manager_receiver,
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

        let sender = self.sender.clone();
        let receiver = self.receiver.clone();

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

    pub async fn on_upstream_message(&self, message: Message) -> Result<(), Error> {
        match message {
            Message::Mining(mining_message) => {
                debug!(
                    "Forwarding mining message to channel manager: {:?}",
                    mining_message
                );
                self.channel_manager_sender
                    .send(mining_message)
                    .await
                    .map_err(|_| Error::ChannelErrorSender);
                Ok(())
            }
            Message::Common(common_message) => {
                debug!("Handling common message from upstream.");
                let self_mutex = Arc::new(Mutex::new(self.clone()));
                let mut frame: StdFrame =
                    AnyMessage::Common(common_message).try_into().map_err(|e| {
                        error!("Failed to parse common message: {:?}", e);
                        e
                    })?;
                let message_type = frame.get_header().unwrap().msg_type();
                let payload = frame.payload();

                ParseCommonMessagesFromUpstream::handle_message_common(
                    self_mutex,
                    message_type,
                    payload,
                )?;
                Ok(())
            }
            _ => {
                warn!("Received unknown message type from upstream: {:?}", message);
                Err(Error::UnexpectedMessage)
            }
        }
    }

    /// Sends a mining message to upstream.
    pub async fn send_upstream(&self, sv2_frame: StdFrame) -> ProxyResult<'static, ()> {
        debug!("Sending message to upstream.");
        let either_frame = sv2_frame.into();
        self.sender.send(either_frame).await?;
        Ok(())
    }

    /// Spawns the upstream receiver task.
    fn spawn_upstream_receiver(&self) -> ProxyResult<'static, ()> {
        info!("Spawning upstream receiver task.");
        let upstream = self.clone();

        tokio::spawn(async move {
            while let Ok(mut frame) = upstream.receiver.recv().await {
                debug!("Received frame from upstream.");
                let message = message_from_frame(&mut frame);

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
            while let Ok(message) = upstream.channel_manager_receiver.recv().await {
                debug!("Received message from channel manager to send upstream.");
                let sv2_frame: StdFrame = AnyMessage::Mining(message)
                    .try_into()
                    .expect("Failed to serialize mining message.");

                if let Err(e) = upstream.send_upstream(sv2_frame).await {
                    error!("Failed to send message upstream: {:?}", e);
                }
            }

            warn!("Upstream sender loop exited.");
        });

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
