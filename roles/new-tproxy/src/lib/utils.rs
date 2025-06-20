use buffer_sv2::Slice;
use codec_sv2::Frame;
use roles_logic_sv2::parsers::{AnyMessage, CommonMessages};

/// Calculates the required length of the proxy's extranonce1.
///
/// The proxy needs to calculate an extranonce1 value to send to the
/// upstream server.  This function determines the length of that
/// extranonce1 value
/// FIXME: The pool only supported 16 bytes exactly for its
/// `extranonce1` field is no longer the case and the
/// code needs to be changed to support variable `extranonce1` lengths.
pub fn proxy_extranonce1_len(
    channel_extranonce2_size: usize,
    downstream_extranonce2_len: usize,
) -> usize {
    // full_extranonce_len - pool_extranonce1_len - miner_extranonce2 = tproxy_extranonce1_len
    channel_extranonce2_size - downstream_extranonce2_len
}

pub fn message_from_frame(frame: &mut Frame<AnyMessage<'static>, Slice>) -> AnyMessage<'static> {
    match frame {
        Frame::Sv2(frame) => {
            if let Some(header) = frame.get_header() {
                let message_type = header.msg_type();
                let mut payload = frame.payload().to_vec();
                let message: Result<AnyMessage<'_>, _> =
                    (message_type, payload.as_mut_slice()).try_into();
                match message {
                    Ok(message) => {
                        let message = into_static(message);
                        message
                    }
                    _ => {
                        println!("Received frame with invalid payload or message type: {frame:?}");
                        panic!();
                    }
                }
            } else {
                println!("Received frame with invalid header: {frame:?}");
                panic!();
            }
        }
        Frame::HandShake(f) => {
            println!("Received unexpected handshake frame: {f:?}");
            panic!();
        }
    }
}

pub fn into_static(m: AnyMessage<'_>) -> AnyMessage<'static> {
    match m {
        AnyMessage::Mining(m) => AnyMessage::Mining(m.into_static()),
        AnyMessage::Common(m) => match m {
            CommonMessages::ChannelEndpointChanged(m) => {
                AnyMessage::Common(CommonMessages::ChannelEndpointChanged(m.into_static()))
            }
            CommonMessages::SetupConnection(m) => {
                AnyMessage::Common(CommonMessages::SetupConnection(m.into_static()))
            }
            CommonMessages::SetupConnectionError(m) => {
                AnyMessage::Common(CommonMessages::SetupConnectionError(m.into_static()))
            }
            CommonMessages::SetupConnectionSuccess(m) => {
                AnyMessage::Common(CommonMessages::SetupConnectionSuccess(m.into_static()))
            }
            CommonMessages::Reconnect(m) => {
                AnyMessage::Common(CommonMessages::Reconnect(m.into_static()))
            }
        },
        _ => todo!(),
    }
}
