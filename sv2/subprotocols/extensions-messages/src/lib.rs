//! # Stratum V2 Extensions Messages Crate.
//!
//! This crate defines extension messages for Stratum V2 protocol.
//!
//! ## Extensions Supported
//!
//! - **Extensions Negotiation** (extension_type=0x0001): Allows endpoints to negotiate
//!   which optional extensions are supported during connection setup.
//!
//! ## Build Options
//! This crate can be built with the following features:
//! - `std`: Enables support for standard library features.
//! - `prop_test`: Enables support for property-based testing.
//!
//! For further information about the extension, please refer to:
//! - [Extensions Negotiation Spec](https://github.com/stratum-mining/sv2-spec/blob/main/extensions/extensions-negotiation.md)

#![no_std]

extern crate alloc;

mod extensions_negotiation;

pub use extensions_negotiation::{
    RequestExtensions, RequestExtensionsError, RequestExtensionsSuccess,
};

// Extension type discriminant
pub const EXTENSION_TYPE_EXTENSIONS_NEGOTIATION: u16 = 0x0001;

// Message type constants for Extensions Negotiation (extension_type=0x0001)
pub const MESSAGE_TYPE_REQUEST_EXTENSIONS: u8 = 0x00;
pub const MESSAGE_TYPE_REQUEST_EXTENSIONS_SUCCESS: u8 = 0x01;
pub const MESSAGE_TYPE_REQUEST_EXTENSIONS_ERROR: u8 = 0x02;

// Channel message bits (all false for extensions as per spec)
pub const CHANNEL_BIT_REQUEST_EXTENSIONS: bool = false;
pub const CHANNEL_BIT_REQUEST_EXTENSIONS_SUCCESS: bool = false;
pub const CHANNEL_BIT_REQUEST_EXTENSIONS_ERROR: bool = false;
