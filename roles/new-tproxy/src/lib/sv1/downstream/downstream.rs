use super::DownstreamMessages;
use crate::{
    error::TproxyError, status::{handle_error, StatusSender}, sv1::downstream::{channel::DownstreamChannelState, data::DownstreamData}, task_manager::TaskManager, utils::ShutdownMessage
};
use async_channel::{Receiver, Sender};
use roles_logic_sv2::{mining_sv2::Target, utils::Mutex};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};
use v1::{
    json_rpc::{self, Message},
    server_to_client, IsServer,
};

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
        hashrate: f32,
    ) -> Self {
        let downstream_data = Arc::new(Mutex::new(DownstreamData::new(
            downstream_id,
            target,
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
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: StatusSender,
        task_manager: Arc<TaskManager>
    ) {
        let mut shutdown_rx = notify_shutdown.subscribe();
        let downstream_id = self.downstream_data.super_safe_lock(|d| d.downstream_id);

        info!("Downstream {downstream_id}: spawning unified task");

        task_manager.spawn(async move {
            loop {
                let sv1_server_receiver = self
                    .downstream_channel_state
                    .sv1_server_receiver
                    .subscribe();

                tokio::select! {
                    msg = shutdown_rx.recv() => {
                        match msg {
                            Ok(ShutdownMessage::ShutdownAll) => {
                                info!("Downstream {downstream_id}: received global shutdown");
                                break;
                            }
                            Ok(ShutdownMessage::DownstreamShutdown(id)) if id == downstream_id => {
                                info!("Downstream {downstream_id}: received targeted shutdown");
                                break;
                            }
                            Ok(ShutdownMessage::DownstreamShutdownAll) => {
                                info!("All downstream shutdown message received");
                                break;
                            }
                            Ok(_) => {
                                // shutdown for other downstream
                            }
                            Err(e) => {
                                warn!("Downstream {downstream_id}: shutdown channel closed: {e}");
                                break;
                            }
                        }
                    }

                    // Handle downstream -> server message
                    res = Self::handle_downstream_message(self.clone()) => {
                        if let Err(e) = res {
                            error!("Downstream {downstream_id}: error in downstream message handler: {e:?}");
                            handle_error(&status_sender, e).await;
                            break;
                        }
                    }

                    // Handle server -> downstream message
                    res = Self::handle_sv1_server_message(self.clone(), sv1_server_receiver) => {
                        if let Err(e) = res {
                            error!("Downstream {downstream_id}: error in server message handler: {e:?}");
                            handle_error(&status_sender, e).await;
                            break;
                        }
                    }

                    else => {
                        warn!("Downstream {downstream_id}: all channels closed; exiting task");
                        break;
                    }
                }
            }

            warn!("Downstream {downstream_id}: unified task shutting down");
            self.downstream_channel_state.drop();
            drop(shutdown_complete_tx);
        });
    }

    pub async fn handle_sv1_server_message(
        self: Arc<Self>,
        mut sv1_server_receiver: broadcast::Receiver<(u32, Option<u32>, json_rpc::Message)>,
    ) -> Result<(), TproxyError> {
        match sv1_server_receiver.recv().await {
            Ok((channel_id, downstream_id, message)) => {
                let (my_channel_id, my_downstream_id) = self
                    .downstream_data
                    .super_safe_lock(|d| (d.channel_id, d.downstream_id));

                let id_matches = (my_channel_id == Some(channel_id) || channel_id == 0)
                    && (downstream_id.is_none() || downstream_id == Some(my_downstream_id));

                if !id_matches {
                    return Ok(()); // Message not intended for this downstream
                }

                if let Message::Notification(notification) = &message {
                    match notification.method.as_str() {
                        "mining.set_difficulty" => {
                            info!("Down: Received set_difficulty notification, storing for next notify");
                            self.downstream_data.super_safe_lock(|d| {
                                d.pending_set_difficulty = Some(message.clone());
                            });
                            return Ok(()); // Defer sending until notify
                        }
                        "mining.notify" => {
                            let pending_set_difficulty = self
                                .downstream_data
                                .super_safe_lock(|d| d.pending_set_difficulty.clone());

                            if let Some(set_difficulty_msg) = &pending_set_difficulty {
                                info!("Down: Sending pending set_difficulty before notify");
                                self.downstream_channel_state
                                    .downstream_sv1_sender
                                    .send(set_difficulty_msg.clone())
                                    .await
                                    .map_err(|e| {
                                        error!(
                                            "Failed to send set_difficulty to downstream: {:?}",
                                            e
                                        );
                                        TproxyError::ChannelErrorSender
                                    })?;

                                self.downstream_data.super_safe_lock(|d| {
                                    if let Some(new_target) = d.pending_target.take() {
                                        d.target = new_target;
                                    }
                                    if let Some(new_hashrate) = d.pending_hashrate.take() {
                                        d.hashrate = new_hashrate;
                                    }
                                    d.pending_set_difficulty = None;
                                    debug!(
                                        "Downstream {}: Updated target and hashrate after sending set_difficulty",
                                        d.downstream_id
                                    );
                                });
                            }

                            if let Ok(mut notify) =
                                server_to_client::Notify::try_from(notification.clone())
                            {
                                let original_clean_jobs = notify.clean_jobs;

                                if pending_set_difficulty.is_some() {
                                    notify.clean_jobs = true;
                                    debug!(
                                        "Down: Sending notify with clean_jobs=true after set_difficulty"
                                    );
                                }

                                self.downstream_data.super_safe_lock(|d| {
                                    d.last_job_version_field = Some(notify.version.0);
                                    if original_clean_jobs {
                                        d.valid_jobs.clear();
                                    }
                                    d.valid_jobs.push(notify.clone());
                                    debug!("Updated valid jobs: {:?}", d.valid_jobs);
                                });

                                self.downstream_channel_state
                                    .downstream_sv1_sender
                                    .send(notify.into())
                                    .await
                                    .map_err(|e| {
                                        error!("Failed to send notify to downstream: {:?}", e);
                                        TproxyError::ChannelErrorSender
                                    })?;

                                return Ok(()); // Notify handled, don't fall through
                            }
                        }
                        _ => {} // Not a special message, proceed below
                    }
                }

                // Default path: forward all other messages
                self.downstream_channel_state
                    .downstream_sv1_sender
                    .send(message.clone())
                    .await
                    .map_err(|e| {
                        error!("Failed to send message to downstream: {:?}", e);
                        TproxyError::ChannelErrorSender
                    })?;
            }
            Err(e) => {
                let downstream_id = self.downstream_data.super_safe_lock(|d| d.downstream_id);
                error!(
                    "Sv1 message handler error for downstream {}: {:?}",
                    downstream_id, e
                );
                return Err(TproxyError::BroadcastChannelErrorReceiver(e));
            }
        }

        Ok(())
    }

    pub async fn handle_downstream_message(self: Arc<Self>) -> Result<(), TproxyError> {
        let message = match self
            .downstream_channel_state
            .downstream_sv1_receiver
            .recv()
            .await
        {
            Ok(msg) => msg,
            Err(e) => {
                error!("Error receiving downstream message: {:?}", e);
                return Err(TproxyError::ChannelErrorReceiver(e));
            }
        };

        let response = self
            .downstream_data
            .super_safe_lock(|data| data.handle_message(message));

        match response {
            Ok(Some(response_msg)) => {
                if let Some(_channel_id) = self.downstream_data.super_safe_lock(|d| d.channel_id) {
                    self.downstream_channel_state
                        .downstream_sv1_sender
                        .send(response_msg.into())
                        .await
                        .map_err(|e| {
                            error!("Failed to send message to downstream: {:?}", e);
                            TproxyError::ChannelErrorSender
                        })?;
                }
            }
            Ok(None) => {
                // Message was handled but no response needed
            }
            Err(e) => {
                error!("Error handling downstream message: {:?}", e);
                return Err(e.into());
            }
        }

        Ok(())
    }
}
