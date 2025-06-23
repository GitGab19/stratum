use crate::{
    downstream_sv1::{
        downstream,
        sv2_to_sv1_utils::{create_notify, get_set_difficulty},
        Downstream, DownstreamMessages,
    },
    error::ProxyResult,
    proxy::ChannelManager,
};
use async_channel::{unbounded, Receiver, Sender};
use network_helpers_sv2::sv1_connection::ConnectionSV1;
use roles_logic_sv2::{
    bitcoin::secp256k1::Message,
    mining_sv2::{SetNewPrevHash, SubmitSharesExtended},
    parsers::Mining,
    utils::{Id as IdFactory, Mutex},
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
};
use tracing::{error, info, warn};
use v1::{
    client_to_server,
    error::Error,
    json_rpc, server_to_client,
    utils::{Extranonce, HexU32Be},
    IsServer,
};

pub struct Sv1Server {
    downstream_id_factory: IdFactory,
    sv1_server_to_downstream_sender: broadcast::Sender<(u32, json_rpc::Message)>,
    sv1_server_to_downstream_receiver: broadcast::Receiver<(u32, json_rpc::Message)>,
    downstream_to_sv1_server_sender: Sender<DownstreamMessages>,
    downstream_to_sv1_server_receiver: Receiver<DownstreamMessages>,
    downstreams: Arc<Mutex<HashMap<u32, Arc<Mutex<Downstream>>>>>,
    prevhash: Arc<Mutex<Option<SetNewPrevHash<'static>>>>,
    listener_addr: SocketAddr,
    channel_manager_to_sv1_server_receiver: broadcast::Sender<Mining<'static>>,
    sv1_server_to_channel_manager_sender: Sender<(u32, Mining<'static>)>,
    channel_opener_sender: Sender<(u32, String)>,
}

impl Sv1Server {
    pub fn new(
        // sv1_server_to_downstream_sender: Sender<(u32, json_rpc::Message)>,
        // downstream_to_sv1_server_receiver: Receiver<(u32, json_rpc::Message)>,
        listener_addr: SocketAddr,
        channel_opener_sender: Sender<(u32, String)>,
        channel_manager_to_sv1_server_receiver: broadcast::Sender<Mining<'static>>,
        sv1_server_to_channel_manager_sender: Sender<(u32, Mining<'static>)>,
    ) -> Self {
        let (sv1_server_to_downstream_sender, sv1_server_to_downstream_receiver) =
            broadcast::channel(10);
        let (downstream_to_sv1_server_sender, downstream_to_sv1_server_receiver) = unbounded();
        Self {
            sv1_server_to_downstream_sender,
            sv1_server_to_downstream_receiver,
            downstream_to_sv1_server_sender,
            downstream_to_sv1_server_receiver,
            downstream_id_factory: IdFactory::new(),
            downstreams: Arc::new(Mutex::new(HashMap::new())),
            prevhash: Arc::new(Mutex::new(None)),
            listener_addr,
            channel_manager_to_sv1_server_receiver,
            sv1_server_to_channel_manager_sender,
            channel_opener_sender,
        }
    }

    pub async fn start(&mut self) -> ProxyResult<'static, ()> {
        info!("Starting SV1 server on {}", self.listener_addr);
        tokio::spawn(Self::handle_downstream_message(
            self.downstream_to_sv1_server_receiver.clone(),
            self.sv1_server_to_channel_manager_sender.clone(),
        ));
        tokio::spawn(Self::handle_upstream_message(
            self.channel_manager_to_sv1_server_receiver.subscribe(),
            self.sv1_server_to_downstream_sender.clone(),
            self.downstreams.clone(),
            self.prevhash.clone(),
        ));

        let listener = TcpListener::bind(self.listener_addr).await.map_err(|e| {
            error!("Failed to bind to {}: {}", self.listener_addr, e);
            e
        })?;

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("New SV1 downstream connection from {}", addr);

                    let connection = ConnectionSV1::new(stream).await;
                    let downstream_id = self.downstream_id_factory.next();
                    let prevhash = self.prevhash.super_safe_lock(|c| c.clone());
                    let mut downstream = Downstream::new(
                        downstream_id,
                        connection.sender().clone(),
                        connection.receiver().clone(),
                        self.downstream_to_sv1_server_sender.clone(),
                        self.sv1_server_to_downstream_sender.clone(),
                        prevhash,
                    );
                    let channel_id = self
                        .bootstrap_non_aggregation(connection, &mut downstream)
                        .await?;
                    if let Some(channel_id) = channel_id {
                        self.downstreams.safe_lock(|d| {
                            d.insert(channel_id, Arc::new(Mutex::new(downstream.clone())))
                        });

                        info!("Downstream {} registered successfully", downstream_id);
                        downstream.spawn_downstream_receiver();
                        downstream.spawn_downstream_sender();
                    }
                }
                Err(e) => {
                    warn!("Failed to accept new connection: {:?}", e);
                }
            }
        }
    }

    pub async fn handle_downstream_message(
        mut downstream_to_sv1_server_receiver: Receiver<DownstreamMessages>,
        sv1_server_to_channel_manager_sender: Sender<(u32, Mining<'static>)>,
    ) -> ProxyResult<'static, ()> {
        info!("Listening for downstream message inside sv1 server");
        while let Ok(downstream_message) = downstream_to_sv1_server_receiver.recv().await {
            // share validation will be done
            match downstream_message {
                DownstreamMessages::SubmitShares(message) => {
                    error!("Message from downstream to sv1 server:{:?}", message);
                    error!(
                        "Downstream id of the downstream which sent message to sv1 server: {:?}",
                        message.downstream_id
                    );

                    let submit_share_extended = SubmitSharesExtended {
                        channel_id: message.channel_id,
                        // will change soon
                        sequence_number: 0,
                        job_id: message.share.job_id.parse::<u32>()?,
                        nonce: message.share.nonce.0,
                        ntime: message.share.time.0,
                        // will change soon
                        version: 0,
                        extranonce: message.extranonce.try_into()?,
                    };
                    // send message to channel manager for validation
                    sv1_server_to_channel_manager_sender.send((
                        message.downstream_id,
                        Mining::SubmitSharesExtended(submit_share_extended),
                    ));
                }
            }
            // let share =
        }
        Ok(())
    }

    pub async fn handle_upstream_message(
        mut channel_manager_to_sv1_server_receiver: broadcast::Receiver<Mining<'static>>,
        sv1_server_to_downstream_sender: broadcast::Sender<(u32, json_rpc::Message)>,
        downstream: Arc<Mutex<HashMap<u32, Arc<Mutex<Downstream>>>>>,
        prevhash_mut: Arc<Mutex<Option<SetNewPrevHash<'static>>>>,
    ) {
        info!("Listening for upstream message inside sv1 server");
        while let Ok(message) = channel_manager_to_sv1_server_receiver.recv().await {
            match message {
                Mining::NewExtendedMiningJob(m) => {
                    if let Some(downstream) = Self::get_downstream(m.channel_id, downstream.clone())
                    {
                        let prevhash = Self::get_prevhash(downstream.clone());
                        let clean_job = Self::get_clean_job(downstream.clone());
                        let downstream_id = Self::get_downstream_id(downstream.clone());
                        if let Some(prevhash) = prevhash {
                            Self::set_clean_job(downstream, false);
                            let notify =
                                create_notify(prevhash, m.clone().into_static(), clean_job);
                            sv1_server_to_downstream_sender.send((downstream_id, notify.into()));
                        } else {
                            let prevhash = prevhash_mut.super_safe_lock(|ph| ph.clone());

                            Self::set_prevhash(downstream.clone(), prevhash.clone().unwrap());
                            Self::set_clean_job(downstream.clone(), true);
                            Self::set_clean_job(downstream, false);
                            let notify = create_notify(
                                prevhash.unwrap(),
                                m.clone().into_static(),
                                clean_job,
                            );
                            sv1_server_to_downstream_sender.send((downstream_id, notify.into()));
                        }
                    }
                }
                Mining::SetNewPrevHash(m) => {
                    prevhash_mut.super_safe_lock(|ph| *ph = Some(m.clone().into_static()));
                    if let Some(mut downstream) =
                        Self::get_downstream(m.channel_id, downstream.clone())
                    {
                        Self::set_prevhash(downstream.clone(), m.clone().into_static());
                        Self::set_clean_job(downstream, true);
                    }
                }
                Mining::CloseChannel(m) => {
                    info!("I got close channel: {:?}", m);
                }
                Mining::OpenMiningChannelError(m) => {
                    info!("I got open mining channel: {:?}", m);
                }
                Mining::UpdateChannelError(m) => {
                    info!("I got update channel error: {:?}", m);
                }
                Mining::SubmitSharesError(m) => {
                    info!("I got submit share error: {:?}", m);
                }
                Mining::SetCustomMiningJobError(m) => {
                    info!("I got set custom mining job: {:?}", m);
                }
                Mining::SubmitSharesSuccess(m) => {
                    info!("Received submit share success: {:?}", m);
                    if let Some(downstream) = Self::get_downstream(m.channel_id, downstream.clone())
                    {
                        let downstream_id = Self::get_downstream_id(downstream.clone());
                        // Send response from upstream to miner
                        // let submit_share = server_to_client::GeneralResponse::into_submit(self);
                        // sv1_server_to_downstream_sender.send((downstream_id,
                        // submit_share.into()));
                    }
                }
                Mining::SetTarget(m) => {
                    error!("Message: {:?}", m);
                    if let Some(downstream_mut) =
                        Self::get_downstream(m.channel_id, downstream.clone())
                    {
                        let set_difficult_message = get_set_difficulty(m.maximum_target.into());
                        if let Ok(set_difficult_message) = set_difficult_message {
                            error!("Set difficulty message: {:#?}", set_difficult_message);
                            sv1_server_to_downstream_sender.send((
                                Self::get_downstream_id(downstream_mut),
                                set_difficult_message.into(),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn get_downstream(
        channel_id: u32,
        downstream: Arc<Mutex<HashMap<u32, Arc<Mutex<Downstream>>>>>,
    ) -> Option<Arc<Mutex<Downstream>>> {
        downstream.super_safe_lock(|c| c.get(&channel_id).cloned())
    }

    pub fn get_downstream_id(downstream: Arc<Mutex<Downstream>>) -> u32 {
        let id = downstream.super_safe_lock(|s| s.downstream_id);
        return id;
    }

    pub fn get_prevhash(downstream: Arc<Mutex<Downstream>>) -> Option<SetNewPrevHash<'static>> {
        downstream.super_safe_lock(|s| s.prevhash.clone())
    }

    pub fn get_clean_job(downstream: Arc<Mutex<Downstream>>) -> bool {
        downstream.super_safe_lock(|s| s.clean_job)
    }

    pub fn set_prevhash(downstream: Arc<Mutex<Downstream>>, prevhash: SetNewPrevHash<'static>) {
        downstream.safe_lock(|d| d.prevhash = Some(prevhash));
    }

    pub fn set_clean_job(downstream: Arc<Mutex<Downstream>>, clean_job: bool) {
        downstream.safe_lock(|d| d.clean_job = clean_job);
    }

    pub async fn bootstrap_non_aggregation(
        &mut self,
        connection: ConnectionSV1,
        downstream: &mut Downstream,
    ) -> ProxyResult<'static, Option<u32>> {
        let subscribe = connection.receiver().recv().await?;

        let mut channel_manager_to_sv1_server_receiver =
            self.channel_manager_to_sv1_server_receiver.subscribe();

        let open_upstream_channel = self
            .channel_opener_sender
            .send((downstream.downstream_id, "translator_worker".into()))
            .await;

        let open_upstream_channel_success = channel_manager_to_sv1_server_receiver.recv().await;

        if let Ok(Mining::OpenExtendedMiningChannelSuccess(msg)) = open_upstream_channel_success {
            downstream.extranonce1 = msg.extranonce_prefix.to_vec();
            downstream.extranonce2_len = msg.extranonce_size.into();
            downstream.channel_id = Some(msg.channel_id);
            let subscribe = downstream.handle_message(subscribe).unwrap().unwrap();
            connection.send(v1::Message::OkResponse(subscribe)).await;
            let authorize = connection.receiver().recv().await?;
            let authorize = downstream.handle_message(authorize).unwrap().unwrap();
            connection.send(v1::Message::OkResponse(authorize)).await;

            return Ok(Some(msg.channel_id));
        }
        Ok(None)
    }
}
