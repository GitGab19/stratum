use crate::{errors::Error, parsers_sv2::CommonMessages};
use common_messages_sv2::{
    ChannelEndpointChanged, Reconnect, SetupConnectionError, SetupConnectionSuccess, *,
};
use core::convert::TryInto;

pub trait ParseCommonMessagesFromUpstream {
    fn handle_common_message(&mut self, message_type: u8, payload: &mut [u8]) -> Result<(), Error> {
        let parsed: CommonMessages<'_> = (message_type, payload).try_into()?;
        self.dispatch_common_message(parsed)
    }

    fn dispatch_common_message(&mut self, message: CommonMessages<'_>) -> Result<(), Error> {
        match message {
            CommonMessages::SetupConnectionSuccess(msg) => {
                self.handle_setup_connection_success(msg)
            }
            CommonMessages::SetupConnectionError(msg) => self.handle_setup_connection_error(msg),
            CommonMessages::ChannelEndpointChanged(msg) => {
                self.handle_channel_endpoint_changed(msg)
            }
            CommonMessages::Reconnect(msg) => self.handle_reconnect(msg),

            CommonMessages::SetupConnection(_) => {
                Err(Error::UnexpectedMessage(MESSAGE_TYPE_SETUP_CONNECTION))
            }
        }
    }

    fn handle_setup_connection_success(&mut self, msg: SetupConnectionSuccess)
        -> Result<(), Error>;

    fn handle_setup_connection_error(&mut self, msg: SetupConnectionError) -> Result<(), Error>;

    fn handle_channel_endpoint_changed(&mut self, msg: ChannelEndpointChanged)
        -> Result<(), Error>;

    fn handle_reconnect(&mut self, msg: Reconnect) -> Result<(), Error>;
}

pub trait ParseCommonMessagesFromDownstream
where
    Self: Sized,
{
    fn handle_common_message(&mut self, message_type: u8, payload: &mut [u8]) -> Result<(), Error> {
        let parsed: CommonMessages<'_> = (message_type, payload).try_into()?;
        self.dispatch_common_message(parsed)
    }

    fn dispatch_common_message(&mut self, message: CommonMessages<'_>) -> Result<(), Error> {
        match message {
            CommonMessages::SetupConnectionSuccess(msg) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
            )),
            CommonMessages::SetupConnectionError(msg) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
            )),
            CommonMessages::ChannelEndpointChanged(msg) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED,
            )),
            CommonMessages::Reconnect(msg) => Err(Error::UnexpectedMessage(MESSAGE_TYPE_RECONNECT)),

            CommonMessages::SetupConnection(msg) => self.handle_setup_connection(msg),
        }
    }

    fn handle_setup_connection(&mut self, msg: SetupConnection) -> Result<(), Error>;
}
