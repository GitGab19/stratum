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
    pub upstream_extended_channel: Option<Arc<RwLock<ExtendedChannel<'static>>>>, /* This is the upstream extended channel that is used in aggregated mode */
    pub extranonce_prefix_factory: Option<Arc<Mutex<ExtendedExtranonce>>>,        /* This is the
                                                                                   * extranonce
                                                                                   * prefix
                                                                                   * factory that is
                                                                                   * used in aggregated
                                                                                   * mode to allocate
                                                                                   * unique extranonce
                                                                                   * prefixes */

    pub mode: ChannelMode,
}

impl ChannelManagerData {
    fn new(mode: ChannelMode) -> Self {
        Self {
            pending_channels: HashMap::new(),
            extended_channels: HashMap::new(),
            upstream_extended_channel: None,
            extranonce_prefix_factory: None,
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
                                                    let mode = self
                                                        .channel_manager_data
                                                        .super_safe_lock(|c| c.mode.clone());
                                                    let active_job = if mode
                                                        == ChannelMode::Aggregated
                                                    {
                                                        self.channel_manager_data.super_safe_lock(
                                                            |c| {
                                                                c.upstream_extended_channel
                                                                    .as_ref()
                                                                    .unwrap()
                                                                    .read()
                                                                    .unwrap()
                                                                    .get_active_job()
                                                                    .map(|job| job.0.clone())
                                                            },
                                                        )
                                                    } else {
                                                        self.channel_manager_data.super_safe_lock(
                                                            |c| {
                                                                c.extended_channels
                                                                    .get(&v.channel_id)
                                                                    .and_then(|extended_channel| {
                                                                        extended_channel
                                                                            .read()
                                                                            .ok()
                                                                            .and_then(|channel| {
                                                                                channel
                                                                                    .get_active_job(
                                                                                    )
                                                                                    .map(|job| {
                                                                                        job.0
                                                                                            .clone()
                                                                                    })
                                                                            })
                                                                    })
                                                            },
                                                        )
                                                    };

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
                    Mining::SubmitSharesExtended(mut m) => {
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
                        if let Some((Ok(result), share_accounting)) = value {
                            let mode = self
                                .channel_manager_data
                                .super_safe_lock(|c| c.mode.clone());
                            if mode == ChannelMode::Aggregated {
                                if self
                                    .channel_manager_data
                                    .super_safe_lock(|c| c.upstream_extended_channel.is_some())
                                {
                                    let upstream_extended_channel_id =
                                        self.channel_manager_data.super_safe_lock(|c| {
                                            let upstream_extended_channel = c
                                                .upstream_extended_channel
                                                .as_ref()
                                                .unwrap()
                                                .read()
                                                .unwrap();
                                            upstream_extended_channel.get_channel_id()
                                        });
                                    m.channel_id = upstream_extended_channel_id; // We need to set the channel id to the upstream extended
                                                                                 // channel id
                                                                                 // Get the downstream channel's extranonce prefix (contains
                                                                                 // upstream prefix + translator proxy prefix)
                                    let downstream_extranonce_prefix =
                                        self.channel_manager_data.super_safe_lock(|c| {
                                            c.extended_channels.get(&m.channel_id).map(|channel| {
                                                channel
                                                    .read()
                                                    .unwrap()
                                                    .get_extranonce_prefix()
                                                    .clone()
                                            })
                                        });
                                    // Get the length of the upstream prefix (range0)
                                    let range0_len =
                                        self.channel_manager_data.super_safe_lock(|c| {
                                            c.extranonce_prefix_factory
                                                .as_ref()
                                                .unwrap()
                                                .safe_lock(|e| e.get_range0_len())
                                                .unwrap()
                                        });
                                    if let Some(downstream_extranonce_prefix) =
                                        downstream_extranonce_prefix
                                    {
                                        // Skip the upstream prefix (range0) and take the remaining
                                        // bytes (translator proxy prefix)
                                        let translator_prefix =
                                            &downstream_extranonce_prefix[range0_len..];
                                        // Create new extranonce: translator proxy prefix + miner's
                                        // extranonce
                                        let mut new_extranonce = translator_prefix.to_vec();
                                        new_extranonce.extend_from_slice(m.extranonce.as_ref());
                                        // Replace the original extranonce with the modified one for
                                        // upstream submission
                                        m.extranonce = new_extranonce.try_into().unwrap();
                                    }
                                }
                            }
                            let frame: StdFrame = Message::Mining(Mining::SubmitSharesExtended(m))
                                .try_into()
                                .unwrap();
                            let frame: EitherFrame = frame.into();
                            self.channel_state.upstream_sender.send(frame).await;
                        }
                    }
                    Mining::OpenExtendedMiningChannel(m) => {
                        let mut open_channel_msg = m.clone();
                        let mut user_identity = std::str::from_utf8(m.user_identity.as_ref())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        let hashrate = m.nominal_hash_rate;
                        let min_extranonce_size = m.min_extranonce_size as usize;
                        let mode = self
                            .channel_manager_data
                            .super_safe_lock(|c| c.mode.clone());

                        if mode == ChannelMode::Aggregated {
                            if self
                                .channel_manager_data
                                .super_safe_lock(|c| c.upstream_extended_channel.is_some())
                            {
                                // We already have the unique channel open and so we create a new
                                // extranonce prefix and we send the
                                // OpenExtendedMiningChannelSuccess message directly to the sv1
                                // server
                                let target = self.channel_manager_data.super_safe_lock(|c| {
                                    c.upstream_extended_channel
                                        .as_ref()
                                        .unwrap()
                                        .read()
                                        .unwrap()
                                        .get_target()
                                        .clone()
                                });
                                let new_extranonce_prefix =
                                    self.channel_manager_data.super_safe_lock(|c| {
                                        c.extranonce_prefix_factory
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
                                let new_extranonce_size =
                                    self.channel_manager_data.super_safe_lock(|c| {
                                        c.extranonce_prefix_factory
                                            .as_ref()
                                            .unwrap()
                                            .safe_lock(|e| e.get_range2_len())
                                            .unwrap()
                                    });
                                if let Some(new_extranonce_prefix) = new_extranonce_prefix {
                                    if new_extranonce_size
                                        >= open_channel_msg.min_extranonce_size as usize
                                    {
                                        let next_channel_id =
                                            self.channel_manager_data.super_safe_lock(|c| {
                                                c.extended_channels.keys().max().unwrap_or(&0) + 1
                                            });
                                        let new_downstream_extended_channel = ExtendedChannel::new(
                                            next_channel_id,
                                            user_identity.clone(),
                                            new_extranonce_prefix
                                                .clone()
                                                .into_b032()
                                                .into_static()
                                                .to_vec(),
                                            target.clone().into(),
                                            hashrate,
                                            true,
                                            new_extranonce_size as u16,
                                        );
                                        self.channel_manager_data.super_safe_lock(|c| {
                                            c.extended_channels.insert(
                                                next_channel_id,
                                                Arc::new(RwLock::new(
                                                    new_downstream_extended_channel,
                                                )),
                                            );
                                        });
                                        let success_message =
                                            Mining::OpenExtendedMiningChannelSuccess(
                                                OpenExtendedMiningChannelSuccess {
                                                    request_id: open_channel_msg.request_id,
                                                    channel_id: next_channel_id,
                                                    target: target.clone().into(),
                                                    extranonce_size: new_extranonce_size as u16,
                                                    extranonce_prefix: new_extranonce_prefix
                                                        .clone()
                                                        .into(),
                                                },
                                            );
                                        self.channel_state.sv1_server_sender.send(success_message).await.map_err(|e| {
                                            error!("Failed to send open channel message to upstream: {:?}", e);
                                            e
                                        });
                                    }
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
}
