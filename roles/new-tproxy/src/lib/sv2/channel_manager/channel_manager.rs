use crate::{
    config::TranslatorConfig,
    error::{Error, ProxyResult},
    sv2::upstream::upstream::{EitherFrame, Message, StdFrame},
    utils::into_static,
};
use async_channel::{Receiver, Sender};
use codec_sv2::Frame;
use roles_logic_sv2::{
    channels::client::extended::ExtendedChannel,
    handlers::mining::{ParseMiningMessagesFromUpstream, SendTo},
    mining_sv2::{
        ExtendedExtranonce, OpenExtendedMiningChannel, OpenExtendedMiningChannelSuccess,
        SubmitSharesError, SubmitSharesSuccess, Target,
    },
    parsers::{AnyMessage, IsSv2Message, Mining},
    utils::{hash_rate_to_target, Mutex},
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

pub type Sv2Message = Mining<'static>;

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub enum ChannelMode {
    Aggregated,
    NonAggregated,
}

#[derive(Clone, Debug)]
pub struct ChannelState {
    upstream_sender: Sender<EitherFrame>,
    upstream_receiver: Receiver<EitherFrame>,
    sv1_server_sender: Sender<Mining<'static>>,
    sv1_server_receiver: Receiver<Mining<'static>>,
}

impl ChannelState {
    pub fn new(
        upstream_sender: Sender<EitherFrame>,
        upstream_receiver: Receiver<EitherFrame>,
        sv1_server_sender: Sender<Mining<'static>>,
        sv1_server_receiver: Receiver<Mining<'static>>,
    ) -> Self {
        Self {
            upstream_sender,
            upstream_receiver,
            sv1_server_sender,
            sv1_server_receiver,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelManagerData {
    // Store pending channel info by downstream_id
    pub pending_channels: HashMap<u32, (String, f32, usize)>, /* (user_identity, hashrate,
                                                               * downstream_extranonce_len) */
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    pub extranonce_prefix_factory_extended: Option<Arc<Mutex<ExtendedExtranonce>>>,

    pub mode: ChannelMode,
}

impl ChannelManagerData {
    fn new(mode: ChannelMode) -> Self {
        Self {
            pending_channels: HashMap::new(),
            extended_channels: HashMap::new(),
            extranonce_prefix_factory_extended: None,
            mode,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChannelManager {
    channel_state: ChannelState,
    channel_manager_data: Arc<Mutex<ChannelManagerData>>,
}

impl ChannelManager {
    pub fn new(
        upstream_sender: Sender<EitherFrame>,
        upstream_receiver: Receiver<EitherFrame>,
        sv1_server_sender: Sender<Mining<'static>>,
        sv1_server_receiver: Receiver<Mining<'static>>,
        mode: ChannelMode,
    ) -> Self {
        let channel_state = ChannelState::new(
            upstream_sender,
            upstream_receiver,
            sv1_server_sender,
            sv1_server_receiver,
        );
        let channel_manager_data = Arc::new(Mutex::new(ChannelManagerData::new(mode)));
        Self {
            channel_state,
            channel_manager_data,
        }
    }

    pub async fn run_channel_manager_tasks(
        self: Arc<Self>,
        notify_shutdown: broadcast::Sender<()>,
        shutdown_complete_tx: mpsc::Sender<()>,
    ) {
        let mut shutdown_rx = notify_shutdown.subscribe();
        info!("Spawning run channel manager task");
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("ChannelManager: received shutdown signal.");
                        break;
                    }
                    Some(_) = Self::handle_upstream_message(self.clone()) => {},
                    Some(_) = Self::handle_downstream_message(self.clone()) => {},
                    else => {
                        warn!("All channel manager message streams closed. Exiting...");
                        break;
                    }
                }
            }

            self.channel_state.upstream_receiver.close();
            self.channel_state.upstream_sender.close();
            self.channel_state.sv1_server_receiver.close();
            self.channel_state.sv1_server_sender.close();
            drop(shutdown_complete_tx);
            warn!("ChannelManager: unified message loop exited.");
        });
    }

    pub async fn handle_upstream_message(self: Arc<Self>) -> Option<()> {
        match self.channel_state.upstream_receiver.recv().await {
            Ok(message) => {
                if let Frame::Sv2(mut frame) = message {
                    if let Some(header) = frame.get_header() {
                        let message_type = header.msg_type();

                        let mut payload = frame.payload().to_vec();
                        // let mut payload1 = payload.clone();
                        let message: AnyMessage<'_> =
                            into_static((message_type, payload.as_mut_slice()).try_into().unwrap())
                                .unwrap();

                        match message {
                            Message::Mining(mining_message) => {
                                let message =
                                    ParseMiningMessagesFromUpstream::handle_message_mining(
                                        self.channel_manager_data.clone(),
                                        message_type,
                                        payload.as_mut_slice(),
                                    );
                                if let Ok(message) = message {
                                    match message {
                                        SendTo::Respond(message_for_upstream) => {
                                            let message = Message::Mining(message_for_upstream);

                                            let frame: StdFrame = message.try_into().unwrap();
                                            let frame: EitherFrame = frame.into();
                                            self.channel_state.upstream_sender.send(frame).await;
                                        }
                                        SendTo::None(Some(m)) => {
                                            match m {
                                                // Implemented message handlers
                                                Mining::SetNewPrevHash(v) => {
                                                    self.channel_state
                                                        .sv1_server_sender
                                                        .send(Mining::SetNewPrevHash(v.clone()))
                                                        .await;
                                                    let active_job = self
                                                        .channel_manager_data
                                                        .super_safe_lock(|c| {
                                                            c.extended_channels
                                                                .get(&v.channel_id)
                                                                .and_then(|extended_channel| {
                                                                    extended_channel
                                                                        .read()
                                                                        .ok()
                                                                        .and_then(|channel| {
                                                                            channel
                                                                                .get_active_job()
                                                                                .map(|job| {
                                                                                    job.0.clone()
                                                                                })
                                                                        })
                                                                })
                                                        });
                                                    if let Some(active_job) = active_job {
                                                        self.channel_state
                                                            .sv1_server_sender
                                                            .send(Mining::NewExtendedMiningJob(
                                                                active_job,
                                                            ))
                                                            .await;
                                                    }
                                                }
                                                Mining::NewExtendedMiningJob(v) => {
                                                    if !v.is_future() {
                                                        self.channel_state
                                                            .sv1_server_sender
                                                            .send(Mining::NewExtendedMiningJob(
                                                                v.clone(),
                                                            ))
                                                            .await;
                                                    }
                                                }
                                                Mining::OpenExtendedMiningChannelSuccess(v) => {
                                                    self.channel_state.sv1_server_sender.send(Mining::OpenExtendedMiningChannelSuccess(v.clone())).await;
                                                }

                                                // TODO: Implement these handlers
                                                Mining::OpenMiningChannelError(_) => todo!(),
                                                // Unreachable - not supported in this
                                                // implementation
                                                _ => unreachable!(),
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {
                                warn!("Received unknown message type from upstream: {:?}", message);
                            }
                        }
                    }
                }
                Some(())
            }
            Err(e) => None,
        }
    }

    pub async fn handle_downstream_message(self: Arc<Self>) -> Option<()> {
        match self.channel_state.sv1_server_receiver.recv().await {
            Ok(message) => {
                match message {
                    Mining::SubmitSharesExtended(m) => {
                        info!(
                            "ChannelManager received SubmitSharesExtended message: {:?}",
                            m
                        );
                        let value = self.channel_manager_data.super_safe_lock(|c| {
                            let extended_channel = c.extended_channels.get(&m.channel_id);
                            if let Some(extended_channel) = extended_channel {
                                let channel = extended_channel.write();
                                if let Ok(mut channel) = channel {
                                    return Some((
                                        channel.validate_share(m.clone()),
                                        channel.get_share_accounting().clone(),
                                    ));
                                }
                            }
                            None
                        });
                        /*if let Some((Ok(result), share_accounting)) = value {
                            let share_validation_success = SubmitSharesSuccess {
                                channel_id: m.channel_id,
                                last_sequence_number: share_accounting
                                    .get_last_share_sequence_number(),
                                new_shares_sum: share_accounting.get_share_work_sum(),
                                new_submits_accepted_count: share_accounting.get_shares_accepted(),
                            };
                            sv1_server_sender
                                .send(Mining::SubmitSharesSuccess(share_validation_success))
                                .await;

                            // send the share message to upstream.
                            let share_message = Message::Mining(
                                roles_logic_sv2::parsers::Mining::SubmitSharesExtended(m.clone()),
                            );
                            let frame: StdFrame = share_message.try_into().unwrap();
                            let frame: EitherFrame = frame.into();
                            upstream_sender.send(frame).await;
                        } else {
                            let share_validation_error = SubmitSharesError {
                                channel_id: m.channel_id,
                                sequence_number: m.sequence_number,
                                error_code: "do better match on error"
                                    .to_string()
                                    .try_into()
                                    .expect("error code must be valid string"),
                            };

                            sv1_server_sender
                                .send(Mining::SubmitSharesError(share_validation_error))
                                .await;
                        }*/
                    }
                    Mining::OpenExtendedMiningChannel(m) => {
                        let mut open_channel_msg = m.clone();
                        let mut user_identity = std::str::from_utf8(m.user_identity.as_ref())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        let hashrate = m.nominal_hash_rate;
                        let min_extranonce_size = m.min_extranonce_size as usize;
                        let (mode, channels_are_empty) = self
                            .channel_manager_data
                            .super_safe_lock(|c| (c.mode.clone(), c.extended_channels.is_empty()));

                        if mode == ChannelMode::Aggregated {
                            if !channels_are_empty {
                                // We already have the unique channel open and so we create a new
                                // extranonce prefix and we send the
                                // OpenExtendedMiningChannelSuccess message directly to the sv1
                                // server
                                let (channel_id, target) = self.channel_manager_data.super_safe_lock(|c| c.extended_channels.iter().next()
                                    .map(|(id, channel)| {
                                        let target = channel.read().unwrap().get_target().clone();
                                        (*id, target)
                                    })
                                    .expect("Expected at least one extended channel in aggregated mode"));
                                let new_extranonce_prefix =
                                    self.channel_manager_data.super_safe_lock(|c| {
                                        c.extranonce_prefix_factory_extended
                                            .as_ref()
                                            .unwrap()
                                            .safe_lock(|e| {
                                                e.next_prefix_extended(
                                                    open_channel_msg.min_extranonce_size.into(),
                                                )
                                            })
                                            .ok()
                                            .and_then(|r| r.ok())
                                    });
                                if let Some(new_extranonce_prefix) = new_extranonce_prefix {
                                    let success_message = Mining::OpenExtendedMiningChannelSuccess(
                                        OpenExtendedMiningChannelSuccess {
                                            request_id: open_channel_msg.request_id,
                                            channel_id: channel_id,
                                            target: target.clone().into(),
                                            extranonce_size: open_channel_msg.min_extranonce_size,
                                            extranonce_prefix: new_extranonce_prefix.clone().into(),
                                        },
                                    );
                                    self.channel_state
                                        .sv1_server_sender
                                        .send(success_message)
                                        .await
                                        .map_err(|e| {
                                            error!(
                                            "Failed to send open channel message to upstream: {:?}",
                                            e
                                        );
                                            e
                                        });
                                }
                                return Some(());
                            } else {
                                // We don't have the unique channel open yet and so we send the
                                // OpenExtendedMiningChannel message to the upstream
                                // Before doing that we need to truncate the user identity at the
                                // first dot and append .translator-proxy
                                // Truncate at the first dot and append .translator-proxy
                                let translator_identity =
                                    if let Some(dot_index) = user_identity.find('.') {
                                        format!("{}.translator-proxy", &user_identity[..dot_index])
                                    } else {
                                        format!("{}.translator-proxy", user_identity)
                                    };
                                user_identity = translator_identity;
                                open_channel_msg.user_identity =
                                    user_identity.as_bytes().to_vec().try_into().unwrap();
                            }
                        }

                        // Store the user identity and hashrate
                        self.channel_manager_data.super_safe_lock(|c| {
                            c.pending_channels.insert(
                                open_channel_msg.request_id,
                                (user_identity, hashrate, min_extranonce_size),
                            );
                        });

                        let frame = StdFrame::try_from(Message::Mining(
                            roles_logic_sv2::parsers::Mining::OpenExtendedMiningChannel(
                                open_channel_msg,
                            ),
                        ))
                        .unwrap();

                        self.channel_state
                            .upstream_sender
                            .send(frame.into())
                            .await
                            .map_err(|e| {
                                error!("Failed to send open channel message to upstream: {:?}", e);
                                e
                            });
                    }
                    _ => {}
                }
                Some(())
            }
            Err(e) => None,
        }
    }

    /*pub async fn open_extended_mining_channel(
        self,
        open_channel: OpenExtendedMiningChannel<'static>,
    ) -> Result<(), Error<'static>> {
        info!("Opening extended mining channel in {:?}", self.mode);
        if self.mode == ChannelMode::NonAggregated {
            let frame = StdFrame::try_from(Message::Mining(
                roles_logic_sv2::parsers::Mining::OpenExtendedMiningChannel(open_channel),
            ))
            .unwrap();
            self.upstream_sender.send(frame.into()).await.map_err(|e| {
                // TODO: Handle this error
                error!("Failed to send open channel message to upstream: {:?}", e);
                e
            });
        } else {
            if self.extended_channels.is_empty() {
               // We need to open the unique channel which will be used by every client
               let user_identity_str = std::str::from_utf8(open_channel.user_identity.as_ref())
                   .map(|s| s.to_string())
                   .unwrap_or_else(|_| "unknown".to_string());
               // Truncate at the first dot and append .translator-proxy
               let truncated_identity = if let Some(dot_index) = user_identity_str.find('.') {
                   format!("{}.translator-proxy", &user_identity_str[..dot_index])
               } else {
                   format!("{}.translator-proxy", user_identity_str)
               };
               let user_identity = truncated_identity.as_bytes().to_vec();

               let open_extended_mining_channel = OpenExtendedMiningChannel {
                    request_id: 0,
                    user_identity: user_identity.try_into()?,
                    nominal_hash_rate: open_channel.nominal_hash_rate,
                    min_extranonce_size: open_channel.min_extranonce_size,
                    max_target: open_channel.max_target,
                };
                let frame = StdFrame::try_from(Message::Mining(
                    roles_logic_sv2::parsers::Mining::OpenExtendedMiningChannel(open_extended_mining_channel),
                ))
                .unwrap();
                self.upstream_sender.send(frame.into()).await.map_err(|e| {
                    error!("Failed to send open channel message to upstream: {:?}", e);
                    e
                });
            } else {
                let (channel_id, target) = self.extended_channels.iter().next()
                    .map(|(id, channel)| {
                        let target = channel.read().unwrap().get_target().clone();
                        (*id, target)
                    })
                    .expect("Expected at least one extended channel in aggregated mode");
                let extranonce_result = self.extranonce_prefix_factory_extended.as_ref().unwrap().safe_lock(|e| {
                    e.next_prefix_extended(open_channel.min_extranonce_size.into())
                });
                if let Ok(Ok(new_extranonce_prefix)) = extranonce_result {
                    let success_message = Mining::OpenExtendedMiningChannelSuccess(OpenExtendedMiningChannelSuccess {
                        request_id: open_channel.request_id,
                        channel_id: channel_id,
                        target: target.clone().into(),
                        extranonce_size: open_channel.min_extranonce_size,
                        extranonce_prefix: new_extranonce_prefix.clone().into(),
                    });
                    self.sv1_server_sender.send(success_message).await.map_err(|e| {
                        error!("Failed to send open channel message to upstream: {:?}", e);
                        e
                    });
                }
            }
        }

        Ok(())
    }*/
}
