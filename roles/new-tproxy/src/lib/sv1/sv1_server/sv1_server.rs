use crate::{
    config::TranslatorConfig,
    error::TproxyError,
    status::{handle_error, Status, StatusSender},
    sv1::{
        downstream::{downstream::Downstream, DownstreamMessages},
        sv1_server::{channel::Sv1ServerChannelState, data::Sv1ServerData},
        translation_utils::{create_notify, get_set_difficulty},
    },
    utils::ShutdownMessage,
};
use async_channel::{Receiver, Sender};
use network_helpers_sv2::sv1_connection::ConnectionSV1;
use roles_logic_sv2::{
    mining_sv2::{SubmitSharesExtended, Target},
    parsers::Mining,
    utils::{hash_rate_to_target, Mutex},
    vardiff::classic::VardiffState,
    Vardiff,
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    time,
};
use tracing::{debug, error, info, warn};

pub struct Sv1Server {
    sv1_server_channel_state: Sv1ServerChannelState,
    sv1_server_data: Arc<Mutex<Sv1ServerData>>,
    shares_per_minute: f32,
    listener_addr: SocketAddr,
    config: TranslatorConfig,
    clean_job: AtomicBool,
    sequence_counter: AtomicU32,
    miner_counter: AtomicU32,
}

impl Sv1Server {
    pub fn drop(&self) {
        self.sv1_server_channel_state.drop();
    }
    pub fn new(
        listener_addr: SocketAddr,
        channel_manager_receiver: Receiver<Mining<'static>>,
        channel_manager_sender: Sender<Mining<'static>>,
        config: TranslatorConfig,
    ) -> Self {
        let shares_per_minute = config.downstream_difficulty_config.shares_per_minute as f32;
        let sv1_server_channel_state =
            Sv1ServerChannelState::new(channel_manager_receiver, channel_manager_sender);
        let sv1_server_data = Arc::new(Mutex::new(Sv1ServerData::new()));
        Self {
            sv1_server_channel_state,
            sv1_server_data,
            config,
            listener_addr,
            shares_per_minute,
            clean_job: AtomicBool::new(true),
            miner_counter: AtomicU32::new(0),
            sequence_counter: AtomicU32::new(0),
        }
    }

    pub async fn start(
        self: Arc<Self>,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: Sender<Status>,
    ) -> Result<(), TproxyError> {
        info!("Starting SV1 server on {}", self.listener_addr);
        let mut shutdown_rx_main = notify_shutdown.subscribe();
        let shutdown_complete_tx_main_clone = shutdown_complete_tx.clone();

        // get the first target for the first set difficulty message
        let first_target: Target = hash_rate_to_target(
            self.config
                .downstream_difficulty_config
                .min_individual_miner_hashrate as f64,
            self.config.downstream_difficulty_config.shares_per_minute as f64,
        )
        .unwrap()
        .into();

        // Spawn vardiff loop
        tokio::spawn(Self::spawn_vardiff_loop(
            Arc::clone(&self),
            notify_shutdown.subscribe(),
            shutdown_complete_tx_main_clone.clone(),
        ));

        let listener = TcpListener::bind(self.listener_addr).await.map_err(|e| {
            error!("Failed to bind to {}: {}", self.listener_addr, e);
            e
        })?;

        let sv1_status_sender = StatusSender::Sv1Server(status_sender.clone());

        loop {
            tokio::select! {
                message = shutdown_rx_main.recv() => {
                    match message {
                        Ok(ShutdownMessage::ShutdownAll) => {
                            info!("SV1 Server: Vardiff loop received shutdown signal. Exiting.");
                            break;
                        }
                        Ok(ShutdownMessage::DownstreamShutdown(downstream_id)) => {
                            let current_downstream = self.sv1_server_data.super_safe_lock(|d| d.downstreams.remove(&downstream_id));
                            if current_downstream.is_some() {
                                info!("Downstream: {downstream_id} removed from sv1 server downstreams");
                            }
                        }
                        _ => {}
                    }
                }
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            info!("New SV1 downstream connection from {}", addr);

                            let connection = ConnectionSV1::new(stream).await;
                            let downstream_id = self.sv1_server_data.super_safe_lock(|v| v.downstream_id_factory.next());
                            let downstream = Downstream::new(
                                downstream_id,
                                connection.sender().clone(),
                                connection.receiver().clone(),
                                self.sv1_server_channel_state.downstream_to_sv1_server_sender.clone(),
                                self.sv1_server_channel_state.sv1_server_to_downstream_sender.clone(),
                                first_target.clone(),
                                self.config
                                    .downstream_difficulty_config
                                    .min_individual_miner_hashrate as f32,
                            );
                            // vardiff initialization
                            let vardiff = Arc::new(RwLock::new(VardiffState::new().expect("Failed to create vardiffstate")));
                            _ = self.sv1_server_data
                                .safe_lock(|d| {
                                    d.downstreams.insert(downstream_id, downstream.clone());
                                    // Insert vardiff state for this downstream
                                    d.vardiff.insert(downstream_id, vardiff);
                                });
                            info!("Downstream {} registered successfully", downstream_id);

                            self
                                .open_extended_mining_channel(downstream)
                                .await?;
                        }
                        Err(e) => {
                            warn!("Failed to accept new connection: {:?}", e);
                        }
                    }
                }
                res = Self::handle_downstream_message(
                    Arc::clone(&self)
                ) => {
                    if let Err(e) = res {
                        handle_error(&sv1_status_sender, e).await;
                        break;
                    }
                }
                res = Self::handle_upstream_message(
                    Arc::clone(&self),
                    first_target.clone(),
                    notify_shutdown.clone(),
                    shutdown_complete_tx_main_clone.clone(),
                    status_sender.clone()
                ) => {
                    if let Err(e) = res {
                        handle_error(&sv1_status_sender, e).await;
                        break;
                    }
                }
            }
        }
        self.sv1_server_channel_state.drop();
        drop(shutdown_complete_tx);
        warn!("SV1 Server main listener loop exited.");
        Ok(())
    }

    pub async fn handle_downstream_message(self: Arc<Self>) -> Result<(), TproxyError> {
        let downstream_message = self
            .sv1_server_channel_state
            .downstream_to_sv1_server_receiver
            .recv()
            .await
            .map_err(TproxyError::ChannelErrorReceiver)?;

        let DownstreamMessages::SubmitShares(message) = downstream_message;

        // Increment vardiff counter for this downstream
        self.sv1_server_data.safe_lock(|v| {
            if let Some(vardiff_state) = v.vardiff.get(&message.downstream_id) {
                vardiff_state
                    .write()
                    .unwrap()
                    .increment_shares_since_last_update();
            }
        })?;

        let last_job_version = message.last_job_version.ok_or_else(|| {
            TproxyError::RolesSv2LogicError(roles_logic_sv2::errors::Error::NoValidJob)
        })?;

        let version = match (message.share.version_bits, message.version_rolling_mask) {
            (Some(version_bits), Some(rolling_mask)) => {
                (last_job_version & !rolling_mask.0) | (version_bits.0 & rolling_mask.0)
            }
            (None, None) => last_job_version,
            _ => return Err(TproxyError::SV1Error),
        };

        let extranonce: Vec<u8> = message.share.extra_nonce2.into();

        let submit_share_extended = SubmitSharesExtended {
            channel_id: message.channel_id,
            sequence_number: self.sequence_counter.load(Ordering::SeqCst),
            job_id: message.share.job_id.parse::<u32>()?,
            nonce: message.share.nonce.0,
            ntime: message.share.time.0,
            version,
            extranonce: extranonce
                .try_into()
                .map_err(|_| TproxyError::General("Invalid extranonce length".into()))?,
        };

        self.sv1_server_channel_state
            .channel_manager_sender
            .send(Mining::SubmitSharesExtended(submit_share_extended))
            .await
            .map_err(|_| TproxyError::ChannelErrorSender)?;

        self.sequence_counter.fetch_add(1, Ordering::SeqCst);

        Ok(())
    }

    pub async fn handle_upstream_message(
        self: Arc<Self>,
        first_target: Target,
        notify_shutdown: broadcast::Sender<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
        status_sender: Sender<Status>,
    ) -> Result<(), TproxyError> {
        let message = self
            .sv1_server_channel_state
            .channel_manager_receiver
            .recv()
            .await
            .map_err(TproxyError::ChannelErrorReceiver)?;

        match message {
            Mining::OpenExtendedMiningChannelSuccess(m) => {
                let downstream_id = m.request_id;
                let downstreams = self
                    .sv1_server_data
                    .super_safe_lock(|v| v.downstreams.clone());
                if let Some(downstream) = Self::get_downstream(downstream_id, downstreams) {
                    downstream.downstream_data.safe_lock(|d| {
                        d.extranonce1 = m.extranonce_prefix.to_vec();
                        d.extranonce2_len = m.extranonce_size.into();
                        d.channel_id = Some(m.channel_id);
                    })?;

                    let status_sender = StatusSender::Downstream {
                        downstream_id,
                        tx: status_sender.clone(),
                    };

                    Downstream::run_downstream_tasks(
                        Arc::new(downstream),
                        notify_shutdown,
                        shutdown_complete_tx,
                        status_sender,
                    );

                    // this is done to make sure that the job is sent after the initial handshake (subscribe, authorize, etc.) is done
                    time::sleep(Duration::from_secs(1)).await;
                    let set_difficulty = get_set_difficulty(first_target).map_err(|_| {
                        TproxyError::General("Failed to generate set_difficulty".into())
                    })?;
                    // send the set_difficulty message to the downstream
                    self.sv1_server_channel_state
                        .sv1_server_to_downstream_sender
                        .send((m.channel_id, None, set_difficulty.into()))
                        .map_err(|_| TproxyError::ChannelErrorSender)?;
                } else {
                    error!("Downstream not found for downstream_id: {}", downstream_id);
                }
            }

            Mining::NewExtendedMiningJob(m) => {
                if let Some(prevhash) = self.sv1_server_data.super_safe_lock(|v| v.prevhash.clone())
                {
                    let notify = create_notify(
                        prevhash,
                        m.clone().into_static(),
                        self.clean_job.load(Ordering::SeqCst),
                    );
                    self.clean_job.store(false, Ordering::SeqCst);
                    let _ = self
                        .sv1_server_channel_state
                        .sv1_server_to_downstream_sender
                        .send((m.channel_id, None, notify.into()));
                }
            }

            Mining::SetNewPrevHash(m) => {
                self.clean_job.store(true, Ordering::SeqCst);
                self.sv1_server_data
                    .super_safe_lock(|v| v.prevhash = Some(m.clone().into_static()));
            }

            Mining::CloseChannel(_) => {
                todo!("Handle CloseChannel message from upstream");
            }

            Mining::OpenMiningChannelError(_) => {
                todo!("Handle OpenMiningChannelError message from upstream");
            }

            Mining::UpdateChannelError(_) => {
                todo!("Handle UpdateChannelError message from upstream");
            }

            _ => unreachable!("Unexpected message type received from upstream"),
        }

        Ok(())
    }

    pub async fn open_extended_mining_channel(
        &self,
        downstream: Downstream,
    ) -> Result<(), TproxyError> {
        let config = &self.config.downstream_difficulty_config;

        let hashrate = config.min_individual_miner_hashrate as f64;
        let shares_per_min = config.shares_per_minute as f64;
        let min_extranonce_size = self.config.min_extranonce2_size;

        let initial_target: Target = hash_rate_to_target(hashrate, shares_per_min)
            .unwrap()
            .into();

        let miner_id = self.miner_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let user_identity = format!("{}.miner{}", self.config.user_identity, miner_id);

        downstream
            .downstream_data
            .safe_lock(|d| d.user_identity = user_identity.clone())?;

        let open_channel_msg = roles_logic_sv2::mining_sv2::OpenExtendedMiningChannel {
            request_id: downstream
                .downstream_data
                .super_safe_lock(|d| d.downstream_id),
            user_identity: user_identity.try_into()?,
            nominal_hash_rate: hashrate as f32,
            max_target: initial_target.into(),
            min_extranonce_size,
        };

        self.sv1_server_channel_state
            .channel_manager_sender
            .send(Mining::OpenExtendedMiningChannel(open_channel_msg))
            .await
            .map_err(|_| TproxyError::ChannelErrorSender)?;

        Ok(())
    }

    pub fn get_downstream(
        downstream_id: u32,
        downstream: HashMap<u32, Downstream>,
    ) -> Option<Downstream> {
        downstream.get(&downstream_id).cloned()
    }

    pub fn get_downstream_id(downstream: Downstream) -> u32 {
        downstream
            .downstream_data
            .super_safe_lock(|s| s.downstream_id)
    }

    /// This method implements the SV1 server's variable difficulty logic for all downstreams.
    /// Every 60 seconds, this method updates the difficulty state for each downstream.
    async fn spawn_vardiff_loop(
        self: Arc<Self>,
        mut notify_shutdown: broadcast::Receiver<ShutdownMessage>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) {
        info!("Spawning vardiff adjustment loop for SV1 server");

        'vardiff_loop: loop {
            tokio::select! {
                message = notify_shutdown.recv() => {
                    match message {
                        Ok(ShutdownMessage::ShutdownAll) => {
                            info!("SV1 Server: Vardiff loop received shutdown signal. Exiting.");
                            break 'vardiff_loop;
                        }
                        Ok(ShutdownMessage::DownstreamShutdown(downstream_id)) => {
                            let current_downstream = self.sv1_server_data.super_safe_lock(|d| d.downstreams.remove(&downstream_id));
                            if current_downstream.is_some() {
                                info!("Downstream: {downstream_id} removed from sv1 server downstreams");
                            }
                        }
                        _ => {}
                    }
                }
                _ = time::sleep(Duration::from_secs(60)) => {
                    let vardiff_map = self.sv1_server_data.super_safe_lock(|v| v.vardiff.clone());
                    let mut updates = Vec::new();
                    for (downstream_id, vardiff_state) in vardiff_map.iter() {
                        debug!("Updating vardiff for downstream_id: {}", downstream_id);
                        let mut vardiff = vardiff_state.write().unwrap();
                        // Get hashrate and target from downstreams
                        let Some((channel_id, hashrate, target)) = self.sv1_server_data.super_safe_lock(|data| {
                            data.downstreams.get(downstream_id).and_then(|ds| {
                                ds.downstream_data.super_safe_lock(|d| Some((d.channel_id, d.hashrate, d.target.clone())))
                            })
                        }) else {
                            continue;
                        };

                        if channel_id.is_none() {
                            error!("Channel id is none for downstream_id: {}", downstream_id);
                            continue;
                        }
                        let channel_id = channel_id.unwrap();
                        let new_hashrate_opt = vardiff.try_vardiff(hashrate, &target, self.shares_per_minute);

                        if let Ok(Some(new_hashrate)) = new_hashrate_opt {
                            // Calculate new target based on new hashrate
                            let new_target: Target =
                                hash_rate_to_target(new_hashrate as f64, self.shares_per_minute as f64)
                                    .unwrap()
                                    .into();

                            // Update the downstream's pending target and hashrate
                            _ = self.sv1_server_data.safe_lock(|dmap| {
                                if let Some(d) = dmap.downstreams.get(downstream_id) {
                                    _ = d.downstream_data.safe_lock(|d| {
                                        d.set_pending_target_and_hashrate(new_target.clone(), new_hashrate);
                                    });
                                }
                            });

                            updates.push((channel_id, Some(*downstream_id), new_target.clone()));

                            debug!(
                                "Calculated new target for downstream_id={} to {:?}",
                                downstream_id, new_target
                            );
                        }
                    }

                    for (channel_id, downstream_id, target) in updates {
                        if let Ok(set_difficulty_msg) = get_set_difficulty(target) {
                            if let Err(e) =
                                self.sv1_server_channel_state.sv1_server_to_downstream_sender.send((channel_id, downstream_id, set_difficulty_msg))
                            {
                                error!(
                                    "Failed to send SetDifficulty message to downstream {}: {:?}",
                                    downstream_id.unwrap_or(0),
                                    e
                                );
                                break 'vardiff_loop;
                            }
                        }
                    }
                }
            }
        }
        drop(shutdown_complete_tx);
        warn!("SV1 Server: Vardiff loop exited.");
    }
}
