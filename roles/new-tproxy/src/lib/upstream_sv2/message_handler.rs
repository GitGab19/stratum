use roles_logic_sv2::{common_messages_sv2::{ChannelEndpointChanged, Reconnect, SetupConnectionError, SetupConnectionSuccess}, handlers::common::{ParseCommonMessagesFromUpstream, SendTo as SendToCommon}, Error};
use tracing::info;
use crate::upstream_sv2::Upstream;

impl ParseCommonMessagesFromUpstream for Upstream {
    fn handle_setup_connection_success(
        &mut self,
        m: SetupConnectionSuccess,
    ) -> Result<SendToCommon, Error> {
        info!(
            "Received `SetupConnectionSuccess`: version={}, flags={:b}",
            m.used_version, m.flags
        );
        Ok(SendToCommon::None(None))
    }

    fn handle_setup_connection_error(&mut self, m: SetupConnectionError) -> Result<SendToCommon, Error> {
        todo!()
    }

    fn handle_channel_endpoint_changed(
        &mut self,
        m: ChannelEndpointChanged,
    ) -> Result<SendToCommon, Error> {
        todo!()
    }

    fn handle_reconnect(&mut self, m: Reconnect) -> Result<SendToCommon, Error> {
        todo!()
    }
}