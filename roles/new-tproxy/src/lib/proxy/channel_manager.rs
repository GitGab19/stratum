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

#[derive(Debug, Clone)]
pub struct ChannelManager {
    channel_manager_to_upstream_sender: Sender<EitherFrame>,
    upstream_to_channel_manager_receiver: Receiver<EitherFrame>,
    pub extended_channels: HashMap<u32, Arc<RwLock<ExtendedChannel<'static>>>>,
    channel_manager_to_sv1_server_sender: broadcast::Sender<Mining<'static>>,
    sv1_server_to_channel_manager_receiver: Receiver<(u32, Mining<'static>)>,
    channel_opener_receiver: Receiver<(u32, String)>,
}

impl ChannelManager {
    pub fn new(
        channel_manager_to_upstream_sender: Sender<EitherFrame>,
        upstream_to_channel_manager_receiver: Receiver<EitherFrame>,
        channel_manager_to_sv1_server_sender: broadcast::Sender<Mining<'static>>,
        sv1_server_to_channel_manager_receiver: Receiver<(u32, Mining<'static>)>,
        channel_opener_receiver: Receiver<(u32, String)>,
    ) -> Self {
        tokio::spawn(Self::create_channel(
            channel_opener_receiver.clone(),
            channel_manager_to_upstream_sender.clone(),
        ));
        Self {
            channel_manager_to_upstream_sender,
            upstream_to_channel_manager_receiver,
            extended_channels: HashMap::new(),
            channel_manager_to_sv1_server_sender,
            sv1_server_to_channel_manager_receiver,
            channel_opener_receiver,
        }
    }

    pub async fn on_upstream_message(self_: Arc<Mutex<Self>>) {
        info!("Starting on upstream message in channel manager");
        tokio::spawn(async move {
            let (
                upstream_to_channel_manager_receiver,
                channel_manager_to_upstream_sender,
                channel_manager_to_sv1_server_sender,
            ) = self_.super_safe_lock(|e| {
                (
                    e.upstream_to_channel_manager_receiver.clone(),
                    e.channel_manager_to_upstream_sender.clone(),
                    e.channel_manager_to_sv1_server_sender.clone(),
                )
            });
            while let Ok(message) = upstream_to_channel_manager_receiver.recv().await {
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
                                            channel_manager_to_upstream_sender.send(frame).await;
                                        }
                                        SendTo::None(Some(m)) => {
                                            if let Mining::SetNewPrevHash(v) = m {
                                                channel_manager_to_sv1_server_sender
                                                    .send(Mining::SetNewPrevHash(v.clone()));
                                                let extended_channel = self_.super_safe_lock(|c| {
                                                    c.extended_channels.get(&v.channel_id).cloned()
                                                });
                                                if let Some(extended_channel) = extended_channel {
                                                    let channel = extended_channel.read().unwrap();
                                                    let active_job = channel.get_active_job();
                                                    if let Some(active_job) = active_job {
                                                        channel_manager_to_sv1_server_sender.send(
                                                            Mining::NewExtendedMiningJob(
                                                                active_job.0.clone(),
                                                            ),
                                                        );
                                                    }
                                                }
                                            } else {
                                                channel_manager_to_sv1_server_sender.send(m);
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

    pub async fn handle_downstream_message(self_: Arc<Mutex<Self>>) {
        info!("Starting on upstream message in channel manager");
        tokio::spawn(async move {
            let (
                sv1_server_to_channel_manager_receiver,
                channel_manager_to_sv1_server_sender,
                channel_manager_to_upstream_sender,
            ) = self_.super_safe_lock(|e| {
                (
                    e.sv1_server_to_channel_manager_receiver.clone(),
                    e.channel_manager_to_sv1_server_sender.clone(),
                    e.channel_manager_to_upstream_sender.clone(),
                )
            });
            while let Ok((downstream_id, message)) =
                sv1_server_to_channel_manager_receiver.recv().await
            {
                // send the share message to upstream.
                let share_message = Message::Mining(message.clone());
                let frame: StdFrame = share_message.try_into().unwrap();
                let frame: EitherFrame = frame.into();
                channel_manager_to_upstream_sender.send(frame).await;

                // This we gonna mostly and only gonna use for share validation.
                match message {
                    Mining::SubmitSharesExtended(m) => {
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
                            channel_manager_to_sv1_server_sender
                                .send(Mining::SubmitSharesSuccess(share_validation_success));
                        } else {
                            let share_validation_error = SubmitSharesError {
                                channel_id: m.channel_id,
                                sequence_number: m.sequence_number,
                                error_code: "do better match on error"
                                    .to_string()
                                    .try_into()
                                    .expect("error code must be valid string"),
                            };

                            channel_manager_to_sv1_server_sender
                                .send(Mining::SubmitSharesError(share_validation_error));
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    pub async fn create_channel(
        channel_opener_receiver: Receiver<(u32, String)>,
        channel_manager_sender: Sender<EitherFrame>,
    ) -> Result<(), Error<'static>> {
        while let Ok((downstream_id, workername)) = channel_opener_receiver.recv().await {
            let open_channel = Mining::OpenExtendedMiningChannel(OpenExtendedMiningChannel {
                request_id: downstream_id,
                user_identity: workername.try_into()?,
                nominal_hash_rate: 1000.0,           // TODO
                max_target: u256_from_int(u64::MAX), // TODO
                min_extranonce_size: 4,              // TODO
            });
            let frame = StdFrame::try_from(Message::Mining(open_channel)).unwrap();
            channel_manager_sender
                .send(frame.into())
                .await
                .map_err(|e| {
                    // TODO: Handle this error
                    error!("Failed to send open channel message to upstream: {:?}", e);
                    e
                });
        }
        Ok(())
    }
}
