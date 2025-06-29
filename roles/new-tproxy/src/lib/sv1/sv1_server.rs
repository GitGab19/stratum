use crate::{
    config::TranslatorConfig,
    error::ProxyResult,
    sv1::{
        downstream::{Downstream, DownstreamData},
        translation_utils::{create_notify, get_set_difficulty},
        DownstreamMessages,
    },
};
use async_channel::{unbounded, Receiver, Sender};
use network_helpers_sv2::sv1_connection::ConnectionSV1;
use roles_logic_sv2::{
    bitcoin::secp256k1::Message,
    mining_sv2::{SetNewPrevHash, SubmitSharesExtended, Target},
    parsers::Mining,
    utils::{hash_rate_to_target, Id as IdFactory, Mutex},
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
use v1::{
    client_to_server,
    error::Error,
    json_rpc, server_to_client,
    utils::{Extranonce, HexU32Be},
    IsServer,
};

struct Sv1ServerChannelState {
    sv1_server_to_downstream_sender: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>,
    sv1_server_to_downstream_receiver: broadcast::Receiver<(u32, Option<u32>, json_rpc::Message)>, /* channel_id, optional downstream_id, message */
    downstream_to_sv1_server_sender: Sender<DownstreamMessages>,
    downstream_to_sv1_server_receiver: Receiver<DownstreamMessages>,
    channel_manager_receiver: Receiver<Mining<'static>>,
    channel_manager_sender: Sender<Mining<'static>>,
}

impl Sv1ServerChannelState {
    fn new(
        channel_manager_receiver: Receiver<Mining<'static>>,
        channel_manager_sender: Sender<Mining<'static>>,
    ) -> Self {
        let (sv1_server_to_downstream_sender, sv1_server_to_downstream_receiver) =
            broadcast::channel(10);
        // mpsc - sender is only clonable and receiver are not..
        let (downstream_to_sv1_server_sender, downstream_to_sv1_server_receiver) = unbounded();

        Self {
            sv1_server_to_downstream_sender,
            sv1_server_to_downstream_receiver,
            downstream_to_sv1_server_receiver,
            downstream_to_sv1_server_sender,
            channel_manager_receiver,
            channel_manager_sender,
        }
    }
}

struct Sv1ServerData {
    downstreams: HashMap<u32, Downstream>,
    vardiff: HashMap<u32, Arc<RwLock<VardiffState>>>,
    prevhash: Option<SetNewPrevHash<'static>>,
    downstream_id_factory: IdFactory,
}

impl Sv1ServerData {
    fn new() -> Self {
        Self {
            downstreams: HashMap::new(),
            vardiff: HashMap::new(),
            prevhash: None,
            downstream_id_factory: IdFactory::new(),
        }
    }
}

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
        notify_shutdown: broadcast::Sender<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) -> ProxyResult<'static, ()> {
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

        tokio::spawn(Self::handle_downstream_message(
            Arc::clone(&self),
            notify_shutdown.subscribe(),
            shutdown_complete_tx_main_clone.clone(),
        ));
        tokio::spawn(Self::handle_upstream_message(
            Arc::clone(&self),
            first_target.clone(),
            notify_shutdown.clone(),
            shutdown_complete_tx_main_clone.clone(),
        ));

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

        loop {
            tokio::select! {
                _ = shutdown_rx_main.recv() => {
                    info!("SV1 Server main listener received shutdown signal. Stopping new connections.");
                    break;
                }
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            info!("New SV1 downstream connection from {}", addr);

                            let connection = ConnectionSV1::new(stream).await;
                            let downstream_id = self.sv1_server_data.super_safe_lock(|v| v.downstream_id_factory.next());
                            let mut downstream = Downstream::new(
                                downstream_id,
                                connection.sender().clone(),
                                connection.receiver().clone(),
                                self.sv1_server_channel_state.downstream_to_sv1_server_sender.clone(),
                                self.sv1_server_channel_state.sv1_server_to_downstream_sender.clone(),
                                first_target.clone(),
                                self.shares_per_minute,
                                self.config
                                    .downstream_difficulty_config
                                    .min_individual_miner_hashrate as f32,
                            );
                            // vardiff initialization
                            let vardiff = Arc::new(RwLock::new(VardiffState::new().expect("Failed to create vardiffstate")));
                            self.sv1_server_data
                                .safe_lock(|d| {
                                    d.downstreams.insert(downstream_id, downstream.clone());
                                    // Insert vardiff state for this downstream
                                    d.vardiff.insert(downstream_id, vardiff);
                                });
                            info!("Downstream {} registered successfully", downstream_id);

                            let channel_id = self
                                .open_extended_mining_channel(connection, downstream)
                                .await?;
                        }
                        Err(e) => {
                            warn!("Failed to accept new connection: {:?}", e);
                        }
                    }
                }
            }
        }
        drop(shutdown_complete_tx);
        warn!("SV1 Server main listener loop exited.");
        Ok(())
    }

    pub async fn handle_downstream_message(
        self: Arc<Self>,
        mut notify_shutdown: broadcast::Receiver<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) -> ProxyResult<'static, ()> {
        info!("SV1 Server: Downstream message handler started.");
        loop {
            tokio::select! {
                _ = notify_shutdown.recv() => {
                    info!("SV1 Server: Downstream message handler received shutdown signal. Exiting");
                    break;
                }
                downstream_message_result = self.sv1_server_channel_state.downstream_to_sv1_server_receiver.recv() => {
                    match downstream_message_result {
                        Ok(downstream_message) => {
                            match downstream_message {
                                DownstreamMessages::SubmitShares(message) => {
                                    // Increment vardiff counter for this downstream
                                    self.sv1_server_data.safe_lock(|v| {
                                        if let Some(vardiff_state) = v.vardiff.get(&message.downstream_id) {
                                            vardiff_state.write().unwrap().increment_shares_since_last_update();
                                        }
                                    });

                                    // For version masking see https://github.com/slushpool/stratumprotocol/blob/master/stratum-extensions.mediawiki#changes-in-request-miningsubmit
                                    let last_job_version =
                                        message
                                            .last_job_version
                                            .ok_or(crate::error::Error::RolesSv2Logic(
                                                roles_logic_sv2::errors::Error::NoValidJob,
                                            ))?;
                                    let version = match (message.share.version_bits, message.version_rolling_mask) {
                                        (Some(version_bits), Some(rolling_mask)) => {
                                            (last_job_version & !rolling_mask.0) | (version_bits.0 & rolling_mask.0)
                                        }
                                        (None, None) => last_job_version,
                                        _ => {
                                            return Err(crate::error::Error::V1Protocol(
                                                v1::error::Error::InvalidSubmission,
                                            ))
                                        }
                                    };
                                    let extranonce: Vec<u8> = message.share.extra_nonce2.into();

                                    let submit_share_extended = SubmitSharesExtended {
                                        channel_id: message.channel_id,
                                        sequence_number: self.sequence_counter.load(Ordering::SeqCst),
                                        job_id: message.share.job_id.parse::<u32>()?,
                                        nonce: message.share.nonce.0,
                                        ntime: message.share.time.0,
                                        version: version,
                                        extranonce: extranonce.try_into()?,
                                    };
                                    // send message to channel manager for validation with channel target
                                    self.sv1_server_channel_state.channel_manager_sender
                                        .send(Mining::SubmitSharesExtended(submit_share_extended))
                                        .await;
                                    self.sequence_counter.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                        }
                        Err(e) => {
                            error!("SV1 Server Downstream message received closed: {:?}", e);
                            break;
                        }
                    }
                }
            }
        }
        self.sv1_server_channel_state
            .downstream_to_sv1_server_receiver
            .close();
        self.sv1_server_channel_state
            .channel_manager_sender
            .close();
        drop(shutdown_complete_tx);
        warn!("SV1 Server: Downstream message handler exited.");
        Ok(())
    }

    pub async fn handle_upstream_message(
        self: Arc<Self>,
        first_target: Target,
        notify_shutdown: broadcast::Sender<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) -> ProxyResult<'static, ()> {
        info!("SV1 Server: Upstream message handler started.");
        let mut notify_subscribe = notify_shutdown.subscribe();
        loop {
            tokio::select! {
                _ = notify_subscribe.recv() => {
                    info!("SV1 Server: Upstream message handler received shutdown signal. Exiting.");
                    break;
                }
                message_result = self.sv1_server_channel_state.channel_manager_receiver.recv() => {
                    match message_result {
                        Ok(message) => {
                            match message {
                                Mining::OpenExtendedMiningChannelSuccess(m) => {
                                    let downstream_id = m.request_id;
                                    let downstreams = self.sv1_server_data.super_safe_lock(|v| v.downstreams.clone());
                                    let downstream = Self::get_downstream(downstream_id, downstreams);
                                    if let Some(downstream) = downstream {
                                        downstream.downstream_data.safe_lock(|d| {
                                            d.extranonce1 = m.extranonce_prefix.to_vec();
                                            d.extranonce2_len = m.extranonce_size.into();
                                            d.channel_id = Some(m.channel_id);
                                        });
                                        downstream.clone().spawn_downstream_receiver(notify_shutdown.clone(), shutdown_complete_tx.clone());
                                        downstream.spawn_downstream_sender(notify_shutdown.clone(), shutdown_complete_tx.clone());
                                    } else {
                                        error!("Downstream not found for downstream id: {}", downstream_id);
                                    }
                                }
                                Mining::NewExtendedMiningJob(m) => {
                                    // if it's the first job, send the set difficulty
                                    if m.job_id == 1 {
                                        let set_difficulty = get_set_difficulty(first_target.clone()).unwrap();
                                        self.sv1_server_channel_state.sv1_server_to_downstream_sender.send((m.channel_id, None, set_difficulty.into()));
                                    }
                                    let prevhash = self.sv1_server_data.super_safe_lock(|x| x.prevhash.clone());
                                    if let Some(prevhash) = prevhash {
                                        let notify = create_notify(prevhash, m.clone().into_static(), self.clean_job.load(Ordering::SeqCst));
                                        self.clean_job.store(false, Ordering::SeqCst);
                                        let _ = self.sv1_server_channel_state.sv1_server_to_downstream_sender.send((m.channel_id, None, notify.into()));
                                    }
                                }
                                Mining::SetNewPrevHash(m) => {
                                    self.clean_job.store(true, Ordering::SeqCst);
                                    self.sv1_server_data.super_safe_lock(|d| d.prevhash = Some(m.clone().into_static()));
                                }
                                Mining::CloseChannel(m) => {
                                    todo!()
                                }
                                Mining::OpenMiningChannelError(m) => {
                                    todo!()
                                }
                                Mining::UpdateChannelError(m) => {
                                    todo!()
                                }
                                _ => unreachable!()
                            }
                        }
                        Err(e) => {
                            error!("SV1 Server ChannelManager receiver closed: {:?}", e);
                            break;
                        }
                    }
                }

            }
        }
        self.sv1_server_channel_state
            .channel_manager_receiver
            .close();
        drop(shutdown_complete_tx);
        warn!("SV1 Server: Upstream message handler exited.");
        Ok(())
    }

    pub async fn open_extended_mining_channel(
        &self,
        connection: ConnectionSV1,
        downstream: Downstream,
    ) -> ProxyResult<'static, Option<u32>> {
        let hashrate = self
            .config
            .downstream_difficulty_config
            .min_individual_miner_hashrate as f64;
        let share_per_min: f64 = self.config.downstream_difficulty_config.shares_per_minute as f64;
        let min_extranonce_size = self.config.min_extranonce2_size;
        let initial_target: Target = hash_rate_to_target(hashrate, share_per_min).unwrap().into();

        // Get the next miner counter and create unique user identity
        self.miner_counter.fetch_add(1, Ordering::SeqCst);
        let user_identity = format!(
            "{}.miner{}",
            self.config.user_identity,
            self.miner_counter.load(Ordering::SeqCst)
        );

        downstream.downstream_data.safe_lock(|d| {
            d.user_identity = user_identity.clone();
        });

        // Create OpenExtendedMiningChannel message with the unique user identity
        let open_channel_msg = roles_logic_sv2::mining_sv2::OpenExtendedMiningChannel {
            request_id: downstream
                .downstream_data
                .super_safe_lock(|d| d.downstream_id),
            user_identity: user_identity.try_into()?,
            nominal_hash_rate: hashrate as f32,
            max_target: initial_target.into(),
            min_extranonce_size: min_extranonce_size,
        };

        let open_upstream_channel = self
            .sv1_server_channel_state
            .channel_manager_sender
            .send(Mining::OpenExtendedMiningChannel(open_channel_msg))
            .await;

        Ok(None)
    }

    pub fn get_downstream(
        downstream_id: u32,
        downstream: HashMap<u32, Downstream>,
    ) -> Option<Downstream> {
        downstream.get(&downstream_id).cloned()
    }

    pub fn get_downstream_id(downstream: Downstream) -> u32 {
        let id = downstream.downstream_data.safe_lock(|s| s.downstream_id);
        return id.unwrap();
    }

    /// This method implements the SV1 server's variable difficulty logic for all downstreams.
    /// Every 60 seconds, this method updates the difficulty state for each downstream.
    async fn spawn_vardiff_loop(
        self: Arc<Self>,
        mut notify_shutdown: broadcast::Receiver<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) {
        info!("Spawning vardiff adjustment loop for SV1 server");

        'vardiff_loop: loop {
            tokio::select! {
                _ = notify_shutdown.recv() => {
                    info!("SV1 Server: Vardiff loop received shutdown signal. Exiting.");
                    break 'vardiff_loop;
                }
                _ = time::sleep(Duration::from_secs(60)) => {
                    info!("Starting vardiff updates for SV1 server");
                    let vardiff_map = self.sv1_server_data.super_safe_lock(|v| v.vardiff.clone());
                    let mut updates = Vec::new();
                    for (downstream_id, vardiff_state) in vardiff_map.iter() {
                        info!("Updating vardiff for downstream_id: {}", downstream_id);
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
                            self.sv1_server_data.safe_lock(|dmap| {
                                if let Some(d) = dmap.downstreams.get(downstream_id) {
                                    d.downstream_data.safe_lock(|d| {
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
