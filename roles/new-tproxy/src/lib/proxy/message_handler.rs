use crate::{downstream_sv1::downstream::Downstream, proxy::ChannelManager};
use roles_logic_sv2::{
    handlers::mining::{ParseMiningMessagesFromUpstream, SendTo, SupportedChannelTypes},
    mining_sv2::{
        NewExtendedMiningJob, OpenExtendedMiningChannelSuccess, SetNewPrevHash, SetTarget,
    },
    Error as RolesLogicError,
};

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
        // let nominal_hashrate =
        // self.proxy_config.downstream_difficulty_config.min_individual_miner_hashrate; let
        // downstream = Downstream::new(m.request_id, "user_identity".to_string(), nominal_hashrate,
        // self.upstream_sender.clone(), self.downstream_sv1_sender.clone(),
        // m.extranonce_prefix.into_static().to_vec(), m.extranonce_size.into());
        // self.downstreams.insert(m.request_id, Arc::new(Mutex::new(downstream)));

        // let extranonce_prefix = m.extranonce_prefix.into_static().to_vec();
        // let target = m.target.into_static();
        // let version_rolling = true; // we assume this is always true on extended channels
        // let extended_channel = ExtendedChannel::new(m.channel_id, "user_identity".to_string(),
        // extranonce_prefix, target.into(), nominal_hashrate, version_rolling, m.extranonce_size);
        // self.extended_channels.insert(m.channel_id, Arc::new(RwLock::new(extended_channel)));
        // Ok(SendTo::None(Some(Mining::OpenExtendedMiningChannelSuccess(m))))
        todo!()
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
