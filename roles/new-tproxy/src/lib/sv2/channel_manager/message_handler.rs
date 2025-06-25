use std::sync::{Arc, RwLock};

use crate::{sv1::downstream::Downstream, sv2::ChannelManager};
use roles_logic_sv2::{
    channels::client::extended::ExtendedChannel,
    common_messages_sv2::{Protocol, SetupConnectionSuccess},
    common_properties::{IsMiningUpstream, IsUpstream},
    handlers::mining::{ParseMiningMessagesFromUpstream, SendTo, SupportedChannelTypes},
    mining_sv2::{
        NewExtendedMiningJob, OpenExtendedMiningChannelSuccess, SetNewPrevHash, SetTarget,
    },
    parsers::Mining,
    Error as RolesLogicError,
};

use roles_logic_sv2::{
    common_messages_sv2::{ChannelEndpointChanged, Reconnect, SetupConnectionError},
    handlers::common::{ParseCommonMessagesFromUpstream, SendTo as SendToCommon},
    Error,
};

use tracing::{debug, error, info};
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
        // Get the stored user identity and hashrate using request_id as downstream_id
        let (user_identity, nominal_hashrate) = self
            .pending_channels
            .remove(&m.request_id)
            .unwrap_or_else(|| ("unknown".to_string(), 100000.0));
        
        info!(
            "Received OpenExtendedMiningChannelSuccess with request id: {} and channel id: {}, user: {}, hashrate: {}",
            m.request_id, m.channel_id, user_identity, nominal_hashrate
        );
        debug!("OpenStandardMiningChannelSuccess: {:?}", m);
        info!("Up: Successfully Opened Extended Mining Channel");
        let extranonce_prefix = m.extranonce_prefix.clone().into_static().to_vec();
        let target = m.target.clone().into_static();
        let version_rolling = true; // we assume this is always true on extended channels
        let extended_channel = ExtendedChannel::new(
            m.channel_id,
            user_identity,
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
        error!(
            "Received OpenExtendedMiningChannelError with error code {}",
            std::str::from_utf8(m.error_code.as_ref()).unwrap_or("unknown error code")
        );
        Ok(SendTo::None(Some(Mining::OpenMiningChannelError(
            m.as_static(),
        ))))
    }

    fn handle_update_channel_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::UpdateChannelError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        error!(
            "Received UpdateChannelError with error code {}",
            std::str::from_utf8(m.error_code.as_ref()).unwrap_or("unknown error code")
        );
        Ok(SendTo::None(Some(Mining::UpdateChannelError(
            m.as_static(),
        ))))
    }

    fn handle_close_channel(
        &mut self,
        m: roles_logic_sv2::mining_sv2::CloseChannel,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        info!("Received CloseChannel for channel id: {}", m.channel_id);
        Ok(SendTo::None(Some(Mining::CloseChannel(m.as_static()))))
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
        info!("Received SubmitSharesSuccess");
        debug!("SubmitSharesSuccess: {:?}", m);
        Ok(SendTo::None(Some(Mining::SubmitSharesSuccess(
            m.into_static(),
        ))))
    }

    fn handle_submit_shares_error(
        &mut self,
        m: roles_logic_sv2::mining_sv2::SubmitSharesError,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        Ok(SendTo::None(Some(Mining::SubmitSharesError(
            m.into_static(),
        ))))
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
        let m_static = m.clone().into_static();
        if let Some(channel) = self.extended_channels.get(&m_static.channel_id) {
            let mut channel = channel.write().unwrap();
            channel.on_new_extended_mining_job(m_static.clone());
            return Ok(SendTo::None(Some(Mining::NewExtendedMiningJob(m_static))));
        }
        Ok(SendTo::None(None))
    }

    fn handle_set_new_prev_hash(
        &mut self,
        m: SetNewPrevHash,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        let m_static = m.clone().into_static();
        if let Some(channel) = self.extended_channels.get(&m_static.channel_id) {
            let mut channel = channel.write().unwrap();
            channel.on_set_new_prev_hash(m_static.clone());
            return Ok(SendTo::None(Some(Mining::SetNewPrevHash(m_static))));
        }
        Ok(SendTo::None(None))
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
        let mut extended_channel = self
            .extended_channels
            .get(&m.channel_id)
            .unwrap()
            .write()
            .unwrap();
        extended_channel.set_target(m.maximum_target.clone().into());
        Ok(SendTo::None(Some(Mining::SetTarget(m.into_static()))))
    }

    fn handle_set_group_channel(
        &mut self,
        _m: roles_logic_sv2::mining_sv2::SetGroupChannel,
    ) -> Result<SendTo<Downstream>, RolesLogicError> {
        unreachable!()
    }
}

impl ParseCommonMessagesFromUpstream for ChannelManager {
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

    fn handle_setup_connection_error(
        &mut self,
        _m: SetupConnectionError,
    ) -> Result<SendToCommon, Error> {
        todo!()
    }

    fn handle_channel_endpoint_changed(
        &mut self,
        _m: ChannelEndpointChanged,
    ) -> Result<SendToCommon, Error> {
        todo!()
    }

    fn handle_reconnect(&mut self, _m: Reconnect) -> Result<SendToCommon, Error> {
        todo!()
    }
}
