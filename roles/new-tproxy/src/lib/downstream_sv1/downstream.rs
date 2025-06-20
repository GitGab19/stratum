use std::sync::Arc;

use async_channel::{Receiver, Sender};
use roles_logic_sv2::{
    common_properties::{CommonDownstreamData, IsDownstream, IsMiningDownstream},
    utils::Mutex,
};
use tracing::debug;
use v1::{
    client_to_server,
    error::Error,
    json_rpc, server_to_client,
    utils::{Extranonce, HexU32Be},
    IsServer,
};

#[derive(Debug, Clone)]
pub struct Downstream {
    downstream_sv1_sender: Sender<json_rpc::Message>,
    downstream_sv1_receiver: Receiver<json_rpc::Message>,
    sv1_server_sender: Sender<json_rpc::Message>,
    sv1_server_receiver: Receiver<json_rpc::Message>,
}

impl Downstream {
    pub fn new(
        downstream_sv1_sender: Sender<json_rpc::Message>,
        downstream_sv1_receiver: Receiver<json_rpc::Message>,
        sv1_server_sender: Sender<json_rpc::Message>,
        sv1_server_receiver: Receiver<json_rpc::Message>,
    ) -> Self {
        Self {
            downstream_sv1_sender,
            downstream_sv1_receiver,
            sv1_server_sender,
            sv1_server_receiver,
        }
    }

    pub fn spawn_downstream_receiver(&self) {
        let downstream = self.clone();
        tokio::spawn(async move {
            while let Ok(message) = downstream.downstream_sv1_receiver.recv().await {
                downstream.sv1_server_sender.send(message).await.unwrap();
            }
        });
    }

    pub fn spawn_downstream_sender(&self) {
        let downstream = self.clone();
        tokio::spawn(async move {
            while let Ok(message) = downstream.sv1_server_receiver.recv().await {
                downstream
                    .downstream_sv1_sender
                    .send(message)
                    .await
                    .unwrap();
            }
        });
    }

    pub fn handle_incoming_sv1_messages(&mut self) {
        todo!()
    }
    /// Sends a SV1 JSON-RPC message to the downstream miner's socket writer task.
    ///
    /// This method is used to send response messages or notifications (like
    /// `mining.notify` or `mining.set_difficulty`) to the connected miner.
    /// The message is sent over the internal `tx_outgoing` channel, which is
    /// read by the socket writer task responsible for serializing and writing
    /// the message to the TCP stream.
    pub async fn send_message_downstream(
        self_: Arc<Mutex<Self>>,
        response: json_rpc::Message,
    ) -> Result<(), async_channel::SendError<v1::Message>> {
        let sender = self_
            .safe_lock(|s| s.downstream_sv1_sender.clone())
            .unwrap();
        debug!("To DOWN: {:?}", response);
        sender.send(response).await
    }
}

// This is the implementation of the server side of the SV1 crate
impl IsServer<'static> for Downstream {
    fn handle_configure(
        &mut self,
        request: &client_to_server::Configure,
    ) -> (Option<server_to_client::VersionRollingParams>, Option<bool>) {
        todo!()
    }

    fn handle_subscribe(&self, request: &client_to_server::Subscribe) -> Vec<(String, String)> {
        todo!()
    }

    fn handle_authorize(&self, request: &client_to_server::Authorize) -> bool {
        todo!()
    }

    fn handle_submit(&self, request: &client_to_server::Submit<'static>) -> bool {
        todo!()
    }

    fn handle_extranonce_subscribe(&self) {
        todo!()
    }

    fn is_authorized(&self, name: &str) -> bool {
        todo!()
    }

    fn authorize(&mut self, name: &str) {
        todo!()
    }

    fn set_extranonce1(&mut self, extranonce1: Option<Extranonce<'static>>) -> Extranonce<'static> {
        todo!()
    }

    fn extranonce1(&self) -> Extranonce<'static> {
        todo!()
    }

    fn set_extranonce2_size(&mut self, extra_nonce2_size: Option<usize>) -> usize {
        todo!()
    }

    fn extranonce2_size(&self) -> usize {
        todo!()
    }

    fn version_rolling_mask(&self) -> Option<HexU32Be> {
        todo!()
    }

    fn set_version_rolling_mask(&mut self, mask: Option<HexU32Be>) {
        todo!()
    }

    fn set_version_rolling_min_bit(&mut self, mask: Option<HexU32Be>) {
        todo!()
    }

    fn notify(&mut self) -> Result<json_rpc::Message, Error> {
        todo!()
    }
}

// This is needed just to satisfy the handler trait
impl IsMiningDownstream for Downstream {}

impl IsDownstream for Downstream {
    fn get_downstream_mining_data(&self) -> CommonDownstreamData {
        todo!()
    }
}
