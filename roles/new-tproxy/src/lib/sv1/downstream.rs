use super::DownstreamMessages;
use crate::{sv1::SubmitShareWithChannelId, utils::validate_sv1_share};
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
pub struct DownstreamChannelManager {
    downstream_sv1_sender: Sender<json_rpc::Message>,
    downstream_sv1_receiver: Receiver<json_rpc::Message>,
    sv1_server_sender: Sender<DownstreamMessages>,
    sv1_server_receiver: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>, /* channel_id, optional downstream_id, message */
}

impl DownstreamChannelManager {
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
    downstream_channel_manager: DownstreamChannelManager,
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
        let downstream_channel_manager = DownstreamChannelManager::new(
            downstream_sv1_sender,
            downstream_sv1_receiver,
            sv1_server_sender,
            sv1_server_receiver,
        );
        Self {
            downstream_data,
            downstream_channel_manager,
        }
    }

    pub fn spawn_downstream_receiver(
        self,
        notify_shutdown: broadcast::Sender<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) {
        let mut notify_shutdown = notify_shutdown.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = notify_shutdown.recv() => {
                        info!("Downstream: downstream receiver loop received shutdown signal. Exiting.");
                        break;
                    }
                    message = self.downstream_channel_manager.downstream_sv1_receiver.recv() => {
                        match message {
                            Ok(message) => {
                                let response = self.downstream_data.super_safe_lock(|downstream_data| downstream_data.handle_message(message));
                                if let Ok(Some(response)) = response {
                                    if let Some(channel_id) = self.downstream_data.super_safe_lock(|d| d.channel_id) {
                                        if let Err(e) = self.downstream_channel_manager.downstream_sv1_sender.send(response.into()).await
                                        {
                                            error!("Failed to send message to downstream: {:?}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                break;
                            }
                        }
                    }
                }
            }
            drop(shutdown_complete_tx);
            warn!("Downstream: downstream receiver loop exited.");
        });
    }

    pub fn spawn_downstream_sender(
        self,
        notify_shutdown: broadcast::Sender<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) {
        let mut sv1_server_receiver = self
            .downstream_channel_manager
            .sv1_server_receiver
            .subscribe();
        let mut notify_shutdown = notify_shutdown.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = notify_shutdown.recv() => {
                        info!("Downstream: downstream sender loop received shutdown signal. Exiting.");
                        break;
                    }
                    message = sv1_server_receiver.recv() => {
                        match  message {
                            Ok((channel_id, downstream_id, message)) => {
                                if let Some(downstream_channel_id) = self.downstream_data.super_safe_lock(|d| d.channel_id) {
                                    if downstream_channel_id == channel_id && (downstream_id.is_none() || downstream_id == Some(self.downstream_data.super_safe_lock(|d| d.downstream_id))) {
                                        // Handle set_difficulty notification
                                        if let Message::Notification(notification) = &message {
                                            if notification.method == "mining.set_difficulty" {
                                                debug!("Down: Received set_difficulty notification, storing for next notify");
                                                self.downstream_data.super_safe_lock(|d| {
                                                    d.pending_set_difficulty = Some(message.clone());
                                                });
                                                continue; // Don't send set_difficulty immediately, wait for next notify
                                            }
                                        }

                                        // Handle notify notification
                                        if let Message::Notification(notification) = &message {
                                            if notification.method == "mining.notify" {
                                                // Check if we have a pending set_difficulty
                                                let pending_set_difficulty = self.downstream_data.super_safe_lock(|d| d.pending_set_difficulty.clone());

                                                // If we have a pending set_difficulty, send it first
                                                if let Some(set_difficulty_msg) = &pending_set_difficulty {
                                                    debug!("Down: Sending pending set_difficulty before notify");
                                                    if let Err(e) = self.downstream_channel_manager.downstream_sv1_sender
                                                        .send(set_difficulty_msg.clone())
                                                        .await
                                                    {
                                                        error!("Failed to send set_difficulty to downstream: {:?}", e);
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
                                                    self.downstream_data.super_safe_lock(|d| d.pending_set_difficulty = None);
                                                }

                                                // Now handle the notify
                                                if let Ok(mut notify) = server_to_client::Notify::try_from(notification.clone()) {
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
                                                    if let Err(e) = self.downstream_channel_manager.downstream_sv1_sender
                                                        .send(notify.into())
                                                        .await
                                                    {
                                                        error!("Failed to send notify to downstream: {:?}", e);
                                                    }
                                                }
                                                continue; // We've handled the notify specially, don't send it again below
                                            }
                                        }

                                        // For all other messages, send them normally
                                        if let Err(e) = self.downstream_channel_manager.downstream_sv1_sender
                                            .send(message.clone())
                                            .await
                                        {
                                            error!("Failed to send message to downstream: {:?}", e);
                                        } else {
                                            // If this was a set_difficulty message, update the target and hashrate from pending values
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
                            },
                            Err(e) => {
                                break;
                            }
                        }
                    }
                }
            }
            drop(shutdown_complete_tx);
            warn!("Downstream: downstream sender loop exited");
        });
    }
}

// Implements `IsServer` for `Downstream` to handle the SV1 messages.
impl IsServer<'static> for DownstreamData {
    fn handle_configure(
        &mut self,
        request: &client_to_server::Configure,
    ) -> (Option<server_to_client::VersionRollingParams>, Option<bool>) {
        info!("Down: Configuring");
        debug!("Down: Handling mining.configure: {:?}", &request);
        self.version_rolling_mask = request
            .version_rolling_mask()
            .map(|mask| HexU32Be(mask & 0x1FFFE000));
        self.version_rolling_min_bit = request.version_rolling_min_bit_count();

        debug!(
            "Negotiated version_rolling_mask is {:?}",
            self.version_rolling_mask
        );
        (
            Some(server_to_client::VersionRollingParams::new(
                self.version_rolling_mask.clone().unwrap_or(HexU32Be(0)),
                self.version_rolling_min_bit.clone().unwrap_or(HexU32Be(0)),
            ).expect("Version mask invalid, automatic version mask selection not supported, please change it in carte::downstream_sv1::mod.rs")),
            Some(false),
        )
    }

    fn handle_subscribe(&self, request: &client_to_server::Subscribe) -> Vec<(String, String)> {
        info!("Down: Subscribing");
        debug!("Down: Handling mining.subscribe: {:?}", &request);

        let set_difficulty_sub = (
            "mining.set_difficulty".to_string(),
            self.downstream_id.to_string(),
        );

        let notify_sub = (
            "mining.notify".to_string(),
            "ae6812eb4cd7735a302a8a9dd95cf71f".to_string(),
        );

        vec![set_difficulty_sub, notify_sub]
    }

    fn handle_authorize(&self, request: &client_to_server::Authorize) -> bool {
        info!("Down: Authorizing");
        debug!("Down: Handling mining.authorize: {:?}", &request);
        true
    }

    fn handle_submit(&self, request: &client_to_server::Submit<'static>) -> bool {
        if let Some(channel_id) = self.channel_id {
            let is_valid_share = validate_sv1_share(
                request,
                self.target.clone(),
                self.extranonce1.clone(),
                self.version_rolling_mask.clone(),
                &self.valid_jobs,
            )
            .unwrap_or(false);
            if !is_valid_share {
                return false;
            }
            let to_send: SubmitShareWithChannelId = SubmitShareWithChannelId {
                channel_id,
                downstream_id: self.downstream_id,
                share: request.clone(),
                extranonce: self.extranonce1.clone(),
                extranonce2_len: self.extranonce2_len,
                version_rolling_mask: self.version_rolling_mask.clone(),
                last_job_version: self.last_job_version_field.clone(),
            };
            if let Err(e) = self
                .sv1_server_sender
                .try_send(DownstreamMessages::SubmitShares(to_send))
            {
                error!("Failed to send share to SV1 server: {:?}", e);
            }
            true
        } else {
            error!("Cannot submit share: channel_id is None (waiting for OpenExtendedMiningChannelSuccess)");
            false
        }
    }

    /// Indicates to the server that the client supports the mining.set_extranonce method.
    fn handle_extranonce_subscribe(&self) {}

    /// Checks if a Downstream role is authorized.
    fn is_authorized(&self, name: &str) -> bool {
        self.authorized_worker_names.contains(&name.to_string())
    }

    /// Authorizes a Downstream role.
    fn authorize(&mut self, name: &str) {
        self.authorized_worker_names.push(name.to_string());
    }

    /// Sets the `extranonce1` field sent in the SV1 `mining.notify` message to the value specified
    /// by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce1(
        &mut self,
        _extranonce1: Option<Extranonce<'static>>,
    ) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().unwrap()
    }

    /// Returns the `Downstream`'s `extranonce1` value.
    fn extranonce1(&self) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().unwrap()
    }

    /// Sets the `extranonce2_size` field sent in the SV1 `mining.notify` message to the value
    /// specified by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce2_size(&mut self, _extra_nonce2_size: Option<usize>) -> usize {
        self.extranonce2_len
    }

    /// Returns the `Downstream`'s `extranonce2_size` value.
    fn extranonce2_size(&self) -> usize {
        self.extranonce2_len
    }

    /// Returns the version rolling mask.
    fn version_rolling_mask(&self) -> Option<HexU32Be> {
        self.version_rolling_mask.clone()
    }

    /// Sets the version rolling mask.
    fn set_version_rolling_mask(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_mask = mask;
    }

    /// Sets the minimum version rolling bit.
    fn set_version_rolling_min_bit(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_min_bit = mask
    }

    fn notify(&mut self) -> Result<json_rpc::Message, v1::error::Error> {
        unreachable!()
    }
}

// Can we remove this?
impl IsMiningDownstream for Downstream {}
// Can we remove this?
impl IsDownstream for Downstream {
    fn get_downstream_mining_data(
        &self,
    ) -> roles_logic_sv2::common_properties::CommonDownstreamData {
        todo!()
    }
}
