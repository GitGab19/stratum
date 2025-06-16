use crate::{upstream_sv2::Upstream, downstream_sv1::Downstream, error::Error};
use roles_logic_sv2::{utils::{Id as IdFactory, Mutex}, channels::client::extended::ExtendedChannel};
use std::{sync::{Arc, RwLock}, collections::HashMap};
use roles_logic_sv2::parsers::Mining;
use roles_logic_sv2::mining_sv2::{OpenExtendedMiningChannel, OpenExtendedMiningChannelSuccess};
use binary_sv2::U256;
use roles_logic_sv2::handlers::mining::{ParseMiningMessagesFromUpstream, SendTo, SupportedChannelTypes};
use codec_sv2::{StandardSv2Frame, StandardEitherFrame};
use roles_logic_sv2::parsers::AnyMessage;
use tracing::error;
use roles_logic_sv2::mining_sv2::Target;

pub type Message = AnyMessage<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type EitherFrame = StandardEitherFrame<Message>;

#[derive(Debug, Clone)]
pub enum ChannelMappingMode {
    PerClient,
    Aggregated,
}

#[derive(Debug, Clone)]
pub struct ChannelManager {
    mode: ChannelMappingMode,
    upstream: Arc<Mutex<Upstream>>,
    downstream_id_factory: IdFactory,
    extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    channel_to_downstream: HashMap<u32, Arc<Mutex<Downstream>>>,
}

impl ChannelManager {
    pub fn new(mode: ChannelMappingMode, upstream: Arc<Mutex<Upstream>>) -> Self {
        Self {
            mode,
            upstream,
            downstream_id_factory: IdFactory::new(),
            extended_channels: HashMap::new(),
            channel_to_downstream: HashMap::new(),
        }
    }

    pub async fn on_new_sv1_connection(&mut self, user_identity: &str, hash_rate: f32, max_target: U256, min_extranonce_size: u16) -> Result<(), Error<'static>> {
        match self.mode {
            ChannelMappingMode::PerClient => {
                let upstream = self.upstream.safe_lock(|u| u.clone())?;
                
                // Send OpenExtendedMiningChannel message
                let downstream_id = self.downstream_id_factory.next();
                
                
                // Wait for response
                let mut incoming: StdFrame = match upstream.receiver.recv().await {
                    Ok(frame) => frame.try_into()?,
                    Err(e) => {
                        error!("Upstream connection closed: {}", e);
                        return Err(Error::SubprotocolMining(
                            "Failed to open extended mining channel".to_string(),
                        ));
                    }
                };
                
                // Parse response
                let message_type = if let Some(header) = incoming.get_header() {
                    header.msg_type()
                } else {
                    return Err(Error::SubprotocolMining(
                        "Invalid mining message when opening downstream connection".to_string(),
                    ));
                };
                let payload = incoming.payload();
                
                match ParseMiningMessagesFromUpstream::handle_message_mining(
                    Arc::new(Mutex::new(self.clone())),
                    message_type,
                    payload,
                ) {
                    Ok(SendTo::None(Some(Mining::OpenExtendedMiningChannelSuccess(success)))) => {
                        let extranonce_prefix = success.extranonce_prefix.to_vec();
                        let extranonce_size = success.extranonce_size;
                        
                        // Convert target from U256 to Target
                        let target: Target = success.target.into();
                        
                        // Store the channel information
                        let channel = ExtendedChannel::new(
                            success.channel_id,
                            user_identity.to_string(),
                            extranonce_prefix,
                            target,
                            hash_rate,
                            true, // we assume version_rolling is true for extended channels
                            extranonce_size,
                        );
                        
                        self.extended_channels.insert(
                            success.channel_id,
                            Arc::new(RwLock::new(channel))
                        );
                        
                        self.channel_to_downstream.insert(
                            success.channel_id,
                            Arc::new(Mutex::new(Downstream::new(downstream_id, user_identity.to_string(), hash_rate, max_target, min_extranonce_size)))
                        );
                        
                        return Ok(());
                    }
                    Ok(SendTo::None(Some(Mining::OpenMiningChannelError(_)))) => {
                        return Err(Error::SubprotocolMining(
                            "Failed to open extended mining channel".to_string(),
                        ));
                    }
                    _ => {
                        return Err(Error::SubprotocolMining(
                            "Invalid mining message when opening downstream connection".to_string(),
                        ));
                    }
                }

            }
            ChannelMappingMode::Aggregated => todo!()
        }
    }
}