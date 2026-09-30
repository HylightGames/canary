//! Length-prefixed framing with the limit gate before allocation.
//!
//! Wire layout: a 4-byte big-endian length prefix followed by exactly that
//! many body bytes (one `postcard`-encoded [`crate::envelope::NetEnvelope`]
//! in this profile). The receiver reads the prefix, checks it against
//! [`NetLimits`](crate::limits::NetLimits), and only then allocates —
//! `read_to_end`-style unbounded reads on untrusted bytes are never used on
//! this path.

use crate::error::NetError;
use crate::limits::NetLimits;

/// Length of the big-endian `u32` length prefix in bytes.
pub const FRAME_HEADER_LEN: usize = 4;

/// Prefixes `body` with its big-endian length for the wire.
///
/// Rejects bodies over the limit (and bodies that cannot fit a `u32`) before
/// allocating the frame.
pub fn encode_frame(body: &[u8], limits: &NetLimits) -> Result<Vec<u8>, NetError> {
    let len = u32::try_from(body.len()).map_err(|_| NetError::OversizeFrame {
        claimed: u32::MAX,
        max: limits.max_message_bytes,
    })?;
    limits.check_frame_len(len)?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + len as usize);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(body);
    Ok(frame)
}

/// Validates a 4-byte length prefix and returns the body length to allocate.
///
/// This is the gate: it runs on the bare prefix, before any body allocation.
/// A hostile prefix claiming gigabytes fails here as
/// [`NetError::OversizeFrame`] having allocated nothing but the 4-byte
/// header buffer.
pub fn decode_frame_len(
    header: [u8; FRAME_HEADER_LEN],
    limits: &NetLimits,
) -> Result<usize, NetError> {
    limits.check_frame_len(u32::from_be_bytes(header))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_preserves_body() {
        let limits = NetLimits::default();
        let body = b"authoritative delta";
        let frame = encode_frame(body, &limits).expect("encode");
        assert_eq!(
            &frame[..FRAME_HEADER_LEN],
            &(body.len() as u32).to_be_bytes()
        );
        let len = decode_frame_len(
            frame[..FRAME_HEADER_LEN].try_into().expect("header slice"),
            &limits,
        )
        .expect("gate");
        assert_eq!(len, body.len());
        assert_eq!(&frame[FRAME_HEADER_LEN..], body);
    }

    #[test]
    fn oversize_prefix_rejected_before_allocation() {
        // A 4 GiB claim against a 1-byte limit: the gate fails on the header
        // alone. No body buffer is ever allocated — the test itself allocates
        // nothing beyond the 4-byte header.
        let limits = NetLimits {
            max_message_bytes: 1,
            ..NetLimits::default()
        };
        let header = u32::MAX.to_be_bytes();
        assert!(matches!(
            decode_frame_len(header, &limits),
            Err(NetError::OversizeFrame {
                claimed: u32::MAX,
                ..
            })
        ));
    }

    #[test]
    fn oversize_body_rejected_on_encode() {
        let limits = NetLimits {
            max_message_bytes: 4,
            ..NetLimits::default()
        };
        assert!(matches!(
            encode_frame(b"too long", &limits),
            Err(NetError::OversizeFrame { .. })
        ));
    }

    #[test]
    fn empty_body_frames_and_gates() {
        let limits = NetLimits::default();
        let frame = encode_frame(&[], &limits).expect("empty body");
        assert_eq!(frame.len(), FRAME_HEADER_LEN);
        assert_eq!(
            decode_frame_len([0, 0, 0, 0], &limits).expect("empty gate"),
            0
        );
    }
}
