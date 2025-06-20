use crate::{downstream_sv1::Downstream, error::ProxyResult, proxy::ChannelManager};
use async_channel::{Receiver, Sender};
use network_helpers_sv2::sv1_connection::ConnectionSV1;
use roles_logic_sv2::{
    parsers::Mining,
    utils::{Id as IdFactory, Mutex},
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tracing::{error, info, warn};
use v1::{
    client_to_server,
    error::Error,
    json_rpc, server_to_client,
    utils::{Extranonce, HexU32Be},
    IsServer,
};

pub struct Sv1Server {
    channel_manager: Arc<Mutex<ChannelManager>>,
    downstream_id_factory: IdFactory,
    downstream_sender: Sender<(u32, json_rpc::Message)>,
    downstream_receiver: Receiver<(u32, json_rpc::Message)>,
    downstreams: HashMap<u32, Downstream>,
    listener_addr: SocketAddr,
    sv1_server_receiver: Receiver<Mining<'static>>,
    channel_opener_sender: Sender<(u32, String)>,
}

impl Sv1Server {
    pub fn new(
        channel_manager: Arc<Mutex<ChannelManager>>,
        downstream_sender: Sender<(u32, json_rpc::Message)>,
        downstream_receiver: Receiver<(u32, json_rpc::Message)>,
        listener_addr: SocketAddr,
        sv1_server_receiver: Receiver<Mining<'static>>,
        channel_opener_sender: Sender<(u32, String)>,
    ) -> Self {
        Self {
            channel_manager,
            downstream_sender,
            downstream_receiver,
            downstream_id_factory: IdFactory::new(),
            downstreams: HashMap::new(),
            listener_addr,
            sv1_server_receiver,
            channel_opener_sender,
        }
    }

    pub async fn start(&mut self) -> ProxyResult<'static, ()> {
        info!("Starting SV1 server on {}", self.listener_addr);

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
                    let mut downstream = Downstream::new(
                        downstream_id,
                        connection.sender().clone(),
                        connection.receiver().clone(),
                        self.downstream_sender.clone(),
                        self.downstream_receiver.clone(),
                    );

                    self.downstreams.insert(downstream_id, downstream.clone());

                    let subscribe = connection.receiver().recv().await?;

                    let subscribe = downstream.handle_message(subscribe);

                    let authorize = connection.receiver().recv().await?;

                    let authorize = downstream.handle_message(authorize);

                    let open_upstream_channel = self
                        .channel_opener_sender
                        .send((downstream_id, "translator_worker".into()))
                        .await;

                    let open_upstream_channel_success = self.sv1_server_receiver.recv().await;

                    if let Ok(msg) = open_upstream_channel_success {
                        match msg {
                            Mining::OpenExtendedMiningChannelSuccess(m) => {}
                            Mining::NewExtendedMiningJob(m) => {}
                            Mining::SetNewPrevHash(m) => {}
                            Mining::CloseChannel(_m) => {}
                            Mining::OpenMiningChannelError(_)
                            | Mining::UpdateChannelError(_)
                            | Mining::SubmitSharesError(_)
                            | Mining::SetCustomMiningJobError(_) => {}
                            // impossible state: handle_message_mining only returns
                            // the above 3 messages in the Ok(SendTo::None(Some(m))) case to be sent
                            // to the bridge for translation.
                            _ => panic!(),
                        }
                    }

                    // We are going to receive a subscribe message from the downstream.
                    // We need to send random values to the sv1 downstream.
                    // We are going to receive a authorize message from the downstream.
                    // Now we can create the channel for the downstream (using the workername)
                    // We need to send a SetExtranonce message to the downstream.
                    // We need to send a Notify message to the downstream.

                    // NOW WE ARE READY TO HANDLE THE SUBMIT SHARES

                    info!("Downstream {} registered successfully", downstream_id);
                    downstream.spawn_downstream_receiver();
                    downstream.spawn_downstream_sender();
                }
                Err(e) => {
                    warn!("Failed to accept new connection: {:?}", e);
                }
            }
        }
    }

    pub async fn handle_downstream_message(
        &mut self,
        message: (u32, json_rpc::Message),
    ) -> ProxyResult<'static, ()> {
        while let Ok((downstream_id, message)) = self.downstream_receiver.recv().await {}
        Ok(())
    }
}
