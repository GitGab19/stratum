use std::sync::{Arc, RwLock};

use crate::{downstream_sv1::downstream::Downstream, proxy::ChannelManager};
use roles_logic_sv2::{
    channels::client::extended::ExtendedChannel,
    common_messages_sv2::Protocol,
    common_properties::{IsMiningUpstream, IsUpstream},
    handlers::mining::{ParseMiningMessagesFromUpstream, SendTo, SupportedChannelTypes},
    mining_sv2::{
        NewExtendedMiningJob, OpenExtendedMiningChannelSuccess, SetNewPrevHash, SetTarget,
    },
    parsers::Mining,
    Error as RolesLogicError,
};
use tracing::{debug, info};
impl ParseMiningMessagesFromUpstream<Downstream> for ChannelManager {
    fn get_channel_type(&self) -> roles_logic_sv2::handlers::mining::SupportedChannelTypes {
        SupportedChannelTypes::Extended
    }

    fn is_work_selection_enabled(&self) -> bool {
        false
    }

    fn handle_open_standard_mining_channel_success(
        &mut self,
        m: roles_logic_sv2::mining_sv2::OpenStandardMiningChannelSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        unreachable!()
    }

    fn handle_open_extended_mining_channel_success(
        &mut self,
        m: OpenExtendedMiningChannelSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        let nominal_hashrate = 100000.0; //TODO
        info!(
            "Received OpenExtendedMiningChannelSuccess with request id: {} and channel id: {}",
            m.request_id, m.channel_id
        );
        debug!("OpenStandardMiningChannelSuccess: {:?}", m);
        info!("Up: Successfully Opened Extended Mining Channel");
        let extranonce_prefix = m.extranonce_prefix.clone().into_static().to_vec();
        let target = m.target.clone().into_static();
        let version_rolling = true; // we assume this is always true on extended channels
        let extended_channel = ExtendedChannel::new(
            m.channel_id,
            "user_identity".to_string(),
            extranonce_prefix,
            target.into(),
            nominal_hashrate,
            version_rolling,
            m.extranonce_size,
        );
        self.extended_channels
            .insert(m.channel_id, Arc::new(RwLock::new(extended_channel)));
        let m = Mining::OpenExtendedMiningChannelSuccess(m.into_static());
        Ok(SendTo::None(Some(m)))
    }

    fn handle_open_mining_channel_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::OpenMiningChannelError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_update_channel_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::UpdateChannelError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_close_channel(
        &mut self,
        m: roles_logic_sv2::mining_sv2::CloseChannel,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
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

    fn handle_submit_shares_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SubmitSharesError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_new_mining_job(
        &mut self,
        m: roles_logic_sv2::mining_sv2::NewMiningJob,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        unreachable!()
    }

    fn handle_new_extended_mining_job(
        &mut self,
        m: NewExtendedMiningJob,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        // let mut channel = self.extended_channels.get(&m.channel_id).unwrap().write().unwrap();
        // channel.on_new_extended_mining_job(m);
        // Ok(SendTo::None(Some(Mining::NewExtendedMiningJob(m))))
        todo!()
    }

    fn handle_set_new_prev_hash(
        &mut self,
        m: SetNewPrevHash,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        // let mut channel = self.extended_channels.get(&m.channel_id).unwrap().write().unwrap();
        // channel.on_set_new_prev_hash(m);
        // Ok(SendTo::None(None))
        todo!()
    }

    fn handle_set_custom_mining_job_success(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SetCustomMiningJobSuccess,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        unreachable!()
    }

    fn handle_set_custom_mining_job_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SetCustomMiningJobError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        unreachable!()
    }

    fn handle_set_target(&mut self, m: SetTarget) -> Result<SendTo<Downstream>, RolesLogicError> {
        todo!()
    }

    fn handle_set_group_channel(
        &mut self,
        _m: roles_logic_sv2::mining_sv2::SetGroupChannel,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        unreachable!()
    }
}
