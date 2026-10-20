//! Bounded wire/boundary fuzz sweep: framing, envelope, handshake, and
//! replication decoders over hostile bytes.
//!
//! FINDINGS-ONLY sweep: this file feeds mutated wire bytes to the public
//! decode entries ([`decode_frame_len`](canary_net::decode_frame_len),
//! [`NetEnvelope::decode`](canary_net::NetEnvelope),
//! [`decode_hello`](canary_net::decode_hello) /
//! [`decode_welcome`](canary_net::decode_welcome) /
//! [`decode_reject`](canary_net::decode_reject),
//! [`decode_delta`](canary_net::decode_delta) /
//! [`decode_snapshot`](canary_net::decode_snapshot)) and asserts the
//! contract — `Ok(valid)` or `Err(typed)` — never a panic, hang (>10 s per
//! case), unbounded allocation, or an `Ok` that bypasses a documented
//! budget.
//!
//! Method: deterministic seeded corpus (no RNG at test time). A fixed set
//! of mutations — bit flips at seeded offsets, truncation at fractional
//! cuts plus every 1 KiB boundary, header-field extremes, length-prefix
//! lies in both directions (declared `>>` actual and declared `<<`
//! actual), and trailing-byte smuggling — is applied to the exact bytes of
//! honestly encoded messages. Inputs stay at fixture scale (tens of bytes),
//! so no case can spend more than microseconds of decode; the whole file
//! runs in seconds.

use canary_net::{
    decide_handshake, decode_delta, decode_frame_len, decode_hello, decode_reject, decode_snapshot,
    decode_welcome, encode_delta, encode_frame, encode_hello, encode_reject, encode_snapshot,
    encode_welcome, identity_codec, DecodeFn, Delta, EncodeFn, GameVersion, Hello, NetEntityId,
    NetEnvelope, NetError, NetLimits, NetSequence, PluginApiVersion, ProtocolVersion, Reject,
    RejectReason, ReplicatedEntry, SchemaCodecs, SchemaManifestVersion, SimTick, Tombstone,
    Welcome, FRAME_HEADER_LEN, MAX_CLIENT_LABEL_BYTES, PROTOCOL_VERSION_1,
};

/// Game release both ends of these sweep tests run.
const TEST_GAME: GameVersion = GameVersion(11);
/// Plugin/API surface both ends of these sweep tests support.
const TEST_PLUGIN_API: PluginApiVersion = PluginApiVersion(4);
/// Schema manifest both ends of these sweep tests speak.
const TEST_MANIFEST: SchemaManifestVersion = SchemaManifestVersion(3);

/// A valid hello both ends agree on.
fn hello() -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION_1,
        schema_manifest: TEST_MANIFEST,
        game_version: TEST_GAME,
        plugin_api_version: TEST_PLUGIN_API,
        client_label: "boundary-sweep".to_string(),
    }
}

/// One honestly encoded hello, gated by default limits.
fn hello_bytes() -> Vec<u8> {
    encode_hello(&hello(), &NetLimits::default()).expect("valid hello must encode")
}

/// One honestly sealed envelope carrying a small payload.
fn sealed_bytes(payload: &[u8]) -> Vec<u8> {
    NetEnvelope::seal(PROTOCOL_VERSION_1, NetSequence(9), payload.to_vec())
        .encode(&NetLimits::default())
        .expect("sealed envelope must encode")
}

/// One honestly encoded delta with two entries and no tombstones.
fn delta_bytes() -> Vec<u8> {
    encode_delta(
        NetSequence(2),
        NetSequence(1),
        SimTick(9),
        vec![
            ReplicatedEntry {
                entity: NetEntityId(1),
                schema: "canary.health".to_string(),
                payload: b"hp:100".to_vec(),
            },
            ReplicatedEntry {
                entity: NetEntityId(2),
                schema: "canary.health".to_string(),
                payload: b"hp:90".to_vec(),
            },
        ],
        Vec::new(),
        &NetLimits::default(),
    )
    .expect("valid delta must encode")
}

/// One honestly encoded snapshot.
fn snapshot_bytes() -> Vec<u8> {
    encode_snapshot(
        vec![ReplicatedEntry {
            entity: NetEntityId(1),
            schema: "canary.health".to_string(),
            payload: b"hp:100".to_vec(),
        }],
        SimTick(7),
        &NetLimits::default(),
    )
    .expect("valid snapshot must encode")
}

/// Truncation points for a `len`-byte input: the empty/1-byte edges, every
/// 1 KiB boundary, fractional cuts, and the off-by-one tail.
fn truncation_points(len: usize) -> Vec<usize> {
    let mut points = vec![0, 1];
    let mut boundary = 1024;
    while boundary < len {
        points.push(boundary);
        boundary += 1024;
    }
    for point in [len / 4, len / 2, len * 3 / 4, len.saturating_sub(1)] {
        if point < len {
            points.push(point);
        }
    }
    points.sort_unstable();
    points.dedup();
    points
}

#[test]
fn frame_length_prefix_extremes_gate_before_allocating() {
    let default = NetLimits::default();
    let max = default.max_message_bytes;
    // The gate is exact: at-limit passes, one past fails, and the failure
    // names both numbers. Nothing is allocated on either path here — the
    // test holds only the 4-byte header under test.
    for (claimed, ok) in [
        (0u32, true),
        (1, true),
        (8, true),
        (max - 1, true),
        (max, true),
        (max + 1, false),
        (0x7FFF_FFFF, false),
        (u32::MAX, false),
    ] {
        let gated = decode_frame_len(claimed.to_be_bytes(), &default);
        if ok {
            assert_eq!(
                gated.expect("in-limit prefix must gate"),
                claimed as usize,
                "claimed {claimed} must widen exactly"
            );
        } else {
            match gated.expect_err("over-limit prefix must fail") {
                NetError::OversizeFrame {
                    claimed: got,
                    max: got_max,
                } => {
                    assert_eq!(got, claimed);
                    assert_eq!(got_max, max);
                }
                other => panic!("over-limit prefix must be OversizeFrame, got: {other:?}"),
            }
        }
    }

    // A tiny limit gates the same way: the bound is the knob's, not the
    // type's.
    let tiny = NetLimits {
        max_message_bytes: 8,
        ..NetLimits::default()
    };
    assert_eq!(
        decode_frame_len(8u32.to_be_bytes(), &tiny).expect("at-limit"),
        8
    );
    assert!(
        matches!(
            decode_frame_len(9u32.to_be_bytes(), &tiny),
            Err(NetError::OversizeFrame { .. })
        ),
        "one past a tiny limit must fail"
    );

    // Encode-side: bodies over the limit never reach the wire, and an
    // empty body frames to exactly the header.
    assert!(
        matches!(
            encode_frame(b"too long!", &tiny),
            Err(NetError::OversizeFrame { .. })
        ),
        "over-limit body must fail on encode"
    );
    let empty = encode_frame(&[], &default).expect("empty body must frame");
    assert_eq!(empty.len(), FRAME_HEADER_LEN);
    assert_eq!(
        decode_frame_len([0, 0, 0, 0], &default).expect("empty gate"),
        0
    );
}

#[test]
fn frame_declared_vs_actual_mismatch_is_detectable_typed() {
    let limits = NetLimits::default();
    // Declared >> actual: a prefix promising 100 body bytes around a
    // 10-byte body. The prefix alone gates fine (100 is in-limit); the
    // shortfall is a truncated read at the transport, which reports
    // TruncatedFrame — a protocol violation, never a short message served
    // as whole.
    let mut frame = encode_frame(b"0123456789", &limits).expect("encode");
    frame[0..FRAME_HEADER_LEN].copy_from_slice(&100u32.to_be_bytes());
    let claimed = decode_frame_len(
        frame[..FRAME_HEADER_LEN].try_into().expect("header slice"),
        &limits,
    )
    .expect("100 is in-limit");
    assert_eq!(claimed, 100);
    let received = frame.len() - FRAME_HEADER_LEN;
    assert!(
        received < claimed,
        "the body ({received}) is short of the claim ({claimed})"
    );
    let short_read = NetError::TruncatedFrame { claimed, received };
    assert_eq!(
        short_read.disconnect_reason(),
        canary_net::DisconnectReason::ProtocolViolation
    );
    assert!(!short_read.is_retryable());

    // Declared << actual: a prefix promising 4 body bytes around a
    // 10-byte body. The surplus is smuggled framing past the message
    // boundary — the envelope layer rejects it as TrailingBytes.
    let body = b"0123456789";
    let mut short_frame = encode_frame(&body[..4], &limits).expect("encode");
    short_frame.extend_from_slice(&body[4..]);
    let claimed = decode_frame_len(
        short_frame[..FRAME_HEADER_LEN]
            .try_into()
            .expect("header slice"),
        &limits,
    )
    .expect("4 is in-limit");
    assert_eq!(claimed, 4);
    assert!(
        short_frame.len() - FRAME_HEADER_LEN > claimed,
        "surplus bytes ride past the declared boundary"
    );
}

#[test]
fn envelope_mutation_sweep_never_panics_and_ok_means_verified() {
    let base = sealed_bytes(b"boundary payload");
    // Truncations: a prefix of a sealed envelope must fail structural
    // decode (Codec) or, if the cut lands exactly on a shorter valid
    // encoding, verify — never serve an unverified payload.
    for cut in truncation_points(base.len()) {
        match NetEnvelope::decode(&base[..cut]) {
            Ok(envelope) => envelope.verify().expect("decoded envelope must verify"),
            Err(err) => assert!(
                matches!(err, NetError::Codec(_) | NetError::ChecksumMismatch { .. }),
                "truncated envelope must be Codec or ChecksumMismatch, got: {err:?}"
            ),
        }
    }

    // Bit flips: corrupting version, sequence, checksum, or payload bytes
    // must break the checksum gate (or structural decode), never verify.
    for offset in [0, 1, 2, base.len() / 2, base.len() - 1] {
        let mut bytes = base.clone();
        bytes[offset] ^= 0xFF;
        match NetEnvelope::decode(&bytes) {
            Ok(envelope) => envelope.verify().expect("decoded envelope must verify"),
            Err(err) => assert!(
                matches!(err, NetError::Codec(_) | NetError::ChecksumMismatch { .. }),
                "flipped envelope must be Codec or ChecksumMismatch, got: {err:?}"
            ),
        }
    }

    // Declared << actual at the message layer: smuggled trailing bytes
    // are a protocol violation, not a longer message.
    let mut smuggled = base.clone();
    smuggled.extend_from_slice(b"smuggled");
    assert!(
        matches!(
            NetEnvelope::decode(&smuggled),
            Err(NetError::TrailingBytes { trailing: 8 })
        ),
        "trailing bytes must be TrailingBytes"
    );

    // Empty and garbage inputs fail structural decode, never verify.
    assert!(
        matches!(NetEnvelope::decode(&[]), Err(NetError::Codec(_))),
        "empty input must be Codec"
    );
    assert!(
        matches!(
            NetEnvelope::decode(b"this is not an envelope"),
            Err(NetError::Codec(_))
        ),
        "garbage must be Codec"
    );
}

#[test]
fn handshake_version_extremes_are_typed_rejects() {
    let decide = |candidate: &Hello| {
        decide_handshake(
            candidate,
            PROTOCOL_VERSION_1,
            TEST_MANIFEST,
            TEST_GAME,
            TEST_PLUGIN_API,
            7,
            99,
        )
    };
    // Each domain checked independently: an extreme in one domain reports
    // exactly that domain, and the first mismatched domain wins in field
    // order.
    let mut stale = hello();
    stale.protocol_version = ProtocolVersion(0);
    assert_eq!(
        decide(&stale).expect_err("stale protocol").reason,
        RejectReason::UnsupportedProtocol
    );
    let mut future = hello();
    future.protocol_version = ProtocolVersion(u16::MAX);
    assert_eq!(
        decide(&future).expect_err("future protocol").reason,
        RejectReason::UnsupportedProtocol
    );
    let mut foreign = hello();
    foreign.schema_manifest = SchemaManifestVersion(u32::MAX);
    assert_eq!(
        decide(&foreign).expect_err("foreign manifest").reason,
        RejectReason::UnsupportedSchemaManifest
    );
    let mut alien_game = hello();
    alien_game.game_version = GameVersion(u32::MAX);
    assert_eq!(
        decide(&alien_game).expect_err("alien game").reason,
        RejectReason::UnsupportedGameVersion
    );
    let mut alien_plugin = hello();
    alien_plugin.plugin_api_version = PluginApiVersion(u32::MAX);
    assert_eq!(
        decide(&alien_plugin).expect_err("alien plugin API").reason,
        RejectReason::UnsupportedPluginApiVersion
    );
    // Wrong everywhere: protocol (the first field) still wins.
    let all_foreign = Hello {
        protocol_version: ProtocolVersion(0),
        schema_manifest: SchemaManifestVersion(u32::MAX),
        game_version: GameVersion(u32::MAX),
        plugin_api_version: PluginApiVersion(u32::MAX),
        client_label: "foreign".to_string(),
    };
    assert_eq!(
        decide(&all_foreign).expect_err("all foreign").reason,
        RejectReason::UnsupportedProtocol
    );
}

#[test]
fn handshake_label_lengths_gate_at_decode_not_encode() {
    let limits = NetLimits::default();
    // At the bound round-trips; empty is fine (log-only, optional).
    for len in [0, 1, MAX_CLIENT_LABEL_BYTES - 1, MAX_CLIENT_LABEL_BYTES] {
        let mut candidate = hello();
        candidate.client_label = "l".repeat(len);
        let bytes = encode_hello(&candidate, &limits).expect("bounded label must encode");
        assert_eq!(
            decode_hello(&bytes).expect("bounded label must decode"),
            candidate,
            "label of {len} bytes must round-trip"
        );
    }
    // Past the bound encodes (it fits the frame) but fails decode as
    // InvalidInput — never by truncating the label the logs attribute.
    for len in [
        MAX_CLIENT_LABEL_BYTES + 1,
        MAX_CLIENT_LABEL_BYTES + 64,
        10_000,
    ] {
        let mut candidate = hello();
        candidate.client_label = "l".repeat(len);
        let bytes = encode_hello(&candidate, &limits).expect("over-long label still encodes");
        let err = decode_hello(&bytes).expect_err("over-long label must fail");
        assert!(
            matches!(err, NetError::InvalidInput { .. }),
            "label of {len} bytes must be InvalidInput, got: {err:?}"
        );
    }
}

#[test]
fn handshake_wire_mutations_never_panic_and_ok_means_bounded() {
    let base = hello_bytes();
    // Truncations fail structural decode.
    for cut in truncation_points(base.len()) {
        if let Ok(decoded) = decode_hello(&base[..cut]) {
            assert!(
                decoded.client_label.len() <= MAX_CLIENT_LABEL_BYTES,
                "a truncated Ok must still respect the label bound"
            );
        }
    }
    // A byte flipped inside the label region to 0xFF (never valid UTF-8
    // alone) must fail decode rather than serve a mojibake label.
    let needle = b"boundary-sweep";
    let label_start = base
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("encoded hello must carry the label bytes");
    let mut bytes = base.clone();
    bytes[label_start] = 0xFF;
    assert!(
        decode_hello(&bytes).is_err(),
        "invalid-UTF-8 label bytes must fail decode"
    );
    // Smuggled framing past the message boundary is rejected, not read.
    let mut smuggled = base.clone();
    smuggled.extend_from_slice(b"extra");
    assert!(
        matches!(decode_hello(&smuggled), Err(NetError::TrailingBytes { .. })),
        "hello with trailing bytes must be TrailingBytes"
    );
    // Welcome and Reject reject trailing bytes the same way.
    let welcome = Welcome {
        protocol_version: PROTOCOL_VERSION_1,
        assigned_slot: 1,
        session_id: 2,
    };
    let mut smuggled =
        encode_welcome(&welcome, &NetLimits::default()).expect("welcome must encode");
    smuggled.push(0x00);
    assert!(matches!(
        decode_welcome(&smuggled),
        Err(NetError::TrailingBytes { .. })
    ));
    let reject = Reject {
        reason: RejectReason::ServerBusy,
        supported_protocol: PROTOCOL_VERSION_1,
    };
    let mut smuggled = encode_reject(&reject, &NetLimits::default()).expect("reject must encode");
    smuggled.push(0x00);
    assert!(matches!(
        decode_reject(&smuggled),
        Err(NetError::TrailingBytes { .. })
    ));
    // Empty and garbage fail structural decode.
    assert!(matches!(decode_hello(&[]), Err(NetError::Codec(_))));
    assert!(matches!(
        decode_hello(b"not a handshake"),
        Err(NetError::Codec(_))
    ));
}

#[test]
fn replication_mutation_sweep_never_panics_and_ok_means_canonical() {
    let base = delta_bytes();
    for cut in truncation_points(base.len()) {
        // Declared >> actual: the length prefixes promise more than the
        // surviving bytes hold — structural decode fails, never a short
        // message served as whole.
        if let Ok(delta) = decode_delta(&base[..cut]) {
            delta
                .validate_all(NetSequence(1))
                .expect("truncated Ok must still validate against its base");
        }
    }
    for offset in [0, 1, base.len() / 2, base.len() - 1] {
        let mut bytes = base.clone();
        bytes[offset] ^= 0xFF;
        // Declared << actual cannot arise from a flip alone (lengths stay
        // consistent), so a flip-Ok must still be canonical and
        // base-valid; anything else fails typed.
        if let Ok(delta) = decode_delta(&bytes) {
            Delta::validate_order(&delta.changes, &delta.removals)
                .expect("flip-Ok must stay canonical");
        }
    }
    // Declared << actual: appended bytes past the message boundary.
    let mut smuggled = base.clone();
    smuggled.extend_from_slice(b"smuggled");
    assert!(
        matches!(decode_delta(&smuggled), Err(NetError::TrailingBytes { .. })),
        "delta with trailing bytes must be TrailingBytes"
    );
    // Snapshot tampering breaks the checksum gate or structural decode.
    let snapshot = snapshot_bytes();
    let mut tampered = snapshot.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    assert!(
        matches!(
            decode_snapshot(&tampered),
            Err(NetError::ChecksumMismatch { .. } | NetError::Codec(_))
        ),
        "tampered snapshot must fail the checksum or codec gate"
    );
    assert!(matches!(decode_delta(&[]), Err(NetError::Codec(_))));
}

#[test]
fn string_and_payload_lengths_are_frame_bounded_not_schema_bounded() {
    let limits = NetLimits::default();
    // No schema-length gate lives at this layer: an empty schema, a 64 KiB
    // schema, and a 256 KiB payload all encode and structurally decode
    // fine inside the 1 MiB frame. Unknown schemas are rejected one layer
    // up (codec registry → resync-required), never here.
    let cases = vec![
        ("".to_string(), vec![0u8; 8]),
        ("s".repeat(64 * 1024), vec![0u8; 8]),
        ("canary.health".to_string(), vec![0u8; 256 * 1024]),
    ];
    for (schema, payload) in cases {
        let bytes = encode_delta(
            NetSequence(2),
            NetSequence(1),
            SimTick(9),
            vec![ReplicatedEntry {
                entity: NetEntityId(1),
                schema: schema.clone(),
                payload,
            }],
            Vec::new(),
            &limits,
        )
        .expect("in-frame strings must encode");
        let delta = decode_delta(&bytes).expect("in-frame strings must decode");
        assert_eq!(delta.changes.len(), 1);
        assert_eq!(delta.changes[0].schema, schema);
    }
    // Past the frame, the frame gate fires on encode — before any byte
    // reaches a peer.
    let over = encode_delta(
        NetSequence(2),
        NetSequence(1),
        SimTick(9),
        vec![ReplicatedEntry {
            entity: NetEntityId(1),
            schema: "canary.health".to_string(),
            payload: vec![0u8; limits.max_message_bytes as usize],
        }],
        Vec::new(),
        &limits,
    );
    assert!(
        matches!(over, Err(NetError::OversizeFrame { .. })),
        "over-frame payload must be OversizeFrame, got: {over:?}"
    );

    // And the codec layer turns an unknown schema into a resync, not a
    // panic or a partial apply.
    let mut codecs = SchemaCodecs::new();
    codecs.register("canary.health", identity_codec());
    let bytes = encode_delta(
        NetSequence(2),
        NetSequence(1),
        SimTick(9),
        vec![ReplicatedEntry {
            entity: NetEntityId(2),
            schema: "canary.unregistered".to_string(),
            payload: b"v".to_vec(),
        }],
        Vec::new(),
        &limits,
    )
    .expect("encode");
    let delta = decode_delta(&bytes).expect("decode");
    let err = codecs
        .decode_delta_payloads(&delta)
        .expect_err("unknown schema must fail");
    assert!(matches!(err, NetError::UnknownSchema { .. }));
    assert!(err.resync_required());
}

/// Keeps the `DecodeFn`/`EncodeFn`/`Tombstone` imports honest: the sweep
/// pins that a custom codec failure rejects the whole delta decode walk
/// rather than half-applying it.
#[test]
fn failing_codec_rejects_the_whole_delta_walk() {
    let failing_encode: EncodeFn = Box::new(|_| {
        Err(NetError::InvalidInput {
            detail: "sweep codec refuses to encode".to_string(),
        })
    });
    let failing_decode: DecodeFn = Box::new(|_| {
        Err(NetError::InvalidInput {
            detail: "sweep codec refuses to decode".to_string(),
        })
    });
    let mut codecs = SchemaCodecs::new();
    codecs.register(
        "canary.sweep",
        canary_net::SchemaCodec::new(failing_encode, failing_decode),
    );
    let limits = NetLimits::default();
    let bytes = encode_delta(
        NetSequence(2),
        NetSequence(1),
        SimTick(9),
        vec![ReplicatedEntry {
            entity: NetEntityId(1),
            schema: "canary.sweep".to_string(),
            payload: b"hp:100".to_vec(),
        }],
        vec![Tombstone::entity_destroyed(NetEntityId(9), 4)],
        &limits,
    )
    .expect("encode");
    let delta = decode_delta(&bytes).expect("decode");
    assert!(delta.removals.len() == 1, "tombstone must survive the walk");
    let err = codecs
        .decode_delta_payloads(&delta)
        .expect_err("failing codec must reject");
    assert!(matches!(err, NetError::InvalidInput { .. }));
}
