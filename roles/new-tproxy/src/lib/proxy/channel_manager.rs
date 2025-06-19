use crate::{config::TranslatorConfig, downstream_sv1::{downstream::Downstream, DownstreamMessages}, error::Error, upstream_sv2::{upstream::{EitherFrame, StdFrame}, Upstream}};
use roles_logic_sv2::{channels::client::extended::ExtendedChannel, handlers::mining::{ParseMiningMessagesFromUpstream, SendTo}, mining_sv2::{NewExtendedMiningJob, SubmitSharesExtended}, parsers::Mining, utils::{Id as IdFactory, Mutex}};
use std::{sync::{Arc, RwLock}, collections::HashMap};
use binary_sv2::U256;
use async_channel::{Receiver, Sender};

pub type Sv2Message = Mining<'static>;

/*#[derive(Debug, Clone)]
pub enum ChannelMappingMode {
    // This is the mode where each client has its own channel.
    PerClient,
    // This is the mode where all clients share the same channel.
    Aggregated,
}*/

#[derive(Debug, Clone)]
pub struct ChannelManager {
    // This is the mode of the channel mapping.
    // mode: ChannelMappingMode,
    // This is the sender for messages to the upstream.
    upstream_sender: Sender<Mining<'static>>,
    // This is the receiver for messages from the upstream.
    upstream_receiver: Receiver<Mining<'static>>,
    // This is a mapping of the channel id to the extended channel.
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    // This is a mapping of the downstream id to the downstream.
    pub downstreams: HashMap<u32, Arc<Mutex<Downstream>>>,
}

impl ChannelManager {
    pub fn new(
        // mode: ChannelMappingMode,
        upstream_sender: Sender<Mining<'static>>,
        upstream_receiver: Receiver<Mining<'static>>,
    ) -> Self {
        Self {
            // mode,
            upstream_sender,
            upstream_receiver,
            extended_channels: HashMap::new(),
            downstreams: HashMap::new(),
        }
    }
}