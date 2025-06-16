
use binary_sv2::U256;
use codec_sv2::{StandardEitherFrame, StandardSv2Frame};
use roles_logic_sv2::{parsers::{AnyMessage, Mining}, mining_sv2::OpenExtendedMiningChannel};
use async_channel::{Receiver, Sender};

pub type Message = AnyMessage<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type EitherFrame = StandardEitherFrame<Message>;

#[derive(Debug, Clone)]
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

    pub async fn open_extended_mining_channel(
        &self,
        request_id: u32,
        user_identity: &str,
        hash_rate: f32,
        max_target: U256,
        min_extranonce_size: u16,
    ) -> Result<(), async_channel::SendError<EitherFrame>> {
        let open_extended_mining_channel = Mining::OpenExtendedMiningChannel(OpenExtendedMiningChannel {
            request_id: request_id,
            user_identity: user_identity.to_string().try_into()?,
            nominal_hash_rate: hash_rate,
            max_target: max_target.into(),
            min_extranonce_size,
        });
        
        let sv2_frame: StdFrame = Message::Mining(open_extended_mining_channel).try_into()?;
        self.sender.send(EitherFrame::Sv2(sv2_frame)).await?;

        Ok(())
    }
}
