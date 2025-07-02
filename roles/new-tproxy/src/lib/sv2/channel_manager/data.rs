use roles_logic_sv2::{
    channels::client::extended::ExtendedChannel, mining_sv2::ExtendedExtranonce, utils::Mutex,
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub enum ChannelMode {
    Aggregated,
    NonAggregated,
}

#[derive(Debug, Clone)]
pub struct ChannelManagerData {
    // Store pending channel info by downstream_id
    pub pending_channels: HashMap<u32, (String, f32, usize)>, /* (user_identity, hashrate,
                                                               * downstream_extranonce_len) */
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    pub upstream_extended_channel: Option<Arc<RwLock<ExtendedChannel<'static>>>>, /* This is the upstream extended channel that is used in aggregated mode */
    pub extranonce_prefix_factory: Option<Arc<Mutex<ExtendedExtranonce>>>,        /* This is the
                                                                                   * extranonce
                                                                                   * prefix
                                                                                   * factory that is
                                                                                   * used in aggregated
                                                                                   * mode to allocate
                                                                                   * unique extranonce
                                                                                   * prefixes */

    pub mode: ChannelMode,
}

impl ChannelManagerData {
    pub fn new(mode: ChannelMode) -> Self {
        Self {
            pending_channels: HashMap::new(),
            extended_channels: HashMap::new(),
            upstream_extended_channel: None,
            extranonce_prefix_factory: None,
            mode,
        }
    }
}
