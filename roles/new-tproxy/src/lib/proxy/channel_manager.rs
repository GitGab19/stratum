use crate::{downstream_sv1::downstream::Downstream, error::Error, upstream_sv2::upstream::StdFrame};
use async_channel::{Receiver, Sender};
use binary_sv2::u256_from_int;
use roles_logic_sv2::{channels::client::extended::ExtendedChannel, parsers::{Mining, AnyMessage}, utils::Mutex, mining_sv2::OpenExtendedMiningChannel, handlers::mining::ParseMiningMessagesFromUpstream};
use tracing::error;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

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
    /*// This is a mapping of the downstream id to the downstream.
    pub downstreams: HashMap<u32, Arc<Mutex<Downstream>>>,*/
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
            //downstreams: HashMap::new(),
        }
    }

    pub async fn on_upstream_message(&mut self) -> Result<(), Error> {
        while let Ok(message) = self.upstream_receiver.recv().await {
            let mut frame: StdFrame =
                AnyMessage::Mining(message).try_into().map_err(|e| {
                    error!("Failed to parse common message: {:?}", e);
                    e
                })?;
            let message_type = frame.get_header().unwrap().msg_type();
            let payload = frame.payload();
            let self_mutex = Arc::new(Mutex::new(self.clone()));
            ParseMiningMessagesFromUpstream::handle_message_mining(self_mutex, message_type, payload)?;
        }
        Ok(())
    }

    pub async fn create_channel(&mut self, downstream_id: u32, workername: String) -> Result<(), Error> {
        let open_channel = Mining::OpenExtendedMiningChannel(OpenExtendedMiningChannel {
            request_id: downstream_id,
            user_identity: workername.try_into()?,
            nominal_hash_rate: 1000.0, // TODO
            max_target: u256_from_int(u64::MAX), // TODO
            min_extranonce_size: 4, // TODO
        });
        self.upstream_sender.send(open_channel).await.map_err(|e| {
            // TODO: Handle this error
            error!("Failed to send open channel message to upstream: {:?}", e);
            e
        });
        Ok(())
    }

}
