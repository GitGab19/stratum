use crate::{
    downstream_sv1::downstream::Downstream,
    error::Error,
    upstream_sv2::upstream::{EitherFrame, Message, StdFrame},
    utils::{into_static, message_from_frame},
};
use async_channel::{Receiver, Sender};
use binary_sv2::{to_bytes, u256_from_int};
use codec_sv2::{Frame, Sv2Frame};
use framing_sv2::header::Header;
use roles_logic_sv2::{
    channels::client::{extended::ExtendedChannel, share_accounting::ShareValidationError},
    handlers::{
        common::ParseCommonMessagesFromUpstream,
        mining::{ParseMiningMessagesFromUpstream, SendTo},
    },
    mining_sv2::{OpenExtendedMiningChannel, SubmitSharesError, SubmitSharesSuccess},
    parsers::{AnyMessage, IsSv2Message, Mining},
    utils::Mutex,
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

pub type Sv2Message = Mining<'static>;

/*#[derive(Debug, Clone)]
pub enum ChannelMappingMode {
    // This is the mode where each client has its own channel.
    PerClient,
    // This is the mode where all clients share the same channel.
    Aggregated,
}*/

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelMappingMode {
    PerClient,
    Aggregated,
}

#[derive(Debug, Clone)]
pub struct ChannelManager {
    upstream_sender: Sender<EitherFrame>,
    upstream_receiver: Receiver<EitherFrame>,
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    sv1_server_sender: Sender<Mining<'static>>,
    sv1_server_receiver: Receiver<Mining<'static>>,
    mode: ChannelMappingMode,
    // Store pending channel info by downstream_id
    pub pending_channels: HashMap<u32, (String, f32)>, // (user_identity, hashrate)
}

impl ChannelManager {
    pub fn new(
        upstream_sender: Sender<EitherFrame>,
        upstream_receiver: Receiver<EitherFrame>,
        sv1_server_sender: Sender<Mining<'static>>,
        sv1_server_receiver: Receiver<Mining<'static>>,
        mode: ChannelMappingMode,
    ) -> Self {
        Self {
            upstream_sender,
            upstream_receiver,
            extended_channels: HashMap::new(),
            sv1_server_sender,
            sv1_server_receiver,
            mode,
            pending_channels: HashMap::new(),
        }
    }

    pub async fn on_upstream_message(self_: Arc<Mutex<Self>>) {
        info!("Starting on upstream message in channel manager");
        tokio::spawn(async move {
            let (
                upstream_receiver,
                upstream_sender,
                sv1_server_sender,
            ) = self_.super_safe_lock(|e| {
                (
                    e.upstream_receiver.clone(),
                    e.upstream_sender.clone(),
                    e.sv1_server_sender.clone(),
                )
            });
            while let Ok(message) = upstream_receiver.recv().await {
                if let Frame::Sv2(mut frame) = message {
                    if let Some(header) = frame.get_header() {
                        let message_type = header.msg_type();

                        let mut payload = frame.payload().to_vec();
                        // let mut payload1 = payload.clone();
                        let message: AnyMessage<'_> =
                            into_static((message_type, payload.as_mut_slice()).try_into().unwrap());

                        match message {
                            Message::Mining(mining_message) => {
                                let message =
                                    ParseMiningMessagesFromUpstream::handle_message_mining(
                                        self_.clone(),
                                        message_type,
                                        payload.as_mut_slice(),
                                    );
                                if let Ok(message) = message {
                                    match message {
                                        SendTo::Respond(message_for_upstream) => {
                                            let message = Message::Mining(message_for_upstream);

                                            let frame: StdFrame = message.try_into().unwrap();
                                            let frame: EitherFrame = frame.into();
                                            upstream_sender.send(frame).await;
                                        }
                                        SendTo::None(Some(m)) => {
                                            match m {
                                                Mining::SetNewPrevHash(v) => {
                                                    sv1_server_sender   
                                                    .send(Mining::SetNewPrevHash(v.clone())).await;
                                                    let active_job = self_.super_safe_lock(|c| {
                                                        c.extended_channels.get(&v.channel_id)
                                                            .and_then(|extended_channel| {
                                                                extended_channel.read().ok()
                                                                    .and_then(|channel| channel.get_active_job()
                                                                        .map(|job| job.0.clone()))
                                                            })
                                                    });
                                                    if let Some(active_job) = active_job {
                                                        sv1_server_sender.send(
                                                            Mining::NewExtendedMiningJob(active_job)
                                                        ).await;
                                                    }
                                                }
                                                Mining::CloseChannel(_) => todo!(),
                                                Mining::NewExtendedMiningJob(v) => {
                                                    if v.is_future() {
                                                        continue; // we wait for the SetNewPrevHash in this case and we don't send anything to sv1 server
                                                    }
                                                    sv1_server_sender.send(Mining::NewExtendedMiningJob(v.clone())).await;
                                                },
                                                Mining::NewMiningJob(_) => unreachable!(),
                                                Mining::OpenExtendedMiningChannel(_) => unreachable!(),
                                                Mining::OpenExtendedMiningChannelSuccess(v) => {
                                                    sv1_server_sender.send(Mining::OpenExtendedMiningChannelSuccess(v.clone())).await;
                                                },
                                                Mining::OpenMiningChannelError(_) => todo!(),
                                                Mining::OpenStandardMiningChannel(_) => todo!(),
                                                Mining::OpenStandardMiningChannelSuccess(_) => todo!(),
                                                Mining::SetCustomMiningJob(_) => todo!(),
                                                Mining::SetCustomMiningJobError(_) => todo!(),
                                                Mining::SetCustomMiningJobSuccess(_) => todo!(),
                                                Mining::SetExtranoncePrefix(_) => todo!(),
                                                Mining::SetGroupChannel(_) => todo!(),
                                                Mining::SetTarget(_) => todo!(),
                                                Mining::SubmitSharesError(_) => todo!(),
                                                Mining::SubmitSharesExtended(_) => todo!(),
                                                Mining::SubmitSharesStandard(_) => todo!(),
                                                Mining::SubmitSharesSuccess(_) => todo!(),
                                                Mining::UpdateChannel(_) => todo!(),
                                                Mining::UpdateChannelError(_) => todo!(),
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Message::Common(common_message) => {
                                debug!("Handling common message from upstream.");
                                ParseCommonMessagesFromUpstream::handle_message_common(
                                    self_.clone(),
                                    message_type,
                                    payload.as_mut_slice(),
                                );
                            }
                            _ => {
                                warn!("Received unknown message type from upstream: {:?}", message);
                            }
                        }
                    }
                }
            }
        });
    }

    pub async fn on_downstream_message(self_: Arc<Mutex<Self>>) {
        info!("Starting on upstream message in channel manager");
        tokio::spawn(async move {
            let (
                sv1_server_receiver,
                sv1_server_sender,
                upstream_sender,
            ) = self_.super_safe_lock(|e| {
                (
                    e.sv1_server_receiver.clone(),
                    e.sv1_server_sender.clone(),
                    e.upstream_sender.clone(),
                )
            });
            while let Ok(message) = sv1_server_receiver.recv().await {
                match message {
                    Mining::SubmitSharesExtended(m) => {
                        //let m = m.clone();
                        error!("Received share validation from downstream: {:?}", m);
                        error!("Time to validate");
                        let value = self_.super_safe_lock(|c| {
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
                            let share_validation_success = SubmitSharesSuccess {
                                channel_id: m.channel_id,
                                last_sequence_number: share_accounting
                                    .get_last_share_sequence_number(),
                                new_shares_sum: share_accounting.get_share_work_sum(),
                                new_submits_accepted_count: share_accounting.get_shares_accepted(),
                            };
                            sv1_server_sender
                                .send(Mining::SubmitSharesSuccess(share_validation_success));
                            
                            // send the share message to upstream.
                            let share_message = Message::Mining(roles_logic_sv2::parsers::Mining::SubmitSharesExtended(m.clone()));
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
                                .send(Mining::SubmitSharesError(share_validation_error));
                        }
                    },
                    Mining::OpenExtendedMiningChannel(m) => {
                        let user_identity = std::str::from_utf8(m.user_identity.as_ref())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        let hashrate = m.nominal_hash_rate;
                        // Store the user identity and hashrate for this downstream
                        self_.super_safe_lock(|c| {
                            c.pending_channels.insert(m.request_id, (user_identity, hashrate));
                        });
                        let _ = Self::open_extended_mining_channel(self_.super_safe_lock(|c| c.clone()), m).await;
                    },
                    _ => {}
                }
            }
        });
    }

    pub async fn open_extended_mining_channel(
        self,
        open_channel: OpenExtendedMiningChannel<'static>,
    ) -> Result<(), Error<'static>> {
        info!("Opening extended mining channel in {:?}", self.mode);
        if self.mode == ChannelMappingMode::PerClient {
            let frame = StdFrame::try_from(Message::Mining(roles_logic_sv2::parsers::Mining::OpenExtendedMiningChannel(open_channel))).unwrap();
            self.upstream_sender
                .send(frame.into())
                .await
                .map_err(|e| {
                    // TODO: Handle this error
                    error!("Failed to send open channel message to upstream: {:?}", e);
                    e
                });
        } else {
            // TODO: Implement this
            // Here we need to create a new extranonce prefix using a ExtendedExtranonceFactory
            todo!()
        }
        
        Ok(())
    }
}
