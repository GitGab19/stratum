//! Non-Custodial Pool Payouts (extension_type=0x0003)
//!
//! This extension lets a Job Declaration Client request the coinbase payout
//! outputs associated with a mining job token.

use alloc::{fmt, vec::Vec};
use binary_sv2::{Deserialize, Serialize, Str0255, B0255, B064K};
use core::convert::TryInto;

/// Extension type for Non-Custodial Pool Payouts.
pub const EXTENSION_TYPE: u16 = 0x0003;

/// Message type constants.
pub const MESSAGE_TYPE_REQUEST_PAYOUT_OUTPUTS: u8 = 0x00;
pub const MESSAGE_TYPE_REQUEST_PAYOUT_OUTPUTS_SUCCESS: u8 = 0x01;
pub const MESSAGE_TYPE_REQUEST_PAYOUT_OUTPUTS_ERROR: u8 = 0x02;

/// Channel message bits (all false for extension-defined messages).
pub const CHANNEL_BIT_REQUEST_PAYOUT_OUTPUTS: bool = false;
pub const CHANNEL_BIT_REQUEST_PAYOUT_OUTPUTS_SUCCESS: bool = false;
pub const CHANNEL_BIT_REQUEST_PAYOUT_OUTPUTS_ERROR: bool = false;

/// Error code used when a payout output set is stale or superseded.
pub const ERROR_CODE_STALE_PAYOUT_OUTPUTS: &str = "stale-payout-outputs";

/// Requests the payout outputs associated with a mining job token.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestPayoutOutputs<'decoder> {
    /// Unique identifier for pairing request/response.
    pub request_id: u32,
    /// Token previously issued by `AllocateMiningJobToken.Success`.
    pub mining_job_token: B0255<'decoder>,
    /// Amount, in satoshis, that must be distributed by the returned output set.
    pub available_payout_value: u64,
}

impl fmt::Display for RequestPayoutOutputs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RequestPayoutOutputs(request_id: {}, mining_job_token: {}, available_payout_value: {})",
            self.request_id,
            self.mining_job_token.as_hex(),
            self.available_payout_value
        )
    }
}

/// Returned when the JDS successfully computes a payout output set.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestPayoutOutputsSuccess<'decoder> {
    /// Unique identifier matching the request.
    pub request_id: u32,
    /// Bitcoin transaction output vector to include in the coinbase transaction.
    pub coinbase_tx_outputs: B064K<'decoder>,
}

impl fmt::Display for RequestPayoutOutputsSuccess<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RequestPayoutOutputs.Success(request_id: {}, coinbase_tx_outputs: {})",
            self.request_id, self.coinbase_tx_outputs
        )
    }
}

/// Returned when the JDS cannot provide a valid payout output set.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestPayoutOutputsError<'decoder> {
    /// Unique identifier matching the request.
    pub request_id: u32,
    /// Error identifier.
    pub error_code: Str0255<'decoder>,
}

impl fmt::Display for RequestPayoutOutputsError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RequestPayoutOutputs.Error(request_id: {}, error_code: {})",
            self.request_id,
            self.error_code.as_utf8_or_hex()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::String, vec};

    #[test]
    fn test_request_payout_outputs() {
        let msg = RequestPayoutOutputs {
            request_id: 1,
            mining_job_token: B0255::try_from(vec![2, 3]).unwrap(),
            available_payout_value: 50_000,
        };

        assert_eq!(msg.request_id, 1);
        assert_eq!(msg.mining_job_token.to_vec(), vec![2, 3]);
        assert_eq!(msg.available_payout_value, 50_000);
    }

    #[test]
    fn test_request_payout_outputs_success() {
        let msg = RequestPayoutOutputsSuccess {
            request_id: 2,
            coinbase_tx_outputs: B064K::try_from(vec![1, 2, 3]).unwrap(),
        };

        assert_eq!(msg.request_id, 2);
        assert_eq!(msg.coinbase_tx_outputs.to_vec(), vec![1, 2, 3]);
    }

    #[test]
    fn test_request_payout_outputs_error() {
        let msg = RequestPayoutOutputsError {
            request_id: 3,
            error_code: Str0255::try_from(String::from(ERROR_CODE_STALE_PAYOUT_OUTPUTS)).unwrap(),
        };

        assert_eq!(msg.request_id, 3);
        assert_eq!(
            msg.error_code.as_utf8_or_hex(),
            ERROR_CODE_STALE_PAYOUT_OUTPUTS
        );
    }
}
