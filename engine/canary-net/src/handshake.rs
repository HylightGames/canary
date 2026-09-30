//! Typed handshake vocabulary: hello, welcome, reject (ADR 0027, point 7).
//!
//! The handshake is the first exchange on a new connection, before any
//! snapshot, delta, or input flows. The client sends one [`Hello`]
//! announcing the four version domains it speaks and understands — the wire
//! [`ProtocolVersion`](crate::ids::ProtocolVersion), the
//! [`SchemaManifestVersion`](crate::ids::SchemaManifestVersion), the
//! [`GameVersion`](crate::ids::GameVersion), and the
//! [`PluginApiVersion`](crate::ids::PluginApiVersion) it requires — plus a
//! log-only client label; the server answers with either [`Welcome`] (an
//! assigned player slot plus the session id that binds later messages to
//! this connection, per ADR 0027 point 9) or a typed [`Reject`] naming the
//! reason. A version mismatch is a typed rejection followed by closing the
//! connection — never a silent drop, never a raw byte comparison at the
//! call site.
//!
//! Compatibility domains stay independent: each version field is checked
//! separately, so sharing an engine release number implies nothing
//! (ADR 0027, point 7). ALPN pinning already happens underneath at the TLS
//! layer (see [`crate::transport::QUIC_ALPN_CANARY_1`]); this vocabulary is
//! the application-level gate above it.

use serde::{Deserialize, Serialize};

use crate::error::NetError;
use crate::ids::{GameVersion, PluginApiVersion, ProtocolVersion, SchemaManifestVersion};
use crate::limits::NetLimits;

/// Maximum accepted [`Hello::client_label`] length in bytes.
///
/// Labels are log-only diagnostics, never identity — but they still ride
/// the wire inside a bounded message, so an unbounded label is a memory
/// spend against the decoder. enforced in [`decode_hello`] after structural
/// decode: over-long labels fail as [`NetError::InvalidInput`], never by
/// truncating (a truncated label would misattribute logs).
pub const MAX_CLIENT_LABEL_BYTES: usize = 128;

/// First message a client sends: what it speaks and what it understands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Wire protocol version the client speaks.
    pub protocol_version: ProtocolVersion,
    /// Schema/content manifest version the client understands.
    pub schema_manifest: SchemaManifestVersion,
    /// Game release version the client runs. Checked independently of the
    /// wire protocol and schema manifest: same engine, different game
    /// content is still a mismatch.
    pub game_version: GameVersion,
    /// Plugin/API surface version the client requires. Checked
    /// independently: a peer needing a plugin surface the server does not
    /// support never reaches the simulation.
    pub plugin_api_version: PluginApiVersion,
    /// Human-readable client label for logs only. Never trusted for
    /// identity or authority (ADR 0027, point 9). Bounded by
    /// [`MAX_CLIENT_LABEL_BYTES`]; see [`decode_hello`].
    pub client_label: String,
}

/// Server acceptance: the slot this connection may submit input for, and
/// the session id binding its later messages to this connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// Wire protocol version the server speaks (echoed so the client can
    /// confirm the negotiated contract).
    pub protocol_version: ProtocolVersion,
    /// Player slot assigned to this connection. Client input naming any
    /// other slot fails ingress validation (see [`crate::input`]).
    pub assigned_slot: u64,
    /// Session id binding this connection's later messages together. A
    /// reconnect negotiates a fresh one; ids are never reused across
    /// connections.
    pub session_id: u64,
}

/// Typed handshake refusal. The server sends exactly one of these, then
/// closes the connection — the client must not retry the same bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reject {
    /// Why the handshake was refused.
    pub reason: RejectReason,
    /// Wire protocol version this server speaks, so the client can report
    /// (or act on) the mismatch without guessing.
    pub supported_protocol: ProtocolVersion,
}

/// Why a handshake was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// The client's wire protocol version is unsupported.
    UnsupportedProtocol,
    /// The client's schema manifest version is unsupported.
    UnsupportedSchemaManifest,
    /// The client's game release version is unsupported.
    UnsupportedGameVersion,
    /// The client's required plugin/API surface version is unsupported.
    UnsupportedPluginApiVersion,
    /// The server is not admitting new connections right now.
    ServerBusy,
    /// The peer's label is temporarily banned after repeated handshake
    /// rejects ([`crate::policy::HandshakeGate`]). The client must back
    /// off and retry later with a fresh handshake — retrying the same
    /// bytes immediately fails the same way until the ban expires.
    TemporarilyBanned,
}

/// Decides a handshake against the server's supported versions.
///
/// Every version domain is checked independently, in field order
/// (protocol, schema manifest, game, plugin/API): the first mismatch wins,
/// so a hello that is wrong in several domains reports the earliest one.
/// Returns the [`Welcome`] to send (with the caller-supplied `assigned_slot`
/// and fresh `session_id`), or the typed [`Reject`] to send before closing
/// the connection. No session state is created on rejection — the caller
/// must not insert a client record for a rejected hello.
pub fn decide_handshake(
    hello: &Hello,
    supported_protocol: ProtocolVersion,
    supported_manifest: SchemaManifestVersion,
    supported_game: GameVersion,
    supported_plugin_api: PluginApiVersion,
    assigned_slot: u64,
    session_id: u64,
) -> Result<Welcome, Reject> {
    if hello.protocol_version != supported_protocol {
        return Err(Reject {
            reason: RejectReason::UnsupportedProtocol,
            supported_protocol,
        });
    }
    if hello.schema_manifest != supported_manifest {
        return Err(Reject {
            reason: RejectReason::UnsupportedSchemaManifest,
            supported_protocol,
        });
    }
    if hello.game_version != supported_game {
        return Err(Reject {
            reason: RejectReason::UnsupportedGameVersion,
            supported_protocol,
        });
    }
    if hello.plugin_api_version != supported_plugin_api {
        return Err(Reject {
            reason: RejectReason::UnsupportedPluginApiVersion,
            supported_protocol,
        });
    }
    Ok(Welcome {
        protocol_version: supported_protocol,
        assigned_slot,
        session_id,
    })
}

/// Encodes a [`Hello`] for the wire, gated by `limits` before it reaches
/// the transport.
pub fn encode_hello(hello: &Hello, limits: &NetLimits) -> Result<Vec<u8>, NetError> {
    let bytes = postcard::to_allocvec(hello)?;
    check_len(&bytes, limits)?;
    Ok(bytes)
}

/// Decodes a [`Hello`], rejecting trailing bytes (no smuggled framing past
/// the message boundary) and labels past [`MAX_CLIENT_LABEL_BYTES`].
///
/// The label bound runs after structural decode: [`Hello::client_label`] is
/// log-only, never identity, but it still spends decoder memory, so an
/// unbounded label fails as [`NetError::InvalidInput`] rather than
/// truncating (truncation would misattribute logs).
pub fn decode_hello(bytes: &[u8]) -> Result<Hello, NetError> {
    let (hello, remainder): (Hello, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    if hello.client_label.len() > MAX_CLIENT_LABEL_BYTES {
        return Err(NetError::InvalidInput {
            detail: "hello client label exceeds the label bound".to_string(),
        });
    }
    Ok(hello)
}

/// Encodes a [`Welcome`] for the wire, gated by `limits`.
pub fn encode_welcome(welcome: &Welcome, limits: &NetLimits) -> Result<Vec<u8>, NetError> {
    let bytes = postcard::to_allocvec(welcome)?;
    check_len(&bytes, limits)?;
    Ok(bytes)
}

/// Decodes a [`Welcome`], rejecting trailing bytes.
pub fn decode_welcome(bytes: &[u8]) -> Result<Welcome, NetError> {
    let (welcome, remainder): (Welcome, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    Ok(welcome)
}

/// Encodes a [`Reject`] for the wire, gated by `limits`.
pub fn encode_reject(reject: &Reject, limits: &NetLimits) -> Result<Vec<u8>, NetError> {
    let bytes = postcard::to_allocvec(reject)?;
    check_len(&bytes, limits)?;
    Ok(bytes)
}

/// Decodes a [`Reject`], rejecting trailing bytes.
pub fn decode_reject(bytes: &[u8]) -> Result<Reject, NetError> {
    let (reject, remainder): (Reject, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    Ok(reject)
}

/// Gates an encoded handshake message against the frame bound before it
/// reaches the transport. Handshake messages are small; anything over the
/// bound is a local encoding anomaly, reported as oversize.
fn check_len(bytes: &[u8], limits: &NetLimits) -> Result<(), NetError> {
    let len = u32::try_from(bytes.len()).map_err(|_| NetError::OversizeFrame {
        claimed: u32::MAX,
        max: limits.max_message_bytes,
    })?;
    limits.check_frame_len(len)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{GameVersion, PluginApiVersion, PROTOCOL_VERSION_1};

    /// Game release both ends of these unit tests run.
    const TEST_GAME: GameVersion = GameVersion(11);
    /// Plugin/API surface both ends of these unit tests support.
    const TEST_PLUGIN_API: PluginApiVersion = PluginApiVersion(4);

    fn hello() -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION_1,
            schema_manifest: SchemaManifestVersion(3),
            game_version: TEST_GAME,
            plugin_api_version: TEST_PLUGIN_API,
            client_label: "loopback-proof".to_string(),
        }
    }

    fn decide(hello: &Hello) -> Result<Welcome, Reject> {
        decide_handshake(
            hello,
            PROTOCOL_VERSION_1,
            SchemaManifestVersion(3),
            TEST_GAME,
            TEST_PLUGIN_API,
            7,
            99,
        )
    }

    #[test]
    fn matching_versions_are_welcomed_with_slot_and_session() {
        let welcome = decide_handshake(
            &hello(),
            PROTOCOL_VERSION_1,
            SchemaManifestVersion(3),
            TEST_GAME,
            TEST_PLUGIN_API,
            7,
            99,
        )
        .expect("compatible hello");
        assert_eq!(welcome.assigned_slot, 7);
        assert_eq!(welcome.session_id, 99);
        assert_eq!(welcome.protocol_version, PROTOCOL_VERSION_1);
    }

    #[test]
    fn protocol_mismatch_is_a_typed_reject_naming_supported_version() {
        let mut stale = hello();
        stale.protocol_version = ProtocolVersion(0);
        let reject = decide(&stale).expect_err("stale protocol must be rejected");
        assert_eq!(reject.reason, RejectReason::UnsupportedProtocol);
        assert_eq!(reject.supported_protocol, PROTOCOL_VERSION_1);
    }

    #[test]
    fn manifest_mismatch_is_a_typed_reject_checked_independently() {
        // Same wire protocol, different content understanding: still a
        // reject, with its own reason — the domains do not cover for each
        // other.
        let mut foreign = hello();
        foreign.schema_manifest = SchemaManifestVersion(4);
        let reject = decide(&foreign).expect_err("foreign manifest must be rejected");
        assert_eq!(reject.reason, RejectReason::UnsupportedSchemaManifest);
    }

    #[test]
    fn game_mismatch_is_a_typed_reject_checked_independently() {
        // Same wire protocol and schema manifest, different game release:
        // still a reject, with its own reason — sharing an engine build
        // implies nothing about game-content compatibility.
        let mut foreign = hello();
        foreign.game_version = GameVersion(12);
        let reject = decide(&foreign).expect_err("foreign game must be rejected");
        assert_eq!(reject.reason, RejectReason::UnsupportedGameVersion);
    }

    #[test]
    fn plugin_api_mismatch_is_a_typed_reject_checked_independently() {
        // Same wire protocol, manifest, and game, different required
        // plugin surface: still a reject, with its own reason.
        let mut foreign = hello();
        foreign.plugin_api_version = PluginApiVersion(5);
        let reject = decide(&foreign).expect_err("foreign plugin API must be rejected");
        assert_eq!(reject.reason, RejectReason::UnsupportedPluginApiVersion);
    }

    #[test]
    fn first_mismatched_domain_wins_in_field_order() {
        // Wrong in every domain at once: protocol reports first (field
        // order), so the client fixes one domain at a time deterministically.
        let foreign = Hello {
            protocol_version: ProtocolVersion(0),
            schema_manifest: SchemaManifestVersion(99),
            game_version: GameVersion(99),
            plugin_api_version: PluginApiVersion(99),
            client_label: "foreign".to_string(),
        };
        let reject = decide(&foreign).expect_err("all-foreign hello must be rejected");
        assert_eq!(reject.reason, RejectReason::UnsupportedProtocol);
        // Protocol fixed, manifest still wrong: the next domain reports.
        let mut next = foreign;
        next.protocol_version = PROTOCOL_VERSION_1;
        let reject = decide(&next).expect_err("manifest still foreign");
        assert_eq!(reject.reason, RejectReason::UnsupportedSchemaManifest);
    }

    #[test]
    fn client_label_is_bounded_at_decode() {
        let limits = NetLimits::default();
        // At the bound decodes fine.
        let mut at_bound = hello();
        at_bound.client_label = "l".repeat(MAX_CLIENT_LABEL_BYTES);
        let bytes = encode_hello(&at_bound, &limits).expect("encode bounded label");
        assert_eq!(
            decode_hello(&bytes).expect("decode bounded label"),
            at_bound
        );
        // One byte past the bound fails as invalid input, never by
        // truncating the label the logs would attribute.
        let mut past_bound = hello();
        past_bound.client_label = "l".repeat(MAX_CLIENT_LABEL_BYTES + 1);
        let bytes = encode_hello(&past_bound, &limits).expect("encode over-long label");
        let error = decode_hello(&bytes).expect_err("over-long label must fail");
        assert!(
            matches!(error, NetError::InvalidInput { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn handshake_messages_round_trip_and_reject_trailing_bytes() {
        let limits = NetLimits::default();
        let bytes = encode_hello(&hello(), &limits).expect("encode hello");
        assert_eq!(decode_hello(&bytes).expect("decode hello"), hello());

        let welcome = decide_handshake(
            &hello(),
            PROTOCOL_VERSION_1,
            SchemaManifestVersion(3),
            TEST_GAME,
            TEST_PLUGIN_API,
            1,
            2,
        )
        .expect("welcome");
        let bytes = encode_welcome(&welcome, &limits).expect("encode welcome");
        assert_eq!(decode_welcome(&bytes).expect("decode welcome"), welcome);

        let reject = Reject {
            reason: RejectReason::ServerBusy,
            supported_protocol: PROTOCOL_VERSION_1,
        };
        let bytes = encode_reject(&reject, &limits).expect("encode reject");
        assert_eq!(decode_reject(&bytes).expect("decode reject"), reject);

        let mut smuggled = encode_hello(&hello(), &limits).expect("encode");
        smuggled.extend_from_slice(b"extra");
        assert!(matches!(
            decode_hello(&smuggled),
            Err(NetError::TrailingBytes { .. })
        ));
    }

    #[test]
    fn protocol_mismatch_maps_to_the_typed_transport_error() {
        // The session layer reports the mismatch as a structured error
        // before closing; callers never compare version bytes by hand.
        let error = NetError::UnsupportedProtocol {
            got: 0,
            supported: 1,
        };
        assert_eq!(
            error.disconnect_reason(),
            crate::error::DisconnectReason::ProtocolViolation
        );
        assert!(!error.is_retryable());
    }

    #[test]
    fn temporary_ban_reject_round_trips_as_typed_refusal() {
        // The ban path sends exactly one typed refusal, then closes: the
        // client learns the ban (not a version mismatch) without guessing.
        let limits = NetLimits::default();
        let reject = Reject {
            reason: RejectReason::TemporarilyBanned,
            supported_protocol: PROTOCOL_VERSION_1,
        };
        let bytes = encode_reject(&reject, &limits).expect("encode ban");
        let decoded = decode_reject(&bytes).expect("decode ban");
        assert_eq!(decoded.reason, RejectReason::TemporarilyBanned);
        assert_eq!(decoded.supported_protocol, PROTOCOL_VERSION_1);
    }
}
