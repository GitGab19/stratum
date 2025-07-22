use roles_logic_sv2::{
    channels::client::extended::ExtendedChannel, mining_sv2::ExtendedExtranonce, utils::Mutex,
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

/// Defines the operational mode for channel management.
///
/// The channel manager can operate in two different modes that affect how
/// downstream connections are mapped to upstream SV2 channels:
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub enum ChannelMode {
    /// All downstream connections share a single extended SV2 channel.
    /// This mode uses extranonce prefix allocation to distinguish between
    /// different downstream miners while presenting them as a single entity
    /// to the upstream server. This is more efficient for pools with many
    /// miners.
    Aggregated,
    /// Each downstream connection gets its own dedicated extended SV2 channel.
    /// This mode provides complete isolation between downstream connections
    /// but may be less efficient for large numbers of miners.
    NonAggregated,
}

/// Internal data structure for the ChannelManager.
///
/// This struct maintains all the state needed for SV2 channel management,
/// including pending channel requests, active channels, and mode-specific
/// data structures like extranonce factories for aggregated mode.
#[derive(Debug, Clone)]
pub struct ChannelManagerData {
    /// Store pending channel info by downstream_id: (user_identity, hashrate, downstream_extranonce_len)
    pub pending_channels: HashMap<u32, (String, f32, usize)>,
    /// Map of active extended channels by channel ID
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    /// The upstream extended channel used in aggregated mode
    pub upstream_extended_channel: Option<Arc<RwLock<ExtendedChannel<'static>>>>,
    /// Extranonce prefix factory for allocating unique prefixes in aggregated mode
    pub extranonce_prefix_factory: Option<Arc<Mutex<ExtendedExtranonce>>>,
    /// Current operational mode
    pub mode: ChannelMode,
}

impl ChannelManagerData {
    /// Creates a new ChannelManagerData instance.
    ///
    /// # Arguments
    /// * `mode` - The operational mode (Aggregated or NonAggregated)
    ///
    /// # Returns
    /// A new ChannelManagerData instance with empty state
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
