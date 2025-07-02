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
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

pub use v1::server_to_client;

use config::TranslatorConfig;

use crate::{
    status::{State, Status},
    sv1::sv1_server::Sv1Server,
    sv2::{channel_manager::ChannelMode, ChannelManager, Upstream},
    utils::ShutdownMessage,
};

pub mod config;
pub mod error;
pub mod handle_result;
pub mod status;
pub mod sv1;
pub mod sv2;
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
        Self { config }
    }

    /// Starts the translator.
    ///
    /// This method starts the main event loop, which handles connections,
    /// protocol translation, job management, and status reporting.
    pub async fn start(self) {
        let (notify_shutdown, _) = tokio::sync::broadcast::channel::<ShutdownMessage>(1);
        let (shutdown_complete_tx, mut shutdown_complete_rx) = mpsc::channel::<()>(1);

        let (status_sender, status_receiver) = async_channel::unbounded::<Status>();

        let (channel_manager_to_upstream_sender, channel_manager_to_upstream_receiver) =
            unbounded();

        let (upstream_to_channel_manager_sender, upstream_to_channel_manager_receiver) =
            unbounded();

        let (channel_manager_to_sv1_server_sender, channel_manager_to_sv1_server_receiver) =
            unbounded();

        let (sv1_server_to_channel_manager_sender, sv1_server_to_channel_manager_receiver) =
            unbounded();

        let upstream_addr = SocketAddr::new(
            self.config.upstream_address.parse().unwrap(),
            self.config.upstream_port,
        );

        info!("Connecting to upstream at: {}", upstream_addr);

        let upstream = match Upstream::new(
            upstream_addr,
            self.config.upstream_authority_pubkey,
            upstream_to_channel_manager_sender.clone(),
            channel_manager_to_upstream_receiver.clone(),
            notify_shutdown.clone(),
            shutdown_complete_tx.clone(),
        )
        .await
        {
            Ok(upstream) => upstream,
            Err(e) => {
                error!("Failed to initialize upstream connection: {:?}", e);
                return;
            }
        };

        let channel_manager = Arc::new(
            (ChannelManager::new(
                channel_manager_to_upstream_sender,
                upstream_to_channel_manager_receiver,
                channel_manager_to_sv1_server_sender.clone(),
                sv1_server_to_channel_manager_receiver,
                if !self.config.aggregate_channels {
                    ChannelMode::Aggregated
                } else {
                    ChannelMode::NonAggregated
                },
            )),
        );

        let downstream_addr: SocketAddr = SocketAddr::new(
            self.config.downstream_address.parse().unwrap(),
            self.config.downstream_port,
        );

        let mut sv1_server = Arc::new(Sv1Server::new(
            downstream_addr,
            channel_manager_to_sv1_server_receiver,
            sv1_server_to_channel_manager_sender,
            self.config.clone(),
        ));

        ChannelManager::run_channel_manager_tasks(
            channel_manager.clone(),
            notify_shutdown.clone(),
            shutdown_complete_tx.clone(),
            status_sender.clone(),
        )
        .await;

        if let Err(e) = upstream
            .start(
                notify_shutdown.clone(),
                shutdown_complete_tx.clone(),
                status_sender.clone(),
            )
            .await
        {
            error!("Failed to start upstream listener: {:?}", e);
            return;
        }
        let notify_shutdown_clone = notify_shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        info!("Ctrl+c received. Intiating graceful shutdown...");
                        notify_shutdown_clone.send(ShutdownMessage::ShutdownAll).unwrap();
                        break;
                    }
                    message = status_receiver.recv() => {
                        error!("I received some error: {message:?}");
                        match message {
                            Ok(status) => {
                                match status.state {
                                    State::DownstreamShutdown{downstream_id,..} => {
                                        notify_shutdown_clone.send(ShutdownMessage::DownstreamShutdown(downstream_id)).unwrap();
                                    }
                                    State::Sv1ServerShutdown(_) => {
                                        notify_shutdown_clone.send(ShutdownMessage::ShutdownAll).unwrap();
                                        break;
                                    }
                                    State::ChannelManagerShutdown(_) => {
                                        notify_shutdown_clone.send(ShutdownMessage::ShutdownAll).unwrap();
                                        break;
                                    }
                                    State::UpstreamShutdown(_) => {
                                        notify_shutdown_clone.send(ShutdownMessage::ShutdownAll).unwrap();
                                        break;
                                    }
                                    State::Healthy(_) => {
                                    }

                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Sv1Server::start(
            sv1_server,
            notify_shutdown.clone(),
            shutdown_complete_tx.clone(),
            status_sender.clone(),
        )
        .await;

        drop(shutdown_complete_tx);
        info!("waiting for shutdown complete...");
        let shutdown_timeout = tokio::time::Duration::from_secs(30);
        tokio::select! {
            _ = shutdown_complete_rx.recv() => {
                info!("All tasks reported shutdown complete.");
            }
            _ = tokio::time::sleep(shutdown_timeout) => {
                warn!("Graceful shutdown timed out after {:?}. Some tasks might still be running.", shutdown_timeout);
            }
        }
    }
}
