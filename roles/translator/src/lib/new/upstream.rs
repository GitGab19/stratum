use async_channel::Receiver;
use async_channel::Sender;
use binary_sv2::u256_from_int;
use roles_logic_sv2::{
    common_properties::IsUpstream,
    mining_sv2::{OpenExtendedMiningChannel, ExtendedExtranonce},
    utils::Mutex,
};
use std::sync::Arc;

/// Represents a generic SV2 message with a static lifetime.
pub type Message = AnyMessage<'static>;
/// A standard SV2 frame containing a message.
pub type StdFrame = StandardSv2Frame<Message>;
/// A standard SV2 frame that can contain either type of frame.
pub type EitherFrame = StandardEitherFrame<Message>;

pub struct Upstream {
    pub receiver: Receiver<EitherFrame>,
    pub sender: Sender<EitherFrame>,
}

impl Upstream {
    pub fn new(
        receiver: Receiver<EitherFrame>,
        sender: Sender<EitherFrame>,
    ) -> Self {
        Self {
            receiver,
            sender,
        }
    }

    /// Main message handling loop that processes incoming messages from upstream
    pub async fn handle_messages(&mut self) -> Result<(), Error<'static>> {
        while let Ok(frame) = self.receiver.recv().await {
            let std_frame: StdFrame = frame.try_into()?;
            
            // Get message type from header
            let message_type = if let Some(header) = std_frame.get_header() {
                header.msg_type()
            } else {
                return Err(framing_sv2::Error::ExpectedHandshakeFrame.into());
            };

            let payload = std_frame.payload();

            // Route to appropriate handler based on message type
            match message_type {
                // Common messages
                0x00..=0x0F => {
                    // Handle common messages
                    let handler = CommonMessageHandler::new(self);
                    handler.handle_message(message_type, payload)?;
                }
                // Mining messages
                0x20..=0x3F => {
                    // Handle mining messages
                    let handler = MiningMessageHandler::new(self);
                    handler.handle_message(message_type, payload)?;
                }
                _ => return Err(Error::InvalidMessageType(message_type)),
            }
        }
        Ok(())
    }

    pub async fn open_extended_mining_channel(
        self_: Arc<Mutex<Self>>,
        nominal_hash_rate: f32,
        min_extranonce_size: u16,
    ) -> Result<(ExtendedExtranonce, u32), Error<'static>> {
        let user_identity = "ABC".to_string().try_into()?;

        let open_channel = Mining::OpenExtendedMiningChannel(OpenExtendedMiningChannel {
            request_id: 0, // TODO
            user_identity,
            nominal_hash_rate,
            max_target: u256_from_int(u64::MAX), // TODO
            min_extranonce_size,
        });

        let sv2_frame: StdFrame = Message::Mining(open_channel).try_into()?;
        
        let mut connection = self_.safe_lock(|s| s.connection.clone())?;
        connection.send(sv2_frame).await?;

        // Wait for response
        let mut incoming: StdFrame = match connection.receiver.recv().await {
            Ok(frame) => frame.try_into()?,
            Err(e) => {
                error!("Upstream connection closed: {}", e);
                return Err(CodecNoise(
                    codec_sv2::noise_sv2::Error::ExpectedIncomingHandshakeMessage,
                ));
            }
        };

        // Parse response and return extranonce and channel ID
        let message_type = if let Some(header) = incoming.get_header() {
            header.msg_type()
        } else {
            return Err(framing_sv2::Error::ExpectedHandshakeFrame.into());
        };
        let payload = incoming.payload();

        match ParseMiningMessagesFromUpstream::handle_message_mining(
            self_.clone(),
            message_type,
            payload,
        )? {
            Ok(SendTo::None(Some(Mining::OpenExtendedMiningChannelSuccess(m)))) => {
                Ok((m.extranonce, m.channel_id))
            }
            Ok(SendTo::None(Some(Mining::OpenMiningChannelError(e)))) => {
                Err(e.into())
            }
            _ => Err(Error::RolesSv2Logic(RolesLogicError::InvalidMessageType)),
        }
    }
}
