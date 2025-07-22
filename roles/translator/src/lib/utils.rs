use binary_sv2::Sv2DataType;
use buffer_sv2::Slice;
use codec_sv2::Frame;
use roles_logic_sv2::{
    bitcoin::{
        block::{Header, Version},
        hashes::Hash,
        CompactTarget, TxMerkleNode,
    },
    mining_sv2::Target,
    parsers_sv2::{AnyMessage, CommonMessages},
    utils::{bytes_to_hex, merkle_root_from_path, u256_to_block_hash},
};
use tracing::{debug, error};
use v1::{client_to_server, server_to_client, utils::HexU32Be};

use crate::error::TproxyError;

/// Validates an SV1 share against the target difficulty and job parameters.
///
/// This function performs complete share validation by:
/// 1. Finding the corresponding job from the valid jobs list
/// 2. Constructing the full extranonce from extranonce1 and extranonce2
/// 3. Calculating the merkle root from the coinbase transaction and merkle path
/// 4. Building the block header with the share's nonce and timestamp
/// 5. Hashing the header and comparing against the target difficulty
///
/// # Arguments
/// * `share` - The SV1 submit message containing the share data
/// * `target` - The target difficulty for this share
/// * `extranonce1` - The first part of the extranonce (from server)
/// * `version_rolling_mask` - Optional mask for version rolling
/// * `valid_jobs` - List of valid jobs to validate against
///
/// # Returns
/// * `Ok(true)` if the share is valid and meets the target
/// * `Ok(false)` if the share is valid but doesn't meet the target
/// * `Err(TproxyError)` if validation fails due to missing job or invalid data
pub fn validate_sv1_share(
    share: &client_to_server::Submit<'static>,
    target: Target,
    extranonce1: Vec<u8>,
    version_rolling_mask: Option<HexU32Be>,
    valid_jobs: &[server_to_client::Notify<'static>],
) -> Result<bool, TproxyError> {
    let job_id = share.job_id.clone();

    let job = valid_jobs
        .iter()
        .find(|job| job.job_id == job_id)
        .ok_or(TproxyError::JobNotFound)?;

    let mut full_extranonce = vec![];
    full_extranonce.extend_from_slice(extranonce1.as_slice());
    full_extranonce.extend_from_slice(share.extra_nonce2.0.as_ref());

    let share_version = share
        .version_bits
        .clone()
        .map(|vb| vb.0)
        .unwrap_or(job.version.0);
    let mask = version_rolling_mask.unwrap_or(HexU32Be(0x1FFFE000_u32)).0;
    let version = (job.version.0 & !mask) | (share_version & mask);

    let prev_hash_vec: Vec<u8> = job.prev_hash.clone().into();
    let prev_hash = binary_sv2::U256::from_vec_(prev_hash_vec).map_err(TproxyError::BinarySv2)?;

    // calculate the merkle root from:
    // - job coinbase_tx_prefix
    // - full extranonce
    // - job coinbase_tx_suffix
    // - job merkle_path
    let merkle_root: [u8; 32] = merkle_root_from_path(
        job.coin_base1.as_ref(),
        job.coin_base2.as_ref(),
        full_extranonce.as_ref(),
        job.merkle_branch.as_ref(),
    )
    .ok_or(TproxyError::InvalidMerkleRoot)?
    .try_into()
    .map_err(|_| TproxyError::InvalidMerkleRoot)?;

    // create the header for validation
    let header = Header {
        version: Version::from_consensus(version as i32),
        prev_blockhash: u256_to_block_hash(prev_hash),
        merkle_root: TxMerkleNode::from_byte_array(merkle_root),
        time: share.time.0,
        bits: CompactTarget::from_consensus(job.bits.0),
        nonce: share.nonce.0,
    };

    // convert the header hash to a target type for easy comparison
    let hash = header.block_hash();
    let raw_hash: [u8; 32] = *hash.to_raw_hash().as_ref();
    let hash_as_target: Target = raw_hash.into();

    // print hash_as_target and self.target as human readable hex
    let hash_as_u256: binary_sv2::U256 = hash_as_target.clone().into();
    let mut hash_bytes = hash_as_u256.to_vec();
    hash_bytes.reverse(); // Convert to big-endian for display
    let target_u256: binary_sv2::U256 = target.clone().into();
    let mut target_bytes = target_u256.to_vec();
    target_bytes.reverse(); // Convert to big-endian for display

    debug!(
        "share validation \nshare:\t\t{}\ndownstream target:\t{}\n",
        bytes_to_hex(&hash_bytes),
        bytes_to_hex(&target_bytes),
    );
    // check if the share hash meets the downstream target
    if hash_as_target < target {
        /*if self.share_accounting.is_share_seen(hash.to_raw_hash()) {
            return Err(ShareValidationError::DuplicateShare);
        }*/

        return Ok(true);
    }

    Ok(false)
}

/// Calculates the required length of the proxy's extranonce prefix.
///
/// This function determines how many bytes the proxy needs to reserve for its own
/// extranonce prefix, based on the difference between the channel's rollable extranonce
/// size and the downstream miner's rollable extranonce size.
///
/// # Arguments
/// * `channel_rollable_extranonce_size` - Size of the rollable extranonce from the channel
/// * `downstream_rollable_extranonce_size` - Size of the rollable extranonce for downstream
///
/// # Returns
/// The number of bytes needed for the proxy's extranonce prefix
pub fn proxy_extranonce_prefix_len(
    channel_rollable_extranonce_size: usize,
    downstream_rollable_extranonce_size: usize,
) -> usize {
    channel_rollable_extranonce_size - downstream_rollable_extranonce_size
}

/// Extracts message type, payload, and parsed message from an SV2 frame.
///
/// This function processes an SV2 frame and extracts the essential components:
/// - Message type identifier
/// - Raw payload bytes
/// - Parsed message structure
///
/// # Arguments
/// * `frame` - The SV2 frame to process
///
/// # Returns
/// A tuple containing (message_type, payload, parsed_message) on success,
/// or a TproxyError if the frame is invalid or cannot be parsed
pub fn message_from_frame(
    frame: &mut Frame<AnyMessage<'static>, Slice>,
) -> Result<(u8, Vec<u8>, AnyMessage<'static>), TproxyError> {
    match frame {
        Frame::Sv2(frame) => {
            let header = frame.get_header().ok_or(TproxyError::UnexpectedMessage)?;
            let message_type = header.msg_type();
            let mut payload = frame.payload().to_vec();
            let message: Result<AnyMessage<'_>, _> =
                (message_type, payload.as_mut_slice()).try_into();
            match message {
                Ok(message) => {
                    let message = into_static(message)?;
                    Ok((message_type, payload.to_vec(), message))
                }
                Err(_) => {
                    error!("Received frame with invalid payload or message type: {frame:?}");
                    Err(TproxyError::UnexpectedMessage)
                }
            }
        }
        Frame::HandShake(f) => {
            error!("Received unexpected handshake frame: {f:?}");
            Err(TproxyError::UnexpectedMessage)
        }
    }
}

/// Converts a borrowed AnyMessage to a static lifetime version.
///
/// This function takes an AnyMessage with a borrowed lifetime and converts it to
/// a static lifetime version, which is necessary for storing messages across
/// async boundaries and in data structures.
///
/// # Arguments
/// * `m` - The AnyMessage to convert to static lifetime
///
/// # Returns
/// A static lifetime version of the message, or TproxyError if the message
/// type is not supported for static conversion
pub fn into_static(m: AnyMessage<'_>) -> Result<AnyMessage<'static>, TproxyError> {
    match m {
        AnyMessage::Mining(m) => Ok(AnyMessage::Mining(m.into_static())),
        AnyMessage::Common(m) => match m {
            CommonMessages::ChannelEndpointChanged(m) => Ok(AnyMessage::Common(
                CommonMessages::ChannelEndpointChanged(m.into_static()),
            )),
            CommonMessages::SetupConnection(m) => Ok(AnyMessage::Common(
                CommonMessages::SetupConnection(m.into_static()),
            )),
            CommonMessages::SetupConnectionError(m) => Ok(AnyMessage::Common(
                CommonMessages::SetupConnectionError(m.into_static()),
            )),
            CommonMessages::SetupConnectionSuccess(m) => Ok(AnyMessage::Common(
                CommonMessages::SetupConnectionSuccess(m.into_static()),
            )),
            CommonMessages::Reconnect(m) => Ok(AnyMessage::Common(CommonMessages::Reconnect(
                m.into_static(),
            ))),
        },
        _ => Err(TproxyError::UnexpectedMessage),
    }
}

/// Messages used for coordinating shutdown across different components.
///
/// This enum defines the different types of shutdown signals that can be sent
/// through the broadcast channel to coordinate graceful shutdown of the translator.
#[derive(Debug, Clone)]
pub enum ShutdownMessage {
    /// Shutdown all components immediately
    ShutdownAll,
    /// Shutdown all downstream connections
    DownstreamShutdownAll,
    /// Shutdown a specific downstream connection by ID
    DownstreamShutdown(u32),
}
