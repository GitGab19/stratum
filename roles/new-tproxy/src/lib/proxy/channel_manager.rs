use crate::{config::TranslatorConfig, downstream_sv1::{downstream::Downstream, DownstreamMessages}, error::Error, upstream_sv2::{upstream::{EitherFrame, StdFrame}, Upstream}};
use roles_logic_sv2::{channels::client::extended::ExtendedChannel, handlers::mining::{ParseMiningMessagesFromUpstream, SendTo}, mining_sv2::{NewExtendedMiningJob, SubmitSharesExtended}, parsers::Mining, utils::{Id as IdFactory, Mutex}};
use std::{sync::{Arc, RwLock}, collections::HashMap};
use binary_sv2::U256;
use async_channel::Receiver;

pub type Sv2Message = Mining<'static>;

#[derive(Debug, Clone)]
pub enum ChannelMappingMode {
    // This is the mode where each client has its own channel.
    PerClient,
    // This is the mode where all clients share the same channel.
    Aggregated,
}

#[derive(Debug, Clone)]
pub struct ChannelManager {
    // This is the mode of the channel mapping.
    mode: ChannelMappingMode,
    // This is a mapping of the channel id to the extended channel.
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    // This is the upstream.
    upstream: Arc<Mutex<Upstream>>,
    // This is the receiver for messages from the upstream.
    upstream_receiver: Receiver<EitherFrame>,
    // This is a factory for the downstream id.
    downstream_id_factory: IdFactory,
    // This is a mapping of the downstream id to the downstream.
    pub downstreams: HashMap<u32, Arc<Mutex<Downstream>>>,
    // This is the receiver for messages from the downstream.
    downstream_receiver: Receiver<DownstreamMessages>,
    // This is the configuration of the proxy.
    pub proxy_config: TranslatorConfig,
}

impl ChannelManager {
    pub fn new(
        mode: ChannelMappingMode, 
        upstream: Arc<Mutex<Upstream>>,
        upstream_receiver: Receiver<EitherFrame>,
        downstream_receiver: Receiver<DownstreamMessages>,
        proxy_config: TranslatorConfig,
    ) -> Self {
        Self {
            mode,
            upstream,
            downstream_id_factory: IdFactory::new(),
            extended_channels: HashMap::new(),
            downstreams: HashMap::new(),
            upstream_receiver,
            downstream_receiver,
            proxy_config,
        }
    }

    pub fn on_new_sv1_connection(&mut self, user_identity: &str, hash_rate: f32, max_target: U256, min_extranonce_size: u16) -> Result<(), Error<'static>> {
        match self.mode {
            ChannelMappingMode::PerClient => {
                let downstream_id = self.downstream_id_factory.next();
                self.upstream.safe_lock(|u| u.open_extended_mining_channel(downstream_id, user_identity, hash_rate, max_target, min_extranonce_size))?;
                Ok(())
            }
            ChannelMappingMode::Aggregated => {
                // Here we need to open an extended mining channel to the upstream
                // if we don't have an existing channel, otherwise we need to use the single
                // already existing channel.
                if self.extended_channels.is_empty() {
                    let downstream_id = self.downstream_id_factory.next();
                    self.upstream.safe_lock(|u| u.open_extended_mining_channel(downstream_id, user_identity, hash_rate, max_target, min_extranonce_size))?;
                    Ok(())
                } else {
                    // here we need to create a unique extranonce for the new client
                    let downstream_id = self.downstream_id_factory.next();
                    Ok(())
                }
            }
        }
    }

    pub async fn handle_upstream_messages(self_: Arc<Mutex<Self>>) -> Result<(), Error<'static>> {
        let receiver = self_.safe_lock(|s| s.upstream_receiver.clone())?;
        loop {
            match receiver.recv().await {
                Ok(message) => {
                    let mut message: StdFrame = message.try_into()?;
                    let message_type = if let Some(header) = message.get_header() {
                        header.msg_type()
                    } else {
                        return Err(framing_sv2::Error::ExpectedHandshakeFrame.into());
                    };
                    // Gets the message payload
                    let payload = message.payload();
                    let result = ParseMiningMessagesFromUpstream::handle_message_mining(
                        self_.clone(),
                        message_type,
                        payload,
                    )?;
                    match result {
                        SendTo::None(None) => {}
                        SendTo::None(Some(NewExtendedMiningJob)) => {
                            self_.safe_lock(|s| s.upstream.clone())?.safe_lock(|u| u.send_upstream(m))?;
                        }
                        SendTo::Downstream(m) => {}
                    }
                }
                Err(e) => {
                    // Handle channel error
                    return Err(Error::ChannelErrorReceiver(e));
                }
            }
        }
    }

    pub async fn handle_downstream_messages(self_: Arc<Mutex<Self>>) -> Result<(), Error<'static>> {
        let receiver = self_.safe_lock(|s| s.downstream_receiver.clone())?;
        loop {
            match receiver.recv().await {
                Ok(message) => {
                    match message {
                        DownstreamMessages::SubmitShares(share) => {
                            let channel = self_.safe_lock(|s| s.extended_channels.get(&share.channel_id).unwrap().clone())?;
                            let channel = channel.read().unwrap();
                            let extended_share = SubmitSharesExtended {
                                channel_id: share.channel_id,
                                sequence_number: todo!(),
                                job_id: todo!(),
                                nonce: todo!(),
                                ntime: todo!(),
                                version: todo!(),
                                extranonce: todo!(),
                            };
                            match channel.validate_share(extended_share) {
                                Ok(_) => {
                                    // Forward the share to the upstream
                                    let upstream = self_.safe_lock(|s| s.upstream.clone())?.clone();
                                    upstream.safe_lock(|u| u.submit_shares_extended(extended_share))?;
                                }
                                Err(e) => {
                                    todo!()
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    // Handle channel error
                    return Err(Error::ChannelErrorReceiver(e));
                }
            }
        }
    }
}