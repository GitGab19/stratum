use crate::{errors::Error, parsers_sv2::Mining};
use codec_sv2::binary_sv2;
use mining_sv2::{
    CloseChannel, NewExtendedMiningJob, NewMiningJob, OpenExtendedMiningChannel,
    OpenExtendedMiningChannelSuccess, OpenMiningChannelError, OpenStandardMiningChannel,
    OpenStandardMiningChannelSuccess, SetCustomMiningJob, SetCustomMiningJobError,
    SetCustomMiningJobSuccess, SetExtranoncePrefix, SetGroupChannel, SetNewPrevHash, SetTarget,
    SubmitSharesError, SubmitSharesExtended, SubmitSharesStandard, SubmitSharesSuccess,
    UpdateChannel, UpdateChannelError,
};

use mining_sv2::*;
use std::fmt::Debug as D;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SupportedChannelTypes {
    Standard,
    Extended,
    Group,
    GroupAndExtended,
}

pub trait ParseMiningMessagesFromDownstream<Up: D>
where
    Self: Sized + D,
{
    fn get_channel_type(&self) -> SupportedChannelTypes;
    fn is_work_selection_enabled(&self) -> bool;

    fn is_downstream_authorized(&self, user_identity: &binary_sv2::Str0255) -> Result<bool, Error>;

    fn handle_mining_message(&mut self, message: Mining) -> Result<(), Error> {
        let (channel_type, work_selection) =
            (self.get_channel_type(), self.is_work_selection_enabled());

        use Mining::*;
        match message {
            OpenStandardMiningChannel(m) => {
                if !self.is_downstream_authorized(&m.user_identity)? {
                    // Add correct error type
                    return Err(Error::DownstreamDown);
                }

                match channel_type {
                    SupportedChannelTypes::Standard
                    | SupportedChannelTypes::Group
                    | SupportedChannelTypes::GroupAndExtended => {
                        self.handle_open_standard_mining_channel(m)
                    }
                    SupportedChannelTypes::Extended => Err(Error::UnexpectedMessage(
                        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
                    )),
                }
            }
            OpenExtendedMiningChannel(m) => {
                if !self.is_downstream_authorized(&m.user_identity)? {
                    // Add correct Error type
                    return Err(Error::DownstreamDown);
                }

                match channel_type {
                    SupportedChannelTypes::Extended | SupportedChannelTypes::GroupAndExtended => {
                        self.handle_open_extended_mining_channel(m)
                    }
                    _ => Err(Error::UnexpectedMessage(
                        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
                    )),
                }
            }
            UpdateChannel(m) => self.handle_update_channel(m),

            SubmitSharesStandard(m) => match channel_type {
                SupportedChannelTypes::Standard
                | SupportedChannelTypes::Group
                | SupportedChannelTypes::GroupAndExtended => self.handle_submit_shares_standard(m),
                SupportedChannelTypes::Extended => Err(Error::UnexpectedMessage(
                    MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
                )),
            },

            SubmitSharesExtended(m) => match channel_type {
                SupportedChannelTypes::Extended | SupportedChannelTypes::GroupAndExtended => {
                    self.handle_submit_shares_extended(m)
                }
                _ => Err(Error::UnexpectedMessage(
                    MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
                )),
            },

            SetCustomMiningJob(m) => match (channel_type, work_selection) {
                (SupportedChannelTypes::Extended, true)
                | (SupportedChannelTypes::GroupAndExtended, true) => {
                    self.handle_set_custom_mining_job(m)
                }
                _ => Err(Error::UnexpectedMessage(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB)),
            },

            _ => Err(Error::UnexpectedMessage(0)),
        }
    }

    fn handle_open_standard_mining_channel(
        &mut self,
        msg: OpenStandardMiningChannel,
    ) -> Result<(), Error>;

    fn handle_open_extended_mining_channel(
        &mut self,
        msg: OpenExtendedMiningChannel,
    ) -> Result<(), Error>;

    fn handle_update_channel(&mut self, msg: UpdateChannel) -> Result<(), Error>;

    fn handle_submit_shares_standard(&mut self, msg: SubmitSharesStandard) -> Result<(), Error>;

    fn handle_submit_shares_extended(&mut self, msg: SubmitSharesExtended) -> Result<(), Error>;

    fn handle_set_custom_mining_job(&mut self, msg: SetCustomMiningJob) -> Result<(), Error>;
}

pub trait ParseMiningMessagesFromUpstream<Down: D>
where
    Self: Sized + D,
{
    fn get_channel_type(&self) -> SupportedChannelTypes;
    fn is_work_selection_enabled(&self) -> bool;

    fn handle_mining_message(&mut self, message: Mining) -> Result<(), Error> {
        let (channel_type, work_selection) =
            (self.get_channel_type(), self.is_work_selection_enabled());

        use Mining::*;
        match message {
            OpenStandardMiningChannelSuccess(m) => match channel_type {
                SupportedChannelTypes::Standard
                | SupportedChannelTypes::Group
                | SupportedChannelTypes::GroupAndExtended => {
                    self.handle_open_standard_mining_channel_success(m)
                }
                _ => Err(Error::UnexpectedMessage(
                    MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
                )),
            },

            OpenExtendedMiningChannelSuccess(m) => match channel_type {
                SupportedChannelTypes::Extended | SupportedChannelTypes::GroupAndExtended => {
                    self.handle_open_extended_mining_channel_success(m)
                }
                _ => Err(Error::UnexpectedMessage(
                    MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
                )),
            },

            OpenMiningChannelError(m) => self.handle_open_mining_channel_error(m),
            UpdateChannelError(m) => self.handle_update_channel_error(m),
            CloseChannel(m) => self.handle_close_channel(m),
            SetExtranoncePrefix(m) => self.handle_set_extranonce_prefix(m),
            SubmitSharesSuccess(m) => self.handle_submit_shares_success(m),
            SubmitSharesError(m) => self.handle_submit_shares_error(m),

            NewMiningJob(m) => match channel_type {
                SupportedChannelTypes::Standard => self.handle_new_mining_job(m),
                _ => Err(Error::UnexpectedMessage(MESSAGE_TYPE_NEW_MINING_JOB)),
            },

            NewExtendedMiningJob(m) => match channel_type {
                SupportedChannelTypes::Extended
                | SupportedChannelTypes::Group
                | SupportedChannelTypes::GroupAndExtended => self.handle_new_extended_mining_job(m),
                _ => Err(Error::UnexpectedMessage(
                    MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB,
                )),
            },

            SetNewPrevHash(m) => self.handle_set_new_prev_hash(m),

            SetCustomMiningJobSuccess(m) => match (channel_type, work_selection) {
                (SupportedChannelTypes::Extended, true)
                | (SupportedChannelTypes::GroupAndExtended, true) => {
                    self.handle_set_custom_mining_job_success(m)
                }
                _ => Err(Error::UnexpectedMessage(
                    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
                )),
            },

            SetCustomMiningJobError(m) => match (channel_type, work_selection) {
                (SupportedChannelTypes::Extended, true)
                | (SupportedChannelTypes::Group, true)
                | (SupportedChannelTypes::GroupAndExtended, true) => {
                    self.handle_set_custom_mining_job_error(m)
                }
                _ => Err(Error::UnexpectedMessage(
                    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
                )),
            },

            SetTarget(m) => self.handle_set_target(m),

            SetGroupChannel(m) => match channel_type {
                SupportedChannelTypes::Group | SupportedChannelTypes::GroupAndExtended => {
                    self.handle_set_group_channel(m)
                }
                _ => Err(Error::UnexpectedMessage(MESSAGE_TYPE_SET_GROUP_CHANNEL)),
            },

            _ => Err(Error::UnexpectedMessage(0)),
        }
    }

    fn handle_open_standard_mining_channel_success(
        &mut self,
        msg: OpenStandardMiningChannelSuccess,
    ) -> Result<(), Error>;

    fn handle_open_extended_mining_channel_success(
        &mut self,
        msg: OpenExtendedMiningChannelSuccess,
    ) -> Result<(), Error>;

    fn handle_open_mining_channel_error(
        &mut self,
        msg: OpenMiningChannelError,
    ) -> Result<(), Error>;

    fn handle_update_channel_error(&mut self, msg: UpdateChannelError) -> Result<(), Error>;

    fn handle_close_channel(&mut self, msg: CloseChannel) -> Result<(), Error>;

    fn handle_set_extranonce_prefix(&mut self, msg: SetExtranoncePrefix) -> Result<(), Error>;

    fn handle_submit_shares_success(&mut self, msg: SubmitSharesSuccess) -> Result<(), Error>;

    fn handle_submit_shares_error(&mut self, msg: SubmitSharesError) -> Result<(), Error>;

    fn handle_new_mining_job(&mut self, msg: NewMiningJob) -> Result<(), Error>;

    fn handle_new_extended_mining_job(&mut self, msg: NewExtendedMiningJob) -> Result<(), Error>;

    fn handle_set_new_prev_hash(&mut self, msg: SetNewPrevHash) -> Result<(), Error>;

    fn handle_set_custom_mining_job_success(
        &mut self,
        msg: SetCustomMiningJobSuccess,
    ) -> Result<(), Error>;

    fn handle_set_custom_mining_job_error(
        &mut self,
        msg: SetCustomMiningJobError,
    ) -> Result<(), Error>;

    fn handle_set_target(&mut self, msg: SetTarget) -> Result<(), Error>;

    fn handle_set_group_channel(&mut self, msg: SetGroupChannel) -> Result<(), Error>;
}
