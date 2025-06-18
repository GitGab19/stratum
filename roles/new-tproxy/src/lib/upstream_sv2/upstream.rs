use std::{net::SocketAddr, sync::Arc};
use binary_sv2::U256;
use network_helpers_sv2::noise_connection::Connection;
use codec_sv2::{HandshakeRole, Initiator, StandardEitherFrame, StandardSv2Frame};
use roles_logic_sv2::{common_messages_sv2::{Protocol, SetupConnection}, handlers::common::ParseCommonMessagesFromUpstream, mining_sv2::{OpenExtendedMiningChannel, SubmitSharesExtended, UpdateChannel}, parsers::{AnyMessage, Mining}, utils::Mutex};
use async_channel::{Receiver, Sender};
use tracing::error;
use key_utils::Secp256k1PublicKey;
use crate::error::{Error, ProxyResult};
use tokio::{
    net::TcpStream,
    time::{sleep, Duration},
};
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
}

impl Upstream {
    pub async fn new(
        upstream_address: SocketAddr,
        upstream_authority_public_key: Secp256k1PublicKey,
        channel_manager_sender: Sender<Mining<'static>>,
    ) -> ProxyResult<'static, Arc<Mutex<Self>>> {
        // Connect to the SV2 Upstream role retry connection every 5 seconds.
        let socket = loop {
            match TcpStream::connect(upstream_address).await {
                Ok(socket) => break socket,
                Err(e) => {
                    error!(
                        "Failed to connect to Upstream role at {}, retrying in 5s: {}",
                        upstream_address, e
                    );

                    sleep(Duration::from_secs(5)).await;
                }
            }
        };
        let pub_key: Secp256k1PublicKey = upstream_authority_public_key;
        let initiator = Initiator::from_raw_k(pub_key.into_bytes())?;
        // Channel to send and receive messages to the SV2 Upstream role
        let (receiver, sender) = Connection::new(socket, HandshakeRole::Initiator(initiator))
            .await
            .unwrap();
        Ok(Arc::new(Mutex::new(Self {
            receiver,
            sender,
            channel_manager_sender,
        })))
    }

    // This function is used to setup the connection to the upstream
    pub async fn setup_connection(self_: Arc<Mutex<Self>>) -> ProxyResult<'static, ()> {
        let sender = self_.safe_lock(|s| s.sender.clone())?;
        let receiver = self_.safe_lock(|s| s.receiver.clone())?;
        // Get the `SetupConnection` message with Mining Device information (currently hard coded)
        let min_version = 2;
        let max_version = 2;
        let setup_connection = Self::get_setup_connection_message(min_version, max_version, false)?;
        // Put the `SetupConnection` message in a `StdFrame` to be sent over the wire
        let sv2_frame: StdFrame = Message::Common(setup_connection.into()).try_into()?;
        let either_frame = sv2_frame.into();
        // Send the `SetupConnection` frame to the SV2 Upstream role
        sender.send(either_frame).await?;

        let mut incoming: StdFrame = match receiver.recv().await {
            Ok(frame) => frame.try_into()?,
            Err(e) => {
                error!("Upstream connection closed: {}", e);
                return Err(Error::CodecNoise(
                    codec_sv2::noise_sv2::Error::ExpectedIncomingHandshakeMessage,
                ));
            }
        };
        // Gets the binary frame message type from the message header
        let message_type = if let Some(header) = incoming.get_header() {
            header.msg_type()
        } else {
            return Err(framing_sv2::Error::ExpectedHandshakeFrame.into());
        };
        // Gets the message payload
        let payload = incoming.payload();
        ParseCommonMessagesFromUpstream::handle_message_common(
            self_.clone(),
            message_type,
            payload,
        )?;

        Ok(())
    }

    // This function is used to open an extended mining channel to the upstream
    pub async fn open_extended_mining_channel(
        &self,
        request_id: u32,
        user_identity: &str,
        hash_rate: f32,
        max_target: U256<'static>,
        min_extranonce_size: u16,
    ) -> ProxyResult<'static, ()> {
        let open_extended_mining_channel = Message::Mining(roles_logic_sv2::parsers::Mining::OpenExtendedMiningChannel(OpenExtendedMiningChannel {
            request_id,
            user_identity: user_identity.to_string().try_into()?,
            nominal_hash_rate: hash_rate,
            max_target: max_target.into(),
            min_extranonce_size,
        }));
        let sv2_frame: StdFrame = open_extended_mining_channel.try_into()?;
        self.send_upstream(sv2_frame).await?;

        Ok(())
    }

    // This function is used to submit shares to the upstream
    pub async fn submit_shares_extended(&self, share: SubmitSharesExtended<'static>) -> ProxyResult<'static, ()> {
        let submit_shares_extended = Message::Mining(roles_logic_sv2::parsers::Mining::SubmitSharesExtended(share));
        let sv2_frame: StdFrame = submit_shares_extended.try_into()?;
        self.send_upstream(sv2_frame).await?;

        Ok(())
    }

    // This function is used to update the upstream when there is a change in downstream hashrate
    pub async fn update_channel(&self, channel_id: u32, nominal_hash_rate: f32, maximum_target: U256<'static>) -> ProxyResult<'static, ()> {
        let update_channel = Message::Mining(roles_logic_sv2::parsers::Mining::UpdateChannel(UpdateChannel {
            channel_id,
            nominal_hash_rate,
            maximum_target,
        }));
        let sv2_frame: StdFrame = update_channel.try_into()?;
        self.send_upstream(sv2_frame).await?;

        Ok(())
    }

    // This function is used to handle the messages from the upstream.
    // It is used to forward the mining messages to the channel manager.
    pub async fn on_upstream_message(&self, message: Message) -> Result<(), Error> {
        match message {
            Message::Mining(mining_message) => {
                self.channel_manager_sender.send(mining_message).await.map_err(|_| Error::ChannelErrorSender);
                Ok(())
            }
            _ => {
                error!("Received unknown message from upstream: {:?}", message);
                Err(Error::UnexpectedMessage)
            }
        }
    }

    // Creates the initial `SetupConnection` message for the SV2 handshake.
    //
    // This message contains information about the proxy acting as a mining device,
    // including supported protocol versions, flags, and hardcoded endpoint details.
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
        let flags = match is_work_selection_enabled {
            false => 0b0000_0000_0000_0000_0000_0000_0000_0100,
            true => 0b0000_0000_0000_0000_0000_0000_0000_0110,
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

    /// Send a SV2 message to the Upstream role
    pub async fn send_upstream(&self, sv2_frame: StdFrame) -> ProxyResult<'static, ()> {
        let either_frame = sv2_frame.into();
        self.sender.send(either_frame).await?;
        Ok(())
    }
}
