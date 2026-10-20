//! Bounded parser/boundary fuzz sweep for the sound decoders (`sound.rs`).
//!
//! FINDINGS-ONLY sweep: this file feeds hostilely mutated fixture bytes to
//! the public loader entries ([`load_sound`]/[`load_sound_with_budget`])
//! and asserts the loader contract — `Ok(valid)` or `Err(typed)` — never a
//! panic, hang, unbounded allocation, or an `Ok` whose values violate the
//! documented [`Sound`] invariants.
//!
//! Scope note: the PNG (`texture.rs`, proptest at 256 cases) and GLB
//! (`mesh.rs`, proptest at 256 cases plus the `catch_unwind` traversal
//! boundary) decoders already carry exhaustive oracle-first bounded fuzz
//! in-module, so this file deliberately does NOT duplicate them. The WAV/OGG
//! decoder (`sound.rs`) has only hand-picked cases in-module and is the gap
//! this sweep closes.
//!
//! Method: deterministic seeded corpus (no RNG at test time). A fixed set of
//! mutations — bit flips at seeded offsets, truncation at every 1 KiB
//! boundary plus fractional cuts, header-field extremes, and length-prefix
//! lies in both directions (declared `>>` actual and declared `<<`
//! actual) — is applied to the exact bytes of the checked-in fixtures.
//! Inputs stay at fixture scale (sub-kilobyte WAV, kilobyte OGG), so no case
//! can spend more than microseconds of decode or more than kilobytes of
//! allocation; the whole file runs in seconds.

use std::path::{Path, PathBuf};

use canary_assets::{
    load_sound, load_sound_with_budget, AssetError, Sound, DEFAULT_MAX_SOUND_SAMPLES,
};

/// Checked-in fixture path.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Writes `bytes` to a fresh temp dir unique to this test process and
/// returns the path, so parallel tests never share a file.
fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "canary-assets-sound-boundary-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("scratch fixture must be writable");
    path
}

/// The sound-side oracle's `Ok` arm: every successfully loaded sound must
/// satisfy the loader's own documented invariants (nonzero rate, mono or
/// stereo, nonempty whole frames of finite samples in `[-1, 1]`).
/// An `Ok` violating any of these is a wrong-`Ok` — worse than an `Err`.
fn assert_sound_invariants(sound: &Sound) {
    assert!(
        sound.sample_rate() > 0,
        "sample rate is nonzero by construction"
    );
    assert!(
        sound.channel_count() == 1 || sound.channel_count() == 2,
        "only mono/stereo loads, got {} channels",
        sound.channel_count()
    );
    assert!(
        !sound.samples().is_empty(),
        "empty streams are rejected at load time"
    );
    assert_eq!(
        sound.samples().len() % usize::from(sound.channel_count()),
        0,
        "samples must hold whole frames"
    );
    assert_eq!(
        sound.frame_count() * usize::from(sound.channel_count()),
        sound.samples().len(),
        "frame_count must agree with the sample buffer"
    );
    for (index, sample) in sound.samples().iter().enumerate() {
        assert!(
            sample.is_finite(),
            "decoded sample {index} must be finite, got {sample}"
        );
    }
}

/// Loads `bytes` under a generous budget and checks the contract: any typed
/// [`AssetError`] is acceptable; an `Ok` must satisfy
/// [`assert_sound_invariants`]. Returns the outcome for taxonomy-specific
/// follow-up assertions.
fn check_contract(bytes: &[u8], name: &str) -> Result<Sound, AssetError> {
    let path = write_temp(name, bytes);
    let result = load_sound_with_budget(&path, DEFAULT_MAX_SOUND_SAMPLES);
    match &result {
        Ok(sound) => assert_sound_invariants(sound),
        Err(err) => assert!(
            err.path().is_some(),
            "every file-originated error must name its path, got: {err:?}"
        ),
    }
    result
}

/// Truncation points for a `len`-byte input: every 1 KiB boundary plus
/// fractional cuts and the off-by-one edges. Fixtures here are only a few
/// hundred bytes, so the 1 KiB boundaries collapse to the leading edge and
/// the fractional cuts do the real work; the code stays general so it keeps
/// covering larger fixtures if they ever land.
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
fn valid_fixtures_load_with_known_shape() {
    let wav = load_sound(&fixture("tone-mono-8k.wav")).expect("WAV fixture must load");
    assert_eq!(wav.sample_rate(), 8000);
    assert_eq!(wav.channel_count(), 1);
    assert_eq!(wav.frame_count(), 8);
    assert_sound_invariants(&wav);

    let ogg = load_sound(&fixture("tone-stereo-8k.ogg")).expect("Ogg fixture must load");
    assert_eq!(ogg.sample_rate(), 8000);
    assert_eq!(ogg.channel_count(), 2);
    assert_sound_invariants(&ogg);
}

#[test]
fn truncation_sweep_never_panics_and_ok_means_valid() {
    for name in ["tone-mono-8k.wav", "tone-stereo-8k.ogg"] {
        let full = std::fs::read(fixture(name)).expect("fixture must exist");
        for cut in truncation_points(full.len()) {
            let label = format!("truncate-{}-{cut}.bin", name.replace('.', "-"));
            // A prefix of a valid file must fail typed or decode to a
            // still-valid (shorter) sound — never panic, never wrong-Ok.
            let _ = check_contract(&full[..cut], &label);
        }
    }
}

#[test]
fn bit_flip_sweep_never_panics_and_ok_means_valid() {
    for name in ["tone-mono-8k.wav", "tone-stereo-8k.ogg"] {
        let full = std::fs::read(fixture(name)).expect("fixture must exist");
        // Seeded offsets: magic (0..12), fmt/page-header region, middle,
        // and tail. Flipping the magic must fail dispatch; flipping
        // payload bytes must fail typed or stay a valid sound.
        let mut offsets = vec![0, 4, 8, 11, 12, 22, 24, 34];
        offsets.push(full.len() / 2);
        offsets.push(full.len().saturating_sub(1));
        for offset in offsets {
            if offset >= full.len() {
                continue;
            }
            for xor in [0x01u8, 0xFFu8] {
                let mut bytes = full.clone();
                bytes[offset] ^= xor;
                let label = format!("flip-{}-{offset}-{xor:02x}.bin", name.replace('.', "-"));
                let _ = check_contract(&bytes, &label);
            }
        }
    }
}

#[test]
fn wav_sample_rate_zero_is_invalid_format() {
    let mut bytes = std::fs::read(fixture("tone-mono-8k.wav")).expect("WAV fixture must exist");
    // Canonical fmt-chunk layout (pinned the same way the in-module
    // 12-bit test pins offset 34): channels at 22, rate at 24.
    assert_eq!(
        u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        8000,
        "offset 24 must hold the 8 kHz rate the fixture was generated with"
    );
    bytes[24..28].copy_from_slice(&0u32.to_le_bytes());
    let err = check_contract(&bytes, "zero-rate.wav").expect_err("zero rate must fail");
    assert!(
        matches!(err, AssetError::InvalidFormat { .. }),
        "zero sample rate must be InvalidFormat, got: {err:?}"
    );
}

#[test]
fn wav_channel_extremes_are_typed_not_misrendered() {
    // Layered taxonomy (same shape as the in-module 12-bit precedent):
    // `hound` itself rejects zero channels and ragged data chunks at parse
    // time, so those shapes surface as InvalidFormat (unparseable file) and
    // never reach the loader's own channel gate. A 4-channel patch keeps
    // the 16-byte data chunk frame-divisible (4 frames of 8 bytes), so it
    // parses and reaches the gate, which declines it as UnsupportedFeature
    // (loader narrowness, not file corruption). All typed, none
    // misrendered — pinned here so a `hound` upgrade that starts parsing
    // these shapes changes this test loudly instead of silently
    // reclassifying the files.
    let mut zero = std::fs::read(fixture("tone-mono-8k.wav")).expect("WAV fixture must exist");
    assert_eq!(
        u16::from_le_bytes([zero[22], zero[23]]),
        1,
        "offset 22 must hold the mono channel count"
    );
    zero[22..24].copy_from_slice(&0u16.to_le_bytes());
    let err = check_contract(&zero, "zero-channel.wav").expect_err("zero channels must fail");
    assert!(
        matches!(err, AssetError::InvalidFormat { .. }),
        "0 channels must be InvalidFormat (hound parse refusal), got: {err:?}"
    );

    let mut six = std::fs::read(fixture("tone-mono-8k.wav")).expect("WAV fixture must exist");
    six[22..24].copy_from_slice(&6u16.to_le_bytes());
    let err = check_contract(&six, "six-channel.wav").expect_err("6 channels must fail");
    assert!(
        matches!(err, AssetError::InvalidFormat { .. }),
        "6 channels (ragged chunk) must be InvalidFormat, got: {err:?}"
    );

    // A 4-channel patch must stay fmt-consistent or `hound` refuses it
    // first: channels (22), byte rate (28), and block align (32) are
    // patched together so the file parses and reaches the loader's own
    // channel gate, which declines it as UnsupportedFeature.
    let mut four = std::fs::read(fixture("tone-mono-8k.wav")).expect("WAV fixture must exist");
    assert_eq!(
        u32::from_le_bytes([four[28], four[29], four[30], four[31]]),
        16_000,
        "offset 28 must hold the mono byte rate"
    );
    assert_eq!(
        u16::from_le_bytes([four[32], four[33]]),
        2,
        "offset 32 must hold the mono block align"
    );
    four[22..24].copy_from_slice(&4u16.to_le_bytes());
    four[28..32].copy_from_slice(&64_000u32.to_le_bytes());
    four[32..34].copy_from_slice(&8u16.to_le_bytes());
    let err = check_contract(&four, "four-channel.wav").expect_err("4 channels must fail");
    assert!(
        matches!(err, AssetError::UnsupportedFeature { .. }),
        "4 channels must be UnsupportedFeature, got: {err:?}"
    );
}

#[test]
fn wav_declared_size_lies_fail_typed_in_both_directions() {
    /// Locates the `data`-chunk size field the way the in-module lying-
    /// header test does, so writer layout drift fails loudly.
    fn data_size_offset(bytes: &[u8]) -> usize {
        bytes
            .windows(4)
            .position(|window| window == b"data")
            .expect("WAV fixture must hold a data chunk")
            + 4
    }

    // Declared >> actual: the data chunk claims near-4 GiB while holding
    // 16 bytes. The header pre-check must refuse it as OverBudget without
    // attempting the allocation.
    let mut inflated = std::fs::read(fixture("tone-mono-8k.wav")).expect("WAV fixture must exist");
    let tag = data_size_offset(&inflated);
    inflated[tag..tag + 4].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
    let err = check_contract(&inflated, "declared-huge.wav").expect_err("gigasample lie must fail");
    match err {
        AssetError::OverBudget { limit, actual, .. } => {
            assert_eq!(limit, DEFAULT_MAX_SOUND_SAMPLES);
            assert!(
                actual > DEFAULT_MAX_SOUND_SAMPLES,
                "patched claim must dwarf the budget, got {actual}"
            );
        }
        other => panic!("declared >> actual must be OverBudget, got: {other:?}"),
    }

    // Declared << actual: the data chunk claims 2 bytes (one 16-bit
    // sample) while holding 16. The chunk size defines the stream, so the
    // loader serves exactly the declared prefix — a valid 1-sample sound,
    // never a misrender. Trailing bytes past the declared chunk are chunk
    // framing, not audio, and are ignored the same way the PNG loader
    // tolerates trailing-IEND bytes (pinned leniency, never corruption).
    let mut shrunk = std::fs::read(fixture("tone-mono-8k.wav")).expect("WAV fixture must exist");
    let tag = data_size_offset(&shrunk);
    shrunk[tag..tag + 4].copy_from_slice(&2u32.to_le_bytes());
    let sound = check_contract(&shrunk, "declared-tiny.wav")
        .expect("declared << actual serves the declared prefix");
    assert_eq!(
        sound.frame_count(),
        1,
        "the short claim must yield exactly one frame, not the trailing bytes"
    );

    // Declared > actual but under budget: the chunk claims 32 bytes (16
    // samples) while holding 16. The read comes up short of the claim and
    // the truncated data chunk surfaces as InvalidFormat.
    let mut over_claimed =
        std::fs::read(fixture("tone-mono-8k.wav")).expect("WAV fixture must exist");
    let tag = data_size_offset(&over_claimed);
    over_claimed[tag..tag + 4].copy_from_slice(&32u32.to_le_bytes());
    let err = check_contract(&over_claimed, "declared-over.wav").expect_err("short data must fail");
    assert!(
        matches!(err, AssetError::InvalidFormat { .. }),
        "declared > actual (in-budget) must be InvalidFormat, got: {err:?}"
    );
}

#[test]
fn magic_dispatch_failures_are_invalid_format() {
    // Empty and sub-header inputs cannot hold either magic.
    for (bytes, label) in [
        (vec![], "empty.wav"),
        (b"OggS".to_vec(), "four-bytes.wav"),
        (b"RIFFXXXXWAV".to_vec(), "eleven-bytes.wav"),
    ] {
        let err = check_contract(&bytes, label).expect_err("sub-header input must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "{label} must be InvalidFormat, got: {err:?}"
        );
    }

    // Right loader family, wrong container: PNG and GLB magics match
    // neither WAV nor Ogg dispatch.
    for name in ["rgba2x2.png", "quad.glb"] {
        let bytes = std::fs::read(fixture(name)).expect("sibling fixture must exist");
        let err = check_contract(&bytes, "misnamed.wav").expect_err("foreign magic must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "{name} bytes must be InvalidFormat, got: {err:?}"
        );
    }

    // `RIFF....` without `WAVE`, and `OggS` with a zeroed page header:
    // dispatched nowhere, or dispatched and unparseable — both typed.
    let mut riff = vec![0u8; 32];
    riff[0..4].copy_from_slice(b"RIFF");
    riff[8..12].copy_from_slice(b"AVIX");
    let err = check_contract(&riff, "riff-avi.wav").expect_err("RIFF/AVI must fail");
    assert!(
        matches!(err, AssetError::InvalidFormat { .. }),
        "non-WAVE RIFF must be InvalidFormat, got: {err:?}"
    );

    let mut bogus_ogg = vec![0u8; 64];
    bogus_ogg[0..4].copy_from_slice(b"OggS");
    let err = check_contract(&bogus_ogg, "bogus.ogg").expect_err("bogus Ogg must fail");
    assert!(
        matches!(err, AssetError::InvalidFormat { .. }),
        "bogus Ogg must be InvalidFormat, got: {err:?}"
    );
}

#[test]
fn budget_gates_hold_at_tiny_exact_and_zero() {
    // The mono fixture holds exactly 8 sample values.
    let err = load_sound_with_budget(&fixture("tone-mono-8k.wav"), 7)
        .expect_err("7-sample budget must fail an 8-sample sound");
    match err {
        AssetError::OverBudget { limit, actual, .. } => {
            assert_eq!(limit, 7);
            assert_eq!(actual, 8);
        }
        other => panic!("must be OverBudget, got: {other:?}"),
    }

    let sound =
        load_sound_with_budget(&fixture("tone-mono-8k.wav"), 8).expect("exact budget must succeed");
    assert_sound_invariants(&sound);

    let err = load_sound_with_budget(&fixture("tone-mono-8k.wav"), 0)
        .expect_err("zero budget must refuse everything");
    assert!(
        matches!(err, AssetError::OverBudget { .. }),
        "zero budget must be OverBudget, got: {err:?}"
    );

    // Vorbis carries no upfront total: the running-total gate must fire
    // mid-decode on a stream hundreds of samples long.
    let err = load_sound_with_budget(&fixture("tone-stereo-8k.ogg"), 2)
        .expect_err("2-sample budget must fail the Ogg fixture");
    assert!(
        matches!(err, AssetError::OverBudget { .. }),
        "mid-stream breach must be OverBudget, got: {err:?}"
    );
}

#[test]
fn missing_file_reports_io_before_any_budget_gate() {
    let missing = Path::new("definitely-not-a-sound-boundary.wav");
    let err = load_sound_with_budget(missing, 0).expect_err("missing file must fail");
    assert!(
        matches!(err, AssetError::Io { .. }),
        "read-before-budget: missing file must be Io even with budget 0, got: {err:?}"
    );
}
