use std::{net::SocketAddr, sync::Arc};
use async_channel::{Sender, Receiver};
use roles_logic_sv2::utils::{Mutex, Id as IdFactory};
use tokio::net::TcpListener;
use v1::{json_rpc, IsServer, client_to_server, server_to_client, utils::{Extranonce, HexU32Be}, error::Error};
use crate::{proxy::ChannelManager, error::ProxyResult, downstream_sv1::Downstream};
use network_helpers_sv2::sv1_connection::ConnectionSV1;

pub struct Sv1Server {
    channel_manager: Arc<Mutex<ChannelManager>>,
    downstream_sender: Sender<json_rpc::Message>,
    downstream_receiver: Receiver<json_rpc::Message>,
    downstream_id_factory: IdFactory,
    downstream_addr: SocketAddr,
}

impl Sv1Server {
    pub fn new(channel_manager: Arc<Mutex<ChannelManager>>, downstream_sender: Sender<json_rpc::Message>, downstream_receiver: Receiver<json_rpc::Message>, downstream_addr: SocketAddr) -> Self {
        Self {  
            channel_manager,
            downstream_sender,
            downstream_receiver,
            downstream_id_factory: IdFactory::new(),
            downstream_addr,
        }
    }

    pub fn start(&self) -> ProxyResult<'static, ()> {
        let accept_connections = tokio::task::spawn({
            async move {
                let listener = TcpListener::bind(self.downstream_addr).await.unwrap();
                while let Ok((stream, _)) = listener.accept().await {
                    let connection = ConnectionSV1::new(stream).await;
                    let downstream = Downstream::new(connection.sender(), connection.receiver(), self.downstream_sender, self.downstream_receiver);
                    let downstream_id = self.downstream_id_factory.next();
                    self.channel_manager.safe_lock(|s| s.downstreams.insert(downstream_id, Arc::new(Mutex::new(downstream))));
                    downstream.spawn_downstream_receiver();
                    downstream.spawn_downstream_sender();
                }
            }
        });
        Ok(())
    }
}

// Implements `IsServer` for `Sv1Server` to handle the SV1 messages.
impl IsServer<'static> for Sv1Server {
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