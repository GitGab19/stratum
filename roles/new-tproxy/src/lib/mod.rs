//! ## Translator Sv2
//!
//! Provides the core logic and main struct (`TranslatorSv2`) for running a
//! Stratum V1 to Stratum V2 translation proxy.
//!
//! This module orchestrates the interaction between downstream SV1 miners and upstream SV2
//! applications (proxies or pool servers).
//!
//! The central component is the `TranslatorSv2` struct, which encapsulates the state and
//! provides the `start` method as the main entry point for running the translator service.
//! It relies on several sub-modules (`config`, `downstream_sv1`, `upstream_sv2`, `proxy`, `status`,
//! etc.) for specialized functionalities.
#![allow(warnings)]
use async_channel::unbounded;
pub use roles_logic_sv2::utils::Mutex;
use std::{net::SocketAddr, sync::Arc};
use tracing::{error, info};

pub use v1::server_to_client;

use config::TranslatorConfig;

use crate::{
    proxy::{sv1_server::Sv1Server, ChannelManager},
    upstream_sv2::Upstream,
};

pub mod config;
pub mod downstream_sv1;
pub mod error;
pub mod proxy;
pub mod status;
pub mod upstream_sv2;
pub mod utils;

/// The main struct that manages the SV1/SV2 translator.
#[derive(Clone, Debug)]
pub struct TranslatorSv2 {
    config: TranslatorConfig,
}

impl TranslatorSv2 {
    /// Creates a new `TranslatorSv2`.
    ///
    /// Initializes the translator with the given configuration and sets up
    /// the reconnect wait time.
    pub fn new(config: TranslatorConfig) -> Self {
        info!("TranslatorSv2 created with config: {:?}", config);
        Self { config }
    }

    /// Starts the translator.
    ///
    /// This method starts the main event loop, which handles connections,
    /// protocol translation, job management, and status reporting.
    pub async fn start(self) {
        info!("Starting TranslatorSv2 service.");

        let (channel_manager_sender, channel_manager_receiver) = unbounded();

        let (sv1_server_sender, sv1_server_receiver) = unbounded();

        let (channel_opener_sender, channel_opener_receiver) = unbounded();

        let upstream_addr = SocketAddr::new(
            self.config.upstream_address.parse().unwrap(),
            self.config.upstream_port,
        );

        info!("Connecting to upstream at: {}", upstream_addr);

        let mut upstream = match Upstream::new(
            upstream_addr,
            self.config.upstream_authority_pubkey,
            channel_manager_sender,
            channel_manager_receiver,
        )
        .await
        {
            Ok(upstream) => {
                info!("Successfully initialized upstream connection.");
                upstream
            }
            Err(e) => {
                error!("Failed to initialize upstream connection: {:?}", e);
                return;
            }
        };

        let (upstream_sender, upstream_receiver) = unbounded();
        let channel_manager = ChannelManager::new(
            upstream_sender,
            upstream_receiver,
            sv1_server_sender,
            channel_opener_receiver,
        );

        let (downstream_sender, downstream_receiver) = unbounded();
        let downstream_addr: SocketAddr = SocketAddr::new(
            self.config.downstream_address.parse().unwrap(),
            self.config.downstream_port,
        );

        info!("Starting downstream SV1 server at: {}", downstream_addr);

        let mut sv1_server = Sv1Server::new(
            Arc::new(Mutex::new(channel_manager)),
            downstream_sender,
            downstream_receiver,
            downstream_addr,
            sv1_server_receiver,
            channel_opener_sender,
        );

        info!("Starting upstream listener task.");

        if let Err(e) = upstream.start().await {
            error!("Failed to start upstream listener: {:?}", e);
            return;
        }

        info!("Starting downstream SV1 server listener.");
        sv1_server.start().await;

        info!("TranslatorSv2 service started successfully.");
    }
}
