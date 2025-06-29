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
    sync::{Arc, RwLock},
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

pub struct Sv1Server {
    downstream_id_factory: IdFactory,
    sv1_server_to_downstream_sender: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>,
    sv1_server_to_downstream_receiver: broadcast::Receiver<(u32, Option<u32>, json_rpc::Message)>, /* channel_id, optional downstream_id, message */
    downstream_to_sv1_server_sender: Sender<DownstreamMessages>,
    downstream_to_sv1_server_receiver: Receiver<DownstreamMessages>,
    downstreams: Arc<Mutex<HashMap<u32, Downstream>>>,
    vardiff: Arc<Mutex<HashMap<u32, Arc<RwLock<VardiffState>>>>>,
    prevhash: Arc<Mutex<Option<SetNewPrevHash<'static>>>>,
    listener_addr: SocketAddr,
    channel_manager_receiver: Receiver<Mining<'static>>,
    channel_manager_sender: Sender<Mining<'static>>,
    clean_job: Arc<Mutex<bool>>,
    config: TranslatorConfig,
    sequence_counter: Arc<Mutex<u32>>,
    miner_counter: Arc<Mutex<u32>>,
    shares_per_minute: f32,
}

impl Sv1Server {
    pub fn new(
        // sv1_server_to_downstream_sender: Sender<(u32, json_rpc::Message)>,
        // downstream_to_sv1_server_receiver: Receiver<(u32, json_rpc::Message)>,
        listener_addr: SocketAddr,
        channel_manager_receiver: Receiver<Mining<'static>>,
        channel_manager_sender: Sender<Mining<'static>>,
        config: TranslatorConfig,
    ) -> Self {
        let (sv1_server_to_downstream_sender, sv1_server_to_downstream_receiver) =
            broadcast::channel(10);
        // mpsc - sender is only clonable and receiver are not..
        let (downstream_to_sv1_server_sender, downstream_to_sv1_server_receiver) = unbounded();
        let shares_per_minute = config.downstream_difficulty_config.shares_per_minute as f32;
        Self {
            sv1_server_to_downstream_sender,
            sv1_server_to_downstream_receiver,
            downstream_to_sv1_server_sender,
            downstream_to_sv1_server_receiver,
            downstream_id_factory: IdFactory::new(),
            downstreams: Arc::new(Mutex::new(HashMap::new())),
            vardiff: Arc::new(Mutex::new(HashMap::new())),
            prevhash: Arc::new(Mutex::new(None)),
            listener_addr,
            channel_manager_receiver,
            channel_manager_sender,
            clean_job: Arc::new(Mutex::new(true)),
            config,
            sequence_counter: Arc::new(Mutex::new(0)),
            miner_counter: Arc::new(Mutex::new(0)),
            shares_per_minute,
        }
    }

    pub async fn start(
        &mut self,
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

        let vardiff = self.vardiff.clone();
        tokio::spawn(Self::handle_downstream_message(
            self.downstream_to_sv1_server_receiver.clone(),
            self.channel_manager_sender.clone(),
            self.sequence_counter.clone(),
            self.downstreams.clone(),
            vardiff.clone(),
            notify_shutdown.subscribe(),
            shutdown_complete_tx_main_clone.clone(),
        ));
        tokio::spawn(Self::handle_upstream_message(
            self.channel_manager_receiver.clone(),
            self.sv1_server_to_downstream_sender.clone(),
            self.downstreams.clone(),
            self.prevhash.clone(),
            self.clean_job.clone(),
            first_target.clone(),
            notify_shutdown.clone(),
            shutdown_complete_tx_main_clone.clone(),
        ));

        // Spawn vardiff loop
        tokio::spawn(Self::spawn_vardiff_loop(
            self.downstreams.clone(),
            vardiff.clone(),
            self.sv1_server_to_downstream_sender.clone(),
            self.shares_per_minute,
            notify_shutdown.subscribe(),
            shutdown_complete_tx_main_clone.clone(),
        ));

        let listener = TcpListener::bind(self.listener_addr).await.map_err(|e| {
            error!("Failed to bind to {}: {}", self.listener_addr, e);
            e
        })?;

        let vardiff = self.vardiff.clone();
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
                            let downstream_id = self.downstream_id_factory.next();
                            let mut downstream = Downstream::new(
                                downstream_id,
                                connection.sender().clone(),
                                connection.receiver().clone(),
                                self.downstream_to_sv1_server_sender.clone(),
                                self.sv1_server_to_downstream_sender.clone(),
                                first_target.clone(),
                                self.shares_per_minute,
                                self.config
                                    .downstream_difficulty_config
                                    .min_individual_miner_hashrate as f32,
                            );
                            self.downstreams
                                .safe_lock(|d| d.insert(downstream_id, downstream.clone()));
                            // Insert vardiff state for this downstream
                            vardiff.safe_lock(|v| {
                                v.insert(
                                    downstream_id,
                                    Arc::new(RwLock::new(
                                        VardiffState::new().expect("Failed to create VardiffState"),
                                    )),
                                );
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
        mut downstream_to_sv1_server_receiver: Receiver<DownstreamMessages>,
        sv1_server_to_channel_manager_sender: Sender<Mining<'static>>,
        sequence_counter: Arc<Mutex<u32>>,
        downstreams: Arc<Mutex<HashMap<u32, Downstream>>>,
        vardiff: Arc<Mutex<HashMap<u32, Arc<RwLock<VardiffState>>>>>,
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
                downstream_message_result = downstream_to_sv1_server_receiver.recv() => {
                    match downstream_message_result {
                        Ok(downstream_message) => {
                            match downstream_message {
                                DownstreamMessages::SubmitShares(message) => {
                                    // Increment vardiff counter for this downstream
                                    vardiff.safe_lock(|v| {
                                        if let Some(vardiff_state) = v.get(&message.downstream_id) {
                                            vardiff_state
                                                .write()
                                                .unwrap()
                                                .increment_shares_since_last_update();
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
                                        sequence_number: sequence_counter.super_safe_lock(|c| *c),
                                        job_id: message.share.job_id.parse::<u32>()?,
                                        nonce: message.share.nonce.0,
                                        ntime: message.share.time.0,
                                        version: version,
                                        extranonce: extranonce.try_into()?,
                                    };
                                    // send message to channel manager for validation with channel target
                                    sv1_server_to_channel_manager_sender
                                        .send(Mining::SubmitSharesExtended(submit_share_extended))
                                        .await;
                                    sequence_counter.super_safe_lock(|c| *c += 1);
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
        downstream_to_sv1_server_receiver.close();
        sv1_server_to_channel_manager_sender.close();
        drop(shutdown_complete_tx);
        warn!("SV1 Server: Downstream message handler exited.");
        Ok(())
    }

    pub async fn handle_upstream_message(
        mut channel_manager_receiver: Receiver<Mining<'static>>,
        downstream_sender: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>,
        downstreams: Arc<Mutex<HashMap<u32, Downstream>>>,
        prevhash_mut: Arc<Mutex<Option<SetNewPrevHash<'static>>>>,
        clean_job_mut: Arc<Mutex<bool>>,
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
                message_result = channel_manager_receiver.recv() => {
                    match message_result {
                        Ok(message) => {
                            match message {
                                Mining::OpenExtendedMiningChannelSuccess(m) => {
                                    let downstream_id = m.request_id;
                                    let downstream = Self::get_downstream(downstream_id, downstreams.clone());
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
                                        downstream_sender.send((m.channel_id, None, set_difficulty.into()));
                                    }
                                    let prevhash = prevhash_mut.super_safe_lock(|ph| ph.clone());
                                    let clean_job = clean_job_mut.super_safe_lock(|c| *c);
                                    if let Some(prevhash) = prevhash {
                                        let notify = create_notify(prevhash, m.clone().into_static(), clean_job);
                                        clean_job_mut.super_safe_lock(|c| *c = false);
                                        let _ = downstream_sender.send((m.channel_id, None, notify.into()));
                                    }
                                }
                                Mining::SetNewPrevHash(m) => {
                                    prevhash_mut.super_safe_lock(|ph| *ph = Some(m.clone().into_static()));
                                    clean_job_mut.super_safe_lock(|c| *c = true);
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
        channel_manager_receiver.close();
        drop(shutdown_complete_tx);
        warn!("SV1 Server: Upstream message handler exited.");
        Ok(())
    }

    pub async fn open_extended_mining_channel(
        &mut self,
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
        let miner_number = self.miner_counter.super_safe_lock(|c| {
            *c += 1;
            *c
        });
        let user_identity = format!("{}.miner{}", self.config.user_identity, miner_number);

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
            .channel_manager_sender
            .send(Mining::OpenExtendedMiningChannel(open_channel_msg))
            .await;

        Ok(None)
    }

    pub fn get_downstream(
        downstream_id: u32,
        downstream: Arc<Mutex<HashMap<u32, Downstream>>>,
    ) -> Option<Downstream> {
        downstream
            .safe_lock(|c| c.get(&downstream_id).cloned())
            .unwrap_or(None)
    }

    pub fn get_downstream_id(downstream: Downstream) -> u32 {
        let id = downstream.downstream_data.safe_lock(|s| s.downstream_id);
        return id.unwrap();
    }

    /// This method implements the SV1 server's variable difficulty logic for all downstreams.
    /// Every 60 seconds, this method updates the difficulty state for each downstream.
    async fn spawn_vardiff_loop(
        downstreams: Arc<Mutex<HashMap<u32, Downstream>>>,
        vardiff: Arc<Mutex<HashMap<u32, Arc<RwLock<VardiffState>>>>>,
        downstream_sender: broadcast::Sender<(u32, Option<u32>, json_rpc::Message)>,
        shares_per_minute: f32,
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
                    let vardiff_map = vardiff.safe_lock(|v| v.clone()).unwrap();
                    let mut updates = Vec::new();
                    for (downstream_id, vardiff_state) in vardiff_map.iter() {
                        info!("Updating vardiff for downstream_id: {}", downstream_id);
                        let mut vardiff = vardiff_state.write().unwrap();
                        // Get hashrate and target from downstreams
                        let (channel_id, hashrate, target) = match downstreams.safe_lock(|dmap| {
                            dmap.get(downstream_id).map(|d| {
                                d.downstream_data.super_safe_lock(|d| (d.channel_id, d.hashrate, d.target.clone()))
                            })
                        }) {
                            Ok(Some((channel_id, hashrate, target))) => (channel_id, hashrate, target),
                            _ => continue,
                        };
                        if channel_id.is_none() {
                            error!("Channel id is none for downstream_id: {}", downstream_id);
                            continue;
                        }
                        let channel_id = channel_id.unwrap();
                        let new_hashrate_opt = vardiff.try_vardiff(hashrate, &target, shares_per_minute);

                        if let Ok(Some(new_hashrate)) = new_hashrate_opt {
                            // Calculate new target based on new hashrate
                            let new_target: Target =
                                hash_rate_to_target(new_hashrate as f64, shares_per_minute as f64)
                                    .unwrap()
                                    .into();

                            // Update the downstream's pending target and hashrate
                            downstreams.safe_lock(|dmap| {
                                if let Some(d) = dmap.get(downstream_id) {
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
                                downstream_sender.send((channel_id, downstream_id, set_difficulty_msg))
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
