use crate::{proxy::channel_manager::ChannelManager, downstream_sv1::Downstream};
use roles_logic_sv2::{
    common_messages_sv2::SetupConnectionSuccess,   
    handlers::{
        common::{ParseCommonMessagesFromUpstream, SendTo as SendToCommon},
        mining::{ParseMiningMessagesFromUpstream, SendTo},
    },
    mining_sv2::{
        NewExtendedMiningJob,
        OpenExtendedMiningChannelSuccess, SetNewPrevHash, SetTarget,
    },
    Error as RolesLogicError, parsers::Mining,
};

impl ParseCommonMessagesFromUpstream for ChannelManager {
    fn handle_setup_connection_success(
        &mut self,
        m: SetupConnectionSuccess,
    ) -> Result<SendToCommon, RolesLogicError> {
        todo!()
    }
    
    fn handle_setup_connection_error(&mut self, m: roles_logic_sv2::common_messages_sv2::SetupConnectionError) -> Result<SendToCommon, RolesLogicError> {
        todo!()
    }
    
    fn handle_channel_endpoint_changed(
        &mut self,
        m: roles_logic_sv2::common_messages_sv2::ChannelEndpointChanged,
    ) -> Result<SendToCommon, RolesLogicError> {
        todo!()
    }
    
    fn handle_reconnect(&mut self, m: roles_logic_sv2::common_messages_sv2::Reconnect) -> Result<SendToCommon, RolesLogicError> {
        todo!()
    }
}

impl ParseMiningMessagesFromUpstream<Downstream> for ChannelManager {
    fn get_channel_type(&self) -> roles_logic_sv2::handlers::mining::SupportedChannelTypes {
        todo!()
    }

    fn is_work_selection_enabled(&self) -> bool {
        todo!()
    }

    fn handle_open_standard_mining_channel_success(
        &mut self,
        m: roles_logic_sv2::mining_sv2::OpenStandardMiningChannelSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_open_extended_mining_channel_success(
        &mut self,
        m: OpenExtendedMiningChannelSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        Ok(SendTo::None(Some(Mining::OpenExtendedMiningChannelSuccess(m))))
    }

    fn handle_open_mining_channel_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::OpenMiningChannelError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_update_channel_error(&mut self, m: roles_logic_sv2::mining_sv2::UpdateChannelError)
        -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_close_channel(&mut self, m: roles_logic_sv2::mining_sv2::CloseChannel) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_set_extranonce_prefix(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SetExtranoncePrefix,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_submit_shares_success(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SubmitSharesSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_submit_shares_error(&mut self, m: roles_logic_sv2::mining_sv2::SubmitSharesError) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_new_mining_job(&mut self, m: roles_logic_sv2::mining_sv2::NewMiningJob) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_new_extended_mining_job(
        &mut self,
        m: NewExtendedMiningJob,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_set_new_prev_hash(&mut self, m: SetNewPrevHash) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_set_custom_mining_job_success(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SetCustomMiningJobSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_set_custom_mining_job_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SetCustomMiningJobError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_set_target(&mut self, m: SetTarget) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_set_group_channel(&mut self, _m: roles_logic_sv2::mining_sv2::SetGroupChannel) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }
}