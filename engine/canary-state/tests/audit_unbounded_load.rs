//! Regression coverage from the 2026-10-24 v0.1.0 final audit (finding `009`).
//!
//! `AuthoredDocument::load` used to read the whole file with `read_to_string`
//! and no size budget (compare `canary-assets`' 64 MiB `DEFAULT_MAX_FILE_BYTES`
//! and `canary-loc`'s per-file budget). A hostile or corrupt project file can
//! therefore force an allocation as large as the file before any validation.
//! The test measures peak allocation while loading a 64 MiB sparse file; the
//! budget is [`canary_state::MAX_PROJECT_BYTES`] /
//! [`canary_state::MAX_SNAPSHOT_BYTES`], both far below the fixture.
//!
//! The counting allocator is process-wide, so this binary holds exactly one
//! test (see the proposed-test bundle README).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use canary_state::{load_snapshot, AuthoredDocument, StateError};

struct Counting;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn bump(n: usize) {
    let now = CUR.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(now, Relaxed);
}

// SAFETY: forwards every call to `System` unchanged; only counters are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: caller upholds the `GlobalAlloc` contract; forwarded verbatim.
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            bump(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: `p`/`l` came from `alloc`, which forwarded to `System`.
        unsafe { System.dealloc(p, l) };
        CUR.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        // SAFETY: as above.
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            CUR.fetch_sub(l.size(), Relaxed);
            bump(n);
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Measures peak allocation above the current baseline while `load` runs.
fn peak_while(load: impl FnOnce() -> Result<(), StateError>) -> (Result<(), StateError>, usize) {
    let base = CUR.load(Relaxed);
    PEAK.store(base, Relaxed);
    let result = load();
    let peak = PEAK.load(Relaxed).saturating_sub(base);
    (result, peak)
}

#[test]
fn oversized_project_and_snapshot_files_are_refused_before_being_read() {
    const FILE_BYTES: u64 = 64 * 1024 * 1024;
    let dir = std::env::temp_dir().join(format!("canary-audit-big-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let project = dir.join("big.json");
    let file = std::fs::File::create(&project).expect("create");
    file.set_len(FILE_BYTES).expect("sparse extend");
    drop(file);

    let (result, peak) = peak_while(|| AuthoredDocument::load(&project).map(|_| ()));
    assert!(
        matches!(result, Err(StateError::InputTooLarge { .. })),
        "oversized project must fail typed, got {result:?}"
    );
    assert!(
        peak < 8 * 1024 * 1024,
        "loading a {FILE_BYTES}-byte project file allocated {peak} bytes before refusing it"
    );

    let snapshot = dir.join("big.bin");
    let file = std::fs::File::create(&snapshot).expect("create");
    file.set_len(FILE_BYTES).expect("sparse extend");
    drop(file);

    let (result, peak) = peak_while(|| load_snapshot(&snapshot).map(|_| ()));
    assert!(
        matches!(result, Err(StateError::InputTooLarge { .. })),
        "oversized snapshot must fail typed, got {result:?}"
    );
    assert!(
        peak < 8 * 1024 * 1024,
        "loading a {FILE_BYTES}-byte snapshot file allocated {peak} bytes before refusing it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
