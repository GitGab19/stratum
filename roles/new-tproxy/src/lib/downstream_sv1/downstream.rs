use std::sync::Arc;

use async_channel::{Receiver, Sender};
use roles_logic_sv2::{
    common_properties::{CommonDownstreamData, IsDownstream, IsMiningDownstream},
    utils::Mutex,
};
use tracing::{debug, error, info, warn};
use v1::{
    client_to_server::{self, Submit},
    error::Error,
    json_rpc, server_to_client,
    utils::{Extranonce, HexU32Be},
    IsServer,
};

#[derive(Debug, Clone)]
pub struct Downstream {
    downstream_id: u32,
    downstream_sv1_sender: Sender<json_rpc::Message>,
    downstream_sv1_receiver: Receiver<json_rpc::Message>,
    sv1_server_sender: Sender<(u32, json_rpc::Message)>,
    sv1_server_receiver: Receiver<(u32, json_rpc::Message)>,
    extranonce1: Vec<u8>,
    extranonce2_len: usize,
    version_rolling_mask: Option<HexU32Be>,
    version_rolling_min_bit: Option<HexU32Be>,
    authorized_names: Vec<String>,
}

impl Downstream {
    pub fn new(
        downstream_id: u32,
        downstream_sv1_sender: Sender<json_rpc::Message>,
        downstream_sv1_receiver: Receiver<json_rpc::Message>,
        sv1_server_sender: Sender<(u32, json_rpc::Message)>,
        sv1_server_receiver: Receiver<(u32, json_rpc::Message)>,
    ) -> Self {
        Self {
            downstream_id,
            downstream_sv1_sender,
            downstream_sv1_receiver,
            sv1_server_sender,
            sv1_server_receiver,
            extranonce1: vec![0; 8],
            extranonce2_len: 0,
            version_rolling_mask: None,
            version_rolling_min_bit: None,
            authorized_names: Vec::new(),
        }
    }

    pub fn spawn_downstream_receiver(&self) {
        let mut downstream = self.clone();
        tokio::spawn(async move {
            info!("Downstream receiver task started.");
            while let Ok(message) = downstream.downstream_sv1_receiver.recv().await {
                debug!("Received message from downstream: {:?}", message);
                let response = downstream.handle_message(message);
                /*if let Err(e) = downstream.sv1_server_sender.send((downstream.downstream_id, message)).await {
                    error!("Failed to forward message to server: {:?}", e);
                }*/
            }
            warn!("Downstream receiver task ended.");
        });
    }

    pub fn spawn_downstream_sender(&self) {
        let downstream = self.clone();
        tokio::spawn(async move {
            info!("Downstream sender task started.");
            while let Ok(message) = downstream.sv1_server_receiver.recv().await {
                debug!("Sending message to downstream: {:?}", message);
                if let Err(e) = downstream.downstream_sv1_sender.send(message.1).await {
                    error!("Failed to send message to downstream: {:?}", e);
                }
            }
            warn!("Downstream sender task ended.");
        });
    }

    pub fn handle_incoming_sv1_messages(&mut self) {
        todo!()
    }

    pub async fn send_message_downstream(
        self_: Arc<Mutex<Self>>,
        response: json_rpc::Message,
    ) -> Result<(), async_channel::SendError<v1::Message>> {
        let sender = match self_.safe_lock(|s| s.downstream_sv1_sender.clone()) {
            Ok(sender) => sender,
            Err(e) => {
                error!("Failed to acquire downstream lock: {:?}", e);
                return Err(async_channel::SendError(response));
            }
        };

        debug!("Sending message to downstream via API: {:?}", response);
        sender.send(response).await
    }
}

// Implements `IsServer` for `Downstream` to handle the SV1 messages.
impl IsServer<'static> for Downstream {
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
        info!("Down: Submitting Share {:?}", request);
        debug!("Down: Handling mining.submit: {:?}", &request);

        self.sv1_server_sender
            .try_send((self.downstream_id, request.clone().into()));

        true
    }

    /// Indicates to the server that the client supports the mining.set_extranonce method.
    fn handle_extranonce_subscribe(&self) {}

    /// Checks if a Downstream role is authorized.
    fn is_authorized(&self, name: &str) -> bool {
        self.authorized_names.contains(&name.to_string())
    }

    /// Authorizes a Downstream role.
    fn authorize(&mut self, name: &str) {
        self.authorized_names.push(name.to_string());
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
