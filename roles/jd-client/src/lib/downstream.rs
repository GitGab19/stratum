use super::{config::JobDeclaratorClientConfig, template_receiver::TemplateRx, PoolChangerTrigger};

use super::{
    job_declarator::JobDeclarator,
    status::{self, State},
    upstream_sv2::Upstream as UpstreamMiningNode,
};
use async_channel::{bounded, Receiver, SendError, Sender};
use roles_logic_sv2::{
    channel_logic::channel_factory::{OnNewShare, PoolChannelFactory, Share},
    common_messages_sv2::{SetupConnection, SetupConnectionSuccess},
    common_properties::{CommonDownstreamData, IsDownstream, IsMiningDownstream},
    errors::Error,
    handlers::{
        common::{ParseCommonMessagesFromDownstream, SendTo as SendToCommon},
        mining::{ParseMiningMessagesFromDownstream, SendTo, SupportedChannelTypes},
    },
    job_creator::JobsCreators,
    mining_sv2::*,
    parsers::{AnyMessage, Mining, MiningDeviceMessages},
    template_distribution_sv2::{NewTemplate, SubmitSolution},
    utils::Mutex,
};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use codec_sv2::{HandshakeRole, Responder, StandardEitherFrame, StandardSv2Frame};
use key_utils::{Secp256k1PublicKey, Secp256k1SecretKey};

use stratum_common::bitcoin::{consensus::Decodable, TxOut};

pub type Message = MiningDeviceMessages<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type EitherFrame = StandardEitherFrame<Message>;

/// 1 to 1 connection with a downstream node that implement the mining (sub)protocol can be either
/// a mining device or a downstream proxy.
/// A downstream can only be linked with an upstream at a time. Support multi upstreams for
/// downstream do not make much sense.
#[derive(Debug)]
pub struct DownstreamMiningNode {
    receiver: Receiver<EitherFrame>,
    sender: Sender<EitherFrame>,
    pub status: DownstreamMiningNodeStatus,
    #[allow(dead_code)]
    pub prev_job_id: Option<u32>,
    solution_sender: Sender<SubmitSolution<'static>>,
    withhold: bool,
    task_collector: Arc<Mutex<Vec<AbortHandle>>>,
    tx_status: status::Sender,
    miner_coinbase_output: Vec<TxOut>,
    // used to retreive the job id of the share that we send upstream
    last_template_id: u64,
    pub jd: Option<Arc<Mutex<JobDeclarator>>>,
    jdc_signature: String,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum DownstreamMiningNodeStatus {
    Initializing(Option<Arc<Mutex<UpstreamMiningNode>>>),
    Paired((CommonDownstreamData, Arc<Mutex<UpstreamMiningNode>>)),
    ChannelOpened(
        (
            PoolChannelFactory,
            CommonDownstreamData,
            Arc<Mutex<UpstreamMiningNode>>,
        ),
    ),
    SoloMinerPaired(CommonDownstreamData),
    SoloMinerChannelOpend((PoolChannelFactory, CommonDownstreamData)),
}

impl DownstreamMiningNodeStatus {
    fn is_paired(&self) -> bool {
        match self {
            DownstreamMiningNodeStatus::Initializing(_) => false,
            DownstreamMiningNodeStatus::Paired(_) => true,
            DownstreamMiningNodeStatus::ChannelOpened(_) => true,
            DownstreamMiningNodeStatus::SoloMinerPaired(_) => true,
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(_) => true,
        }
    }

    fn pair(&mut self, data: CommonDownstreamData) {
        match self {
            DownstreamMiningNodeStatus::Initializing(Some(up)) => {
                let self_ = Self::Paired((data, up.clone()));
                let _ = std::mem::replace(self, self_);
            }
            DownstreamMiningNodeStatus::Initializing(None) => {
                let self_ = Self::SoloMinerPaired(data);
                let _ = std::mem::replace(self, self_);
            }
            _ => panic!("Try to pair an already paired downstream"),
        }
    }

    fn set_channel(&mut self, channel: PoolChannelFactory) -> bool {
        match self {
            DownstreamMiningNodeStatus::Initializing(_) => false,
            DownstreamMiningNodeStatus::Paired((data, up)) => {
                let self_ = Self::ChannelOpened((channel, *data, up.clone()));
                let _ = std::mem::replace(self, self_);
                true
            }
            DownstreamMiningNodeStatus::ChannelOpened(_) => false,
            DownstreamMiningNodeStatus::SoloMinerPaired(data) => {
                let self_ = Self::SoloMinerChannelOpend((channel, *data));
                let _ = std::mem::replace(self, self_);
                true
            }
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(_) => false,
        }
    }

    pub fn get_channel(&mut self) -> &mut PoolChannelFactory {
        match self {
            DownstreamMiningNodeStatus::Initializing(_) => panic!(),
            DownstreamMiningNodeStatus::Paired(_) => panic!(),
            DownstreamMiningNodeStatus::ChannelOpened((channel, _, _)) => channel,
            DownstreamMiningNodeStatus::SoloMinerPaired(_) => panic!(),
            DownstreamMiningNodeStatus::SoloMinerChannelOpend((channel, _)) => channel,
        }
    }
    fn have_channel(&self) -> bool {
        match self {
            DownstreamMiningNodeStatus::Initializing(_) => false,
            DownstreamMiningNodeStatus::Paired(_) => false,
            DownstreamMiningNodeStatus::ChannelOpened(_) => true,
            DownstreamMiningNodeStatus::SoloMinerPaired(_) => false,
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(_) => true,
        }
    }
    fn get_upstream(&mut self) -> Option<Arc<Mutex<UpstreamMiningNode>>> {
        match self {
            DownstreamMiningNodeStatus::Initializing(Some(up)) => Some(up.clone()),
            DownstreamMiningNodeStatus::Paired((_, up)) => Some(up.clone()),
            DownstreamMiningNodeStatus::ChannelOpened((_, _, up)) => Some(up.clone()),
            DownstreamMiningNodeStatus::Initializing(None) => None,
            DownstreamMiningNodeStatus::SoloMinerPaired(_) => None,
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(_) => None,
        }
    }
    fn is_solo_miner(&mut self) -> bool {
        matches!(
            self,
            DownstreamMiningNodeStatus::Initializing(None)
                | DownstreamMiningNodeStatus::SoloMinerPaired(_)
                | DownstreamMiningNodeStatus::SoloMinerChannelOpend(_)
        )
    }
}

use core::convert::TryInto;
use std::{net::IpAddr, str::FromStr, sync::Arc};

impl DownstreamMiningNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        receiver: Receiver<EitherFrame>,
        sender: Sender<EitherFrame>,
        upstream: Option<Arc<Mutex<UpstreamMiningNode>>>,
        solution_sender: Sender<SubmitSolution<'static>>,
        withhold: bool,
        task_collector: Arc<Mutex<Vec<AbortHandle>>>,
        tx_status: status::Sender,
        miner_coinbase_output: Vec<TxOut>,
        jd: Option<Arc<Mutex<JobDeclarator>>>,
        jdc_signature: String,
    ) -> Self {
        Self {
            receiver,
            sender,
            status: DownstreamMiningNodeStatus::Initializing(upstream),
            prev_job_id: None,
            solution_sender,
            withhold,
            task_collector,
            tx_status,
            miner_coinbase_output,
            // set it to an arbitrary value cause when we use it we always updated it.
            // Is used before sending the share to upstream in the main loop when we have a share.
            // Is upated in the message handler that si called earlier in the main loop.
            last_template_id: 0,
            jd,
            jdc_signature,
        }
    }

    /// Send SetupConnectionSuccess to downstream and start processing new messages coming from
    /// downstream
    pub async fn start(
        self_mutex: &Arc<Mutex<Self>>,
        setup_connection_success: SetupConnectionSuccess,
    ) {
        if self_mutex
            .safe_lock(|self_| self_.status.is_paired())
            .unwrap()
        {
            let setup_connection_success: MiningDeviceMessages = setup_connection_success.into();

            {
                DownstreamMiningNode::send(
                    self_mutex,
                    setup_connection_success.try_into().unwrap(),
                )
                .await
                .unwrap();
            }
            let receiver = self_mutex
                .safe_lock(|self_| self_.receiver.clone())
                .unwrap();
            Self::set_channel_factory(self_mutex.clone());

            while let Ok(message) = receiver.recv().await {
                let incoming: StdFrame = message.try_into().unwrap();
                Self::next(self_mutex, incoming).await;
            }
            let tx_status = self_mutex.safe_lock(|s| s.tx_status.clone()).unwrap();
            let err = Error::DownstreamDown;
            let status = status::Status {
                state: State::DownstreamShutdown(err.into()),
            };
            tx_status.send(status).await.unwrap();
        } else {
            panic!()
        }
    }

    // When we do pooled minig we create a channel factory when the pool send a open extended
    // mining channel success
    fn set_channel_factory(self_mutex: Arc<Mutex<Self>>) {
        if !self_mutex.safe_lock(|s| s.status.is_solo_miner()).unwrap() {
            // Safe unwrap already checked if it contains an upstream with `is_solo_miner`
            let upstream = self_mutex
                .safe_lock(|s| s.status.get_upstream().unwrap())
                .unwrap();
            let recv_factory = {
                let self_mutex = self_mutex.clone();
                tokio::task::spawn(async move {
                    let factory = UpstreamMiningNode::take_channel_factory(upstream).await;
                    self_mutex
                        .safe_lock(|s| {
                            s.status.set_channel(factory);
                        })
                        .unwrap();
                })
            };
            self_mutex
                .safe_lock(|s| {
                    s.task_collector
                        .safe_lock(|c| c.push(recv_factory.abort_handle()))
                        .unwrap()
                })
                .unwrap();
        }
    }

    /// Parse the received message and relay it to the right upstream
    pub async fn next(self_mutex: &Arc<Mutex<Self>>, mut incoming: StdFrame) {
        let message_type = incoming.get_header().unwrap().msg_type();
        let payload = incoming.payload();

        let next_message_to_send = ParseMiningMessagesFromDownstream::handle_message_mining(
            self_mutex.clone(),
            message_type,
            payload,
        );
        Self::match_send_to(self_mutex.clone(), next_message_to_send, Some(incoming)).await;
    }

    #[async_recursion::async_recursion]
    async fn match_send_to(
        self_mutex: Arc<Mutex<Self>>,
        next_message_to_send: Result<SendTo<UpstreamMiningNode>, Error>,
        incoming: Option<StdFrame>,
    ) {
        match next_message_to_send {
            Ok(SendTo::RelaySameMessageToRemote(upstream_mutex)) => {
                let sv2_frame: codec_sv2::Sv2Frame<AnyMessage, buffer_sv2::Slice> =
                    incoming.unwrap().map(|payload| payload.try_into().unwrap());
                UpstreamMiningNode::send(&upstream_mutex, sv2_frame)
                    .await
                    .unwrap();
            }
            Ok(SendTo::RelayNewMessage(Mining::SubmitSharesExtended(mut share))) => {
                // If we have a realy new message it means that we are in a pooled mining mods.
                let upstream_mutex = self_mutex
                    .safe_lock(|s| s.status.get_upstream().unwrap())
                    .unwrap();
                // When re receive SetupConnectionSuccess we link the last_template_id with the
                // pool's job_id. The below return as soon as we have a pairable job id for the
                // template_id associated with this share.
                let last_template_id = self_mutex.safe_lock(|s| s.last_template_id).unwrap();
                let job_id_future =
                    UpstreamMiningNode::get_job_id(&upstream_mutex, last_template_id);
                let job_id = match timeout(Duration::from_secs(10), job_id_future).await {
                    Ok(job_id) => job_id,
                    Err(_) => {
                        return;
                    }
                };
                share.job_id = job_id;
                debug!(
                    "Sending valid block solution upstream, with job_id {}",
                    job_id
                );
                let message = Mining::SubmitSharesExtended(share);
                let message: AnyMessage = AnyMessage::Mining(message);
                let sv2_frame: codec_sv2::Sv2Frame<AnyMessage, buffer_sv2::Slice> =
                    message.try_into().unwrap();
                UpstreamMiningNode::send(&upstream_mutex, sv2_frame)
                    .await
                    .unwrap();
            }
            Ok(SendTo::RelayNewMessage(message)) => {
                let message: AnyMessage = AnyMessage::Mining(message);
                let sv2_frame: codec_sv2::Sv2Frame<AnyMessage, buffer_sv2::Slice> =
                    message.try_into().unwrap();
                let upstream_mutex = self_mutex.safe_lock(|s| s.status.get_upstream().expect("We should return RelayNewMessage only if we are not in solo mining mode")).unwrap();
                UpstreamMiningNode::send(&upstream_mutex, sv2_frame)
                    .await
                    .unwrap();
            }
            Ok(SendTo::Multiple(messages)) => {
                for message in messages {
                    Self::match_send_to(self_mutex.clone(), Ok(message), None).await;
                }
            }
            Ok(SendTo::Respond(message)) => {
                let message = MiningDeviceMessages::Mining(message);
                let sv2_frame: codec_sv2::Sv2Frame<MiningDeviceMessages, buffer_sv2::Slice> =
                    message.try_into().unwrap();
                Self::send(&self_mutex, sv2_frame).await.unwrap();
            }
            Ok(SendTo::None(None)) => (),
            Ok(m) => unreachable!("Unexpected message type: {:?}", m),
            Err(_) => todo!(),
        }
    }

    /// Send a message downstream
    pub async fn send(
        self_mutex: &Arc<Mutex<Self>>,
        sv2_frame: StdFrame,
    ) -> Result<(), SendError<StdFrame>> {
        let either_frame = sv2_frame.into();
        let sender = self_mutex.safe_lock(|self_| self_.sender.clone()).unwrap();
        match sender.send(either_frame).await {
            Ok(_) => Ok(()),
            Err(_) => {
                todo!()
            }
        }
    }

    pub async fn on_new_template(
        self_mutex: &Arc<Mutex<Self>>,
        mut new_template: NewTemplate<'static>,
        pool_output: &[u8],
    ) -> Result<(), Error> {
        if !self_mutex.safe_lock(|s| s.status.have_channel()).unwrap() {
            super::IS_NEW_TEMPLATE_HANDLED.store(true, std::sync::atomic::Ordering::Release);
            return Ok(());
        }
        let mut pool_out = &pool_output[0..];
        let pool_output =
            TxOut::consensus_decode(&mut pool_out).expect("Upstream sent an invalid coinbase");
        let to_send = self_mutex
            .safe_lock(|s| {
                let channel = s.status.get_channel();
                channel.update_pool_outputs(vec![pool_output]);
                channel.on_new_template(&mut new_template)
            })
            .unwrap()?;
        // to_send is HashMap<channel_id, messages_to_send> but here we have only one downstream so
        // only one channel opened downstream. That means that we can take all the messages in the
        // map and send them downstream.
        let to_send = to_send.into_values();
        for message in to_send {
            let message = if let Mining::NewExtendedMiningJob(job) = message {
                if let Some(jd) = self_mutex.safe_lock(|s| s.jd.clone()).unwrap() {
                    jd.safe_lock(|jd| {
                        jd.coinbase_tx_prefix = job.coinbase_tx_prefix.clone();
                        jd.coinbase_tx_suffix = job.coinbase_tx_suffix.clone();
                    })
                    .unwrap();
                }
                Mining::NewExtendedMiningJob(job)
            } else {
                message
            };
            let message = MiningDeviceMessages::Mining(message);
            let frame: StdFrame = message.try_into().unwrap();
            Self::send(self_mutex, frame).await.unwrap();
        }
        // See coment on the definition of the global for memory
        // ordering
        super::IS_NEW_TEMPLATE_HANDLED.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub async fn on_set_new_prev_hash(
        self_mutex: &Arc<Mutex<Self>>,
        new_prev_hash: roles_logic_sv2::template_distribution_sv2::SetNewPrevHash<'static>,
    ) -> Result<(), Error> {
        if !self_mutex.safe_lock(|s| s.status.have_channel()).unwrap() {
            return Ok(());
        }
        let job_id = self_mutex
            .safe_lock(|s| {
                let channel = s.status.get_channel();
                channel.on_new_prev_hash_from_tp(&new_prev_hash)
            })
            .unwrap()?;
        let channel_ids = self_mutex
            .safe_lock(|s| s.status.get_channel().get_extended_channels_ids())
            .unwrap();
        let channel_id = match channel_ids.len() {
            1 => channel_ids[0],
            _ => unreachable!(),
        };
        let to_send = SetNewPrevHash {
            channel_id,
            job_id,
            prev_hash: new_prev_hash.prev_hash,
            min_ntime: new_prev_hash.header_timestamp,
            nbits: new_prev_hash.n_bits,
        };
        let message = MiningDeviceMessages::Mining(Mining::SetNewPrevHash(to_send));
        let frame = message.try_into().unwrap();
        Self::send(self_mutex, frame).await.unwrap();
        Ok(())
    }
}

/// It impl UpstreamMining cause the proxy act as an upstream node for the DownstreamMiningNode
impl ParseMiningMessagesFromDownstream<UpstreamMiningNode> for DownstreamMiningNode {
    fn get_channel_type(&self) -> SupportedChannelTypes {
        SupportedChannelTypes::Extended
    }

    fn is_work_selection_enabled(&self) -> bool {
        false
    }

    fn is_downstream_authorized(
        _self_mutex: Arc<Mutex<Self>>,
        _user_identity: &Str0255,
    ) -> Result<bool, Error> {
        Ok(true)
    }

    fn handle_open_standard_mining_channel(
        &mut self,
        _: OpenStandardMiningChannel,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        warn!("Ignoring OpenStandardMiningChannel");
        Ok(SendTo::None(None))
    }

    fn handle_open_extended_mining_channel(
        &mut self,
        m: OpenExtendedMiningChannel,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        info!(
            "Received OpenExtendedMiningChannel from: {} with id: {}",
            std::str::from_utf8(m.user_identity.as_ref()).unwrap_or("Unknown identity"),
            m.get_request_id_as_u32()
        );
        debug!("OpenExtendedMiningChannel: {:?}", m);
        if !self.status.is_solo_miner() {
            // Safe unwrap alreay checked if it cointains upstream with is_solo_miner
            Ok(SendTo::RelaySameMessageToRemote(
                self.status.get_upstream().unwrap(),
            ))
        } else {
            // The channel factory is created here so that we are sure that if we have a channel
            // open we have a factory and if we have a factory we have a channel open. This allowto
            // not change the semantic of Status beween solo and pooled modes
            let extranonce_len = 32;
            let jdc_signature_len = self.jdc_signature.len();
            let range_0 = std::ops::Range { start: 0, end: 0 };

            // JDC only allows for one single downstream, so we don't need any free bytes on range_1
            // we just allocate enough space for the JDC signature
            let range_1 = std::ops::Range {
                start: 0,
                end: jdc_signature_len,
            };
            let range_2 = std::ops::Range {
                start: jdc_signature_len,
                end: extranonce_len,
            };
            let ids = Arc::new(Mutex::new(roles_logic_sv2::utils::GroupId::new()));
            let coinbase_outputs = self.miner_coinbase_output.clone();

            let extranonces = ExtendedExtranonce::new(
                range_0,
                range_1,
                range_2,
                Some(self.jdc_signature.as_bytes().to_vec()),
            )
            .map_err(|_| {
                roles_logic_sv2::Error::ExtendedExtranonceCreationFailed(
                    "Failed to create ExtendedExtranonce".into(),
                )
            })?;
            let creator = JobsCreators::new(extranonce_len as u8);
            let share_per_min = 1.0;
            let kind = roles_logic_sv2::channel_logic::channel_factory::ExtendedChannelKind::Pool;
            let channel_factory = PoolChannelFactory::new(
                ids,
                extranonces,
                creator,
                share_per_min,
                kind,
                coinbase_outputs,
            );
            self.status.set_channel(channel_factory);

            let request_id = m.request_id;
            let hash_rate = m.nominal_hash_rate;
            let min_extranonce_size = m.min_extranonce_size;
            let messages_res = self.status.get_channel().new_extended_channel(
                request_id,
                hash_rate,
                min_extranonce_size,
            );
            match messages_res {
                Ok(messages) => {
                    let messages = messages.into_iter().map(SendTo::Respond).collect();
                    Ok(SendTo::Multiple(messages))
                }
                Err(_) => Err(roles_logic_sv2::Error::ChannelIsNeitherExtendedNeitherInAPool),
            }
        }
    }

    fn handle_update_channel(
        &mut self,
        m: UpdateChannel,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        info!("Received UpdateChannel message");
        if !self.status.is_solo_miner() {
            // Safe unwrap alreay checked if it cointains upstream with is_solo_miner
            Ok(SendTo::RelaySameMessageToRemote(
                self.status.get_upstream().unwrap(),
            ))
        } else {
            let maximum_target =
                roles_logic_sv2::utils::hash_rate_to_target(m.nominal_hash_rate.into(), 10.0)?;
            self.status
                .get_channel()
                .update_target_for_channel(m.channel_id, maximum_target.clone().into());
            let set_target = SetTarget {
                channel_id: m.channel_id,
                maximum_target,
            };
            Ok(SendTo::Respond(Mining::SetTarget(set_target)))
        }
    }

    fn handle_submit_shares_standard(
        &mut self,
        _: SubmitSharesStandard,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        warn!("Ignoring SubmitSharesStandard");
        Ok(SendTo::None(None))
    }

    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        info!("Received SubmitSharesExtended message");
        debug!("SubmitSharesExtended {:?}", m);
        match self
            .status
            .get_channel()
            .on_submit_shares_extended(m.clone())
            .unwrap()
        {
            OnNewShare::SendErrorDownstream(s) => {
                error!("Share does not meet the downstream target");
                Ok(SendTo::Respond(Mining::SubmitSharesError(s)))
            }
            OnNewShare::SendSubmitShareUpstream((m, Some(template_id))) => {
                if !self.status.is_solo_miner() {
                    match m {
                        Share::Extended(share) => {
                            let for_upstream = Mining::SubmitSharesExtended(share);
                            self.last_template_id = template_id;
                            Ok(SendTo::RelayNewMessage(for_upstream))
                        }
                        // We are in an extended channel shares are extended
                        Share::Standard(_) => unreachable!(),
                    }
                } else {
                    Ok(SendTo::None(None))
                }
            }
            OnNewShare::RelaySubmitShareUpstream => unreachable!(),
            OnNewShare::ShareMeetBitcoinTarget((
                share,
                Some(template_id),
                coinbase,
                extranonce,
            )) => {
                match share {
                    Share::Extended(share) => {
                        let solution_sender = self.solution_sender.clone();
                        let solution = SubmitSolution {
                            template_id,
                            version: share.version,
                            header_timestamp: share.ntime,
                            header_nonce: share.nonce,
                            coinbase_tx: coinbase.try_into()?,
                        };
                        // The below channel should never be full is ok to block
                        solution_sender.send_blocking(solution).unwrap();
                        if !self.status.is_solo_miner() {
                            {
                                let jd = self.jd.clone();
                                let mut share = share.clone();
                                share.extranonce = extranonce.try_into().unwrap();
                                tokio::task::spawn(async move {
                                    JobDeclarator::on_solution(&jd.unwrap(), share).await
                                });
                            }
                        }

                        // Safe unwrap alreay checked if it cointains upstream with is_solo_miner
                        if !self.withhold && !self.status.is_solo_miner() {
                            self.last_template_id = template_id;
                            let for_upstream = Mining::SubmitSharesExtended(share);
                            Ok(SendTo::RelayNewMessage(for_upstream))
                        } else {
                            Ok(SendTo::None(None))
                        }
                    }
                    // We are in an extended channel shares are extended
                    Share::Standard(_) => unreachable!(),
                }
            }
            // When we have a ShareMeetBitcoinTarget it means that the proxy know the bitcoin
            // target that means that the proxy must have JD capabilities that means that the
            // second tuple elements can not be None but must be Some(template_id)
            OnNewShare::ShareMeetBitcoinTarget(_) => unreachable!(),
            OnNewShare::SendSubmitShareUpstream(_) => unreachable!(),
            OnNewShare::ShareMeetDownstreamTarget => Ok(SendTo::None(None)),
        }
    }

    fn handle_set_custom_mining_job(
        &mut self,
        _: SetCustomMiningJob,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        warn!("Ignoring SetCustomMiningJob");
        Ok(SendTo::None(None))
    }
}

impl ParseCommonMessagesFromDownstream for DownstreamMiningNode {
    fn handle_setup_connection(
        &mut self,
        m: SetupConnection,
    ) -> Result<roles_logic_sv2::handlers::common::SendTo, Error> {
        info!(
            "Received `SetupConnection`: version={}, flags={:b}",
            m.min_version, m.flags
        );
        let response = SetupConnectionSuccess {
            used_version: 2,
            // require extended channels
            flags: 0b0000_0000_0000_0010,
        };
        let data = CommonDownstreamData {
            header_only: false,
            work_selection: false,
            version_rolling: true,
        };
        self.status.pair(data);
        Ok(SendToCommon::Respond(response.into()))
    }
}

use binary_sv2::Str0255;
use network_helpers_sv2::noise_connection::Connection;
use std::net::SocketAddr;
use tokio::{
    net::TcpListener,
    task::AbortHandle,
    time::{timeout, Duration},
};

/// Start listen for downstream mining node. Return as soon as one downstream connect.
#[allow(clippy::too_many_arguments)]
pub async fn listen_for_downstream_mining(
    address: SocketAddr,
    upstream: Option<Arc<Mutex<UpstreamMiningNode>>>,
    withhold: bool,
    authority_public_key: Secp256k1PublicKey,
    authority_secret_key: Secp256k1SecretKey,
    cert_validity_sec: u64,
    task_collector: Arc<Mutex<Vec<AbortHandle>>>,
    tx_status: async_channel::Sender<status::Status<'static>>,
    miner_coinbase_output: Vec<TxOut>,
    jd: Option<Arc<Mutex<JobDeclarator>>>,
    config: JobDeclaratorClientConfig,
    shutdown: Arc<Notify>,
    jdc_signature: String,
) {
    info!("Listening for downstream mining connections on {}", address);
    let listener = TcpListener::bind(address).await.unwrap();
    let mut has_downstream = false;
    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                info!("Shutdown signal received. Stopping downstream mining listener.");
                break;
            }
            Ok((stream, _)) = listener.accept() => {
                if has_downstream {
                    error!("A downstream connection is already active. Ignoring additional connections.");
                    continue;
                }
                has_downstream = true;
                let task_collector = task_collector.clone();
                let miner_coinbase_output = miner_coinbase_output.clone();
                let jd = jd.clone();
                let upstream = upstream.clone();
                let timeout = config.timeout();
                let mut parts = config.tp_address().split(':');
                let ip_tp = parts.next().unwrap().to_string();
                let port_tp = parts.next().unwrap().parse::<u16>().unwrap();

                let (send_solution, recv_solution) = bounded(10);

                let responder = Responder::from_authority_kp(
                    &authority_public_key.into_bytes(),
                    &authority_secret_key.into_bytes(),
                    std::time::Duration::from_secs(cert_validity_sec),
                )
                .unwrap();
                let (receiver, sender) =
                    Connection::new(stream, HandshakeRole::Responder(responder))
                        .await
                        .expect("impossible to connect");

                let tx_status_downstream = status::Sender::Downstream(tx_status.clone());
                let node = DownstreamMiningNode::new(
                    receiver,
                    sender,
                    upstream.clone(),
                    send_solution,
                    withhold,
                    task_collector.clone(),
                    tx_status_downstream,
                    miner_coinbase_output,
                    jd.clone(),
                    jdc_signature.clone(),
                );

        let mut incoming: StdFrame = node.receiver.recv().await.unwrap().try_into().unwrap();
        let message_type = incoming.get_header().unwrap().msg_type();
        let payload = incoming.payload();
        let node = Arc::new(Mutex::new(node));
        if let Some(upstream) = upstream {
            upstream
                .safe_lock(|s| s.downstream = Some(node.clone()))
                .unwrap();
        }

                if let Ok(SendToCommon::Respond(message)) = DownstreamMiningNode::handle_message_common(
                    node.clone(),
                    message_type,
                    payload,
                ) {
                    let message = match message {
                        roles_logic_sv2::parsers::CommonMessages::SetupConnectionSuccess(m) => m,
                        _ => panic!(),
                    };

                    let main_task = tokio::task::spawn({
                        let node = node.clone();
                        async move {
                            DownstreamMiningNode::start(&node, message).await;
                        }
                    });

                    node.safe_lock(|n| {
                        n.task_collector
                            .safe_lock(|c| {
                                c.push(main_task.abort_handle());
                            })
                            .unwrap()
                    })
                    .unwrap();

                    TemplateRx::connect(
                        SocketAddr::new(IpAddr::from_str(ip_tp.as_str()).unwrap(), port_tp),
                        recv_solution,
                        status::Sender::TemplateReceiver(tx_status.clone()),
                        jd,
                        node,
                        task_collector,
                        Arc::new(Mutex::new(PoolChangerTrigger::new(timeout))),
                        vec![],
                        config.tp_authority_public_key().cloned(),
                    )
                    .await;
                }
            }
        }
    }

    info!("Downstream mining listener has shut down.");
}

impl IsDownstream for DownstreamMiningNode {
    fn get_downstream_mining_data(&self) -> CommonDownstreamData {
        match self.status {
            DownstreamMiningNodeStatus::Initializing(_) => panic!(),
            DownstreamMiningNodeStatus::Paired((data, _)) => data,
            DownstreamMiningNodeStatus::ChannelOpened((_, data, _)) => data,
            DownstreamMiningNodeStatus::SoloMinerPaired(data) => data,
            DownstreamMiningNodeStatus::SoloMinerChannelOpend((_, data)) => data,
        }
    }
}
impl IsMiningDownstream for DownstreamMiningNode {}
