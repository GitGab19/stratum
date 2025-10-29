# `extensions_sv2`

This crate provides message types for Stratum V2 protocol extensions.

## Supported Extensions

### Extensions Negotiation (0x0001)

The Extensions Negotiation extension allows endpoints to negotiate which optional extensions are supported during connection setup. This enables backward compatibility and graceful feature detection.

**Messages:**
- `RequestExtensions` - Client requests support for specific extensions
- `RequestExtensionsSuccess` - Server accepts and confirms supported extensions
- `RequestExtensionsError` - Server rejects request or indicates missing required extensions

**Specification:** [extensions-negotiation.md](https://github.com/stratum-mining/sv2-spec/blob/main/extensions/extensions-negotiation.md)


## Example

```rust
use extensions_sv2::{RequestExtensions, EXTENSION_TYPE_EXTENSIONS_NEGOTIATION};
use binary_sv2::Seq064K;

// Client requests extensions negotiation support
let request = RequestExtensions {
    request_id: 1,
    requested_extensions: Seq064K::new(vec![EXTENSION_TYPE_EXTENSIONS_NEGOTIATION]).unwrap(),
};
```