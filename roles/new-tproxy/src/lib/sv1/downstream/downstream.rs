use super::DownstreamMessages;
use crate::{error::TproxyError, handle_status_result, status::{handle_error, StatusSender}, utils::validate_sv1_share};
use async_channel::{Receiver, Sender};
use roles_logic_sv2::{
    common_properties::{CommonDownstreamData, IsDownstream, IsMiningDownstream},
    mining_sv2::Target,
    utils::Mutex,
    vardiff::classic::VardiffState,
    Vardiff,
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};
use v1::{
    client_to_server::{self, Submit},
    error::Error,
    json_rpc::{self, Message, Notification},
    server_to_client,
    utils::{Extranonce, HexU32Be, PrevHash},
    IsServer,
};

#[derive(Debug, Clone)]
pub struct DownstreamChannelState {
    downstream_sv1_sender: Sender<json_rpc::Message>,
    downstream_sv1_receiver: Receiver<json_rpc::Message>,
    sv1_server_sender: Sender<DownstreamMessages>,
    sv1_server_receiver: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>, /* channel_id, optional downstream_id, message */
}

impl DownstreamChannelState {
    fn new(
        downstream_sv1_sender: Sender<json_rpc::Message>,
        downstream_sv1_receiver: Receiver<json_rpc::Message>,
        sv1_server_sender: Sender<DownstreamMessages>,
        sv1_server_receiver: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>,
    ) -> Self {
        Self {
            downstream_sv1_receiver,
            downstream_sv1_sender,
            sv1_server_receiver,
            sv1_server_sender,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownstreamData {
    pub channel_id: Option<u32>,
    pub downstream_id: u32,
    pub extranonce1: Vec<u8>,
    pub extranonce2_len: usize,
    pub version_rolling_mask: Option<HexU32Be>,
    pub version_rolling_min_bit: Option<HexU32Be>,
    pub last_job_version_field: Option<u32>,
    pub authorized_worker_names: Vec<String>,
    pub user_identity: String,
    pub valid_jobs: Vec<server_to_client::Notify<'static>>,
    pub target: Target,
    pub hashrate: f32,
    pub pending_set_difficulty: Option<json_rpc::Message>,
    pub pending_target: Option<Target>,
    pub pending_hashrate: Option<f32>,
    pub sv1_server_sender: Sender<DownstreamMessages>, // just here for time being
}

impl DownstreamData {
    fn new(
        downstream_id: u32,
        target: Target,
        shares_per_minute: f32,
        hashrate: f32,
        sv1_server_sender: Sender<DownstreamMessages>,
    ) -> Self {
        DownstreamData {
            channel_id: None,
            downstream_id: downstream_id,
            extranonce1: vec![0; 8],
            extranonce2_len: 4,
            version_rolling_mask: None,
            version_rolling_min_bit: None,
            last_job_version_field: None,
            authorized_worker_names: Vec::new(),
            user_identity: String::new(),
            valid_jobs: Vec::new(),
            target,
            hashrate: hashrate,
            pending_set_difficulty: None,
            pending_target: None,
            pending_hashrate: None,
            sv1_server_sender,
        }
    }

    pub fn set_pending_target_and_hashrate(&mut self, new_target: Target, new_hashrate: f32) {
        self.pending_target = Some(new_target);
        self.pending_hashrate = Some(new_hashrate);
        debug!(
            "Downstream {}: Set pending target and hashrate",
            self.downstream_id
        );
    }
}

#[derive(Debug, Clone)]
pub struct Downstream {
    pub downstream_data: Arc<Mutex<DownstreamData>>,
    downstream_channel_state: DownstreamChannelState,
}

impl Downstream {
    pub fn new(
        downstream_id: u32,
        downstream_sv1_sender: Sender<json_rpc::Message>,
        downstream_sv1_receiver: Receiver<json_rpc::Message>,
        sv1_server_sender: Sender<DownstreamMessages>,
        sv1_server_receiver: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>,
        target: Target,
        shares_per_minute: f32,
        hashrate: f32,
    ) -> Self {
        let downstream_data = Arc::new(Mutex::new(DownstreamData::new(
            downstream_id,
            target,
            shares_per_minute,
            hashrate,
            sv1_server_sender.clone(),
        )));
        let downstream_channel_state = DownstreamChannelState::new(
            downstream_sv1_sender,
            downstream_sv1_receiver,
            sv1_server_sender,
            sv1_server_receiver,
        );
        Self {
            downstream_data,
            downstream_channel_state,
        }
    }

    pub fn run_downstream_tasks(
        self: Arc<Self>,
        notify_shutdown: broadcast::Sender<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: StatusSender
    ) {
        let mut shutdown_rx = notify_shutdown.subscribe();
        info!("Spawning downstream tasks");
        tokio::spawn(async move {
            loop {
                let mut sv1_server_receiver = self
                    .downstream_channel_state
                    .sv1_server_receiver
                    .subscribe();
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Downstream: received shutdown signal");
                        break;
                    }
                    res = Self::handle_downstream_message(self.clone()) => {
                        if let Err(e) = res {
                            handle_error(&status_sender, e);
                            break;
                        }
                    },
                    res = Self::handle_sv1_server_message(self.clone(), sv1_server_receiver) => {
                        if let Err(e) = res {
                            handle_error(&status_sender, e);
                            break;
                        }
                    },
                    else => {
                        warn!("Downstream: all channels closed, exiting loop");
                        break;
                    }
                }
            }

            drop(shutdown_complete_tx);
            warn!("Downstream: unified task exited");
        });
    }

    pub async fn handle_sv1_server_message(
        self: Arc<Self>,
        mut sv1_server_receiver: broadcast::Receiver<(u32, Option<u32>, json_rpc::Message)>,
    ) -> Result<(), TproxyError> {
        match sv1_server_receiver.recv().await {
            Ok((channel_id, downstream_id, message)) => {
                if let Some(downstream_channel_id) =
                    self.downstream_data.super_safe_lock(|d| d.channel_id)
                {
                    if downstream_channel_id == channel_id
                        && (downstream_id.is_none()
                            || downstream_id
                                == Some(self.downstream_data.super_safe_lock(|d| d.downstream_id)))
                    {
                        // Handle set_difficulty notification
                        if let Message::Notification(notification) = &message {
                            if notification.method == "mining.set_difficulty" {
                                debug!("Down: Received set_difficulty notification, storing for next notify");
                                self.downstream_data.super_safe_lock(|d| {
                                    d.pending_set_difficulty = Some(message.clone());
                                });
                                return Ok(()); // Don't send set_difficulty immediately, wait for
                                                 // next notify
                            }
                        }

                        // Handle notify notification
                        if let Message::Notification(notification) = &message {
                            if notification.method == "mining.notify" {
                                // Check if we have a pending set_difficulty
                                let pending_set_difficulty = self
                                    .downstream_data
                                    .super_safe_lock(|d| d.pending_set_difficulty.clone());

                                // If we have a pending set_difficulty, send it first
                                if let Some(set_difficulty_msg) = &pending_set_difficulty {
                                    debug!("Down: Sending pending set_difficulty before notify");
                                    if let Err(e) = self
                                        .downstream_channel_state
                                        .downstream_sv1_sender
                                        .send(set_difficulty_msg.clone())
                                        .await
                                    {
                                        error!(
                                            "Failed to send set_difficulty to downstream: {:?}",
                                            e
                                        );
                                        return Err(TproxyError::ChannelErrorSender);
                                    } else {
                                        // Update target and hashrate after successful send
                                        self.downstream_data.super_safe_lock(|d| {
                                            if let Some(new_target) = d.pending_target.take() {
                                                d.target = new_target;
                                            }
                                            if let Some(new_hashrate) = d.pending_hashrate.take() {
                                                d.hashrate = new_hashrate;
                                            }
                                            debug!("Downstream {}: Updated target and hashrate after sending set_difficulty", d.downstream_id);
                                        });
                                    }
                                    // Clear the pending set_difficulty
                                    self.downstream_data
                                        .super_safe_lock(|d| d.pending_set_difficulty = None);
                                }

                                // Now handle the notify
                                if let Ok(mut notify) =
                                    server_to_client::Notify::try_from(notification.clone())
                                {
                                    // Check the original clean_jobs value before modifying it
                                    let original_clean_jobs = notify.clean_jobs;

                                    // Set clean_jobs to true if we had a pending set_difficulty
                                    if pending_set_difficulty.is_some() {
                                        notify.clean_jobs = true;
                                        debug!("Down: Sending notify with clean_jobs=true after set_difficulty");
                                    }

                                    // Update the downstream's job tracking
                                    self.downstream_data.super_safe_lock(|d| {
                                        d.last_job_version_field = Some(notify.version.0);
                                        if original_clean_jobs {
                                            d.valid_jobs.clear();
                                            d.valid_jobs.push(notify.clone());
                                        } else {
                                            d.valid_jobs.push(notify.clone());
                                        }
                                        debug!("Updated valid jobs: {:?}", d.valid_jobs);
                                    });

                                    // Send the notify to downstream
                                    if let Err(e) = self
                                        .downstream_channel_state
                                        .downstream_sv1_sender
                                        .send(notify.into())
                                        .await
                                    {
                                        error!("Failed to send notify to downstream: {:?}", e);
                                        return Err(TproxyError::ChannelErrorSender);
                                    }
                                }
                                return Ok(()); // We've handled the notify specially, don't send
                                                 // it again below
                            }
                        }

                        // For all other messages, send them normally
                        if let Err(e) = self
                            .downstream_channel_state
                            .downstream_sv1_sender
                            .send(message.clone())
                            .await
                        {
                            error!("Failed to send message to downstream: {:?}", e);
                            /// This could mean sv1 server is down
                            return Err(TproxyError::ChannelErrorSender);
                        } else {
                            // If this was a set_difficulty message, update the target and hashrate
                            // from pending values
                            if let Message::Notification(notification) = &message {
                                if notification.method == "mining.set_difficulty" {
                                    self.downstream_data.super_safe_lock(|d| {
                                        if let Some(new_target) = d.pending_target.take() {
                                            d.target = new_target;
                                        }
                                        if let Some(new_hashrate) = d.pending_hashrate.take() {
                                            d.hashrate = new_hashrate;
                                        }
                                        debug!("Downstream {}: Updated target and hashrate after sending direct set_difficulty", d.downstream_id);
                                    });
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Something went wrong in Sv1 message handler in downstream {}: {:?}",self.downstream_data.super_safe_lock(|d| d.downstream_id), e);
                return Err(TproxyError::BroadcastChannelErrorReceiver(e));
            }
        }
        Ok(())
    }

    pub async fn handle_downstream_message(self: Arc<Self>) -> Result<(), TproxyError> {
        match self
            .downstream_channel_state
            .downstream_sv1_receiver
            .recv()
            .await
        {
            Ok(message) => {
                let response = self
                    .downstream_data
                    .super_safe_lock(|downstream_data| downstream_data.handle_message(message));
                if let Ok(Some(response)) = response {
                    if let Some(channel_id) = self.downstream_data.super_safe_lock(|d| d.channel_id)
                    {
                        if let Err(e) = self
                            .downstream_channel_state
                            .downstream_sv1_sender
                            .send(response.into())
                            .await
                        {
                            error!("Failed to send message to downstream: {:?}", e);
                            return Err(TproxyError::ChannelErrorSender);
                        }
                    }
                }
            }
            Err(e) => {
                error!(
                    "Something went wrong in downstream message handler: {:?}",
                    e
                );
                return Err(TproxyError::ChannelErrorReceiver(e));
            }
        }
        Ok(())
    }
}
