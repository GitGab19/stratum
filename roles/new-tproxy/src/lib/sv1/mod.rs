//! ## Downstream SV1 Module
//!
//! This module defines the structures, messages, and utility functions
//! used for handling the downstream connection with SV1 mining clients.
//!
//! It includes definitions for messages exchanged with a Bridge component,
//! structures for submitting shares and updating targets, and constants
//! and functions for managing client interactions.
//!
//! The module is organized into the following sub-modules:
//! - [`diff_management`]: (Declared here, likely contains downstream difficulty logic)
//! - [`downstream`]: Defines the core [`Downstream`] struct and its functionalities.

use v1::{client_to_server::Submit, utils::HexU32Be};
pub mod downstream;
pub mod sv1_server;
pub mod translation_utils;
pub use downstream::Downstream;
pub use sv1_server::Sv1Server;

/// The messages that are sent from the downstream handling logic
/// to a central "Bridge" component for further processing.
#[derive(Debug)]
pub enum DownstreamMessages {
    /// Represents a submitted share from a downstream miner,
    /// wrapped with the relevant channel ID.
    SubmitShares(SubmitShareWithChannelId),
}

/// wrapper around a `mining.submit` with extra channel information for the Bridge to
/// process
#[derive(Debug)]
pub struct SubmitShareWithChannelId {
    pub channel_id: u32,
    pub downstream_id: u32,
    pub share: Submit<'static>,
    pub extranonce: Vec<u8>,
    pub extranonce2_len: usize,
    pub version_rolling_mask: Option<HexU32Be>,
    pub last_job_version: Option<u32>,
}

/// This is just a wrapper function to send a message on the Downstream task shutdown channel
/// it does not matter what message is sent because the receiving ends should shutdown on any
/// message
pub async fn kill(sender: &async_channel::Sender<bool>) {
    // safe to unwrap since the only way this can fail is if all receiving channels are dropped
    // meaning all tasks have already dropped
    sender.send(true).await.unwrap();
}

/// Generates a new, hardcoded string intended to be used as a subscription ID.
///
/// FIXME
pub fn new_subscription_id() -> String {
    "ae6812eb4cd7735a302a8a9dd95cf71f".into()
}
