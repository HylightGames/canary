// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::path::PathBuf;

use crate::error::CoreError;
use crate::subsystem::Subsystem;

/// The entry point every Canary program (game, headless server, editor, or
/// test harness) shares: owns the top-level lifecycle (init → tick →
/// shutdown) and a registry of [`Subsystem`]s.
///
/// See `docs/architecture/core-runtime.md#the-appengine-bootstrap`.
///
/// Two ways to drive the tick loop, added together in `v0.0.9` so neither
/// has to fake the other: [`App::run_for`] runs a fixed number of ticks at
/// a caller-specified, fixed `dt` — deterministic, for tests and
/// headless/CI use, where a real wall clock would make runs
/// non-reproducible. [`App::run`] runs for real, using actual elapsed
/// wall-clock time as `dt` each iteration, until a caller-supplied
/// condition says to stop — what an actual running game or server uses.
#[derive(Default)]
pub struct App {
    subsystems: Vec<Box<dyn Subsystem>>,
    plugin_dirs: Vec<PathBuf>,
}

impl App {
    /// Creates an empty `App` with no subsystems and no plugin directories
    /// registered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a subsystem. Subsystems are initialized in registration
    /// order and shut down in reverse.
    pub fn add_subsystem<S: Subsystem>(&mut self, subsystem: S) -> &mut Self {
        self.subsystems.push(Box::new(subsystem));
        self
    }

    /// Registers a directory that plugins may be loaded from.
    ///
    /// v0.0.1-pre1 only records this path; nothing walks it automatically
    /// yet (see `canary-plugin-api` and `docs/architecture/plugin-system.md`
    /// for the loader itself, and `docs/roadmap/v0.0.1-roadmap.md` for what
    /// wiring it into `App` automatically would require).
    pub fn add_plugin_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Self {
        self.plugin_dirs.push(dir.into());
        self
    }

    /// The plugin directories registered via [`App::add_plugin_dir`].
    pub fn plugin_dirs(&self) -> &[PathBuf] {
        &self.plugin_dirs
    }

    /// The number of subsystems currently registered.
    pub fn subsystem_count(&self) -> usize {
        self.subsystems.len()
    }

    /// Runs every subsystem's `init`, in registration order, stopping at
    /// the first error. Returns how many subsystems actually initialized
    /// (needed by both [`App::run_for`] and [`App::run`] to know how many
    /// to shut down afterward, even on failure) alongside the result.
    fn init_all(&mut self) -> (usize, Result<(), CoreError>) {
        let mut initialized = 0usize;
        let result = (|| {
            for subsystem in &mut self.subsystems {
                let name = subsystem.name().to_string();
                subsystem
                    .init()
                    .map_err(|source| CoreError::SubsystemInit { name, source })?;
                initialized += 1;
            }
            Ok(())
        })();
        (initialized, result)
    }

    /// Shuts down the first `initialized` subsystems, in **reverse**
    /// registration order — see [`Subsystem::shutdown`]'s own docs for why.
    fn shutdown_all(&mut self, initialized: usize) {
        for subsystem in self.subsystems[..initialized].iter_mut().rev() {
            subsystem.shutdown();
        }
    }

    /// Runs `init` on every subsystem (registration order), then `tick`
    /// on every subsystem, `ticks` times (registration order each time,
    /// each call passed the same fixed `dt`), then `shutdown` on every
    /// subsystem (**reverse** registration order).
    ///
    /// `dt` is caller-specified rather than measured from a real clock —
    /// deliberately, so a test or headless/CI run gets the exact same
    /// sequence of `tick(dt)` calls every time it runs, regardless of how
    /// fast the machine actually executing it happens to be. Use
    /// [`App::run`] for a real, wall-clock-timed loop.
    ///
    /// Returns the first `init` error encountered, if any; in that case,
    /// no subsystem's `tick` runs, but `shutdown` is still called on every
    /// subsystem that was already initialized, in reverse order, so
    /// resources they acquired during `init` aren't leaked.
    pub fn run_for(&mut self, ticks: u32, dt: std::time::Duration) -> Result<(), CoreError> {
        let (initialized, init_result) = self.init_all();

        if init_result.is_ok() {
            for _ in 0..ticks {
                for subsystem in &mut self.subsystems {
                    subsystem.tick(dt);
                }
            }
        }

        self.shutdown_all(initialized);
        init_result
    }

    /// Runs `init` on every subsystem, then ticks every subsystem
    /// (registration order) in a real loop — each iteration's `dt` is the
    /// actual wall-clock time elapsed since the previous iteration
    /// (measured with [`std::time::Instant`]), not a fixed value — for as
    /// long as `should_continue` keeps returning `true`, then `shutdown`
    /// on every subsystem (reverse registration order). This is what an
    /// actual running game or server uses; see [`App::run_for`] for the
    /// deterministic, fixed-`dt` alternative tests and headless/CI runs
    /// should prefer instead.
    ///
    /// `should_continue` is called once per iteration, *before* that
    /// iteration's tick — including once before the very first tick, so
    /// an already-closed window (say) never ticks at all. `App` has no
    /// built-in notion of a window or any other exit condition on
    /// purpose: it doesn't depend on `canary-platform`, so polling
    /// events and deciding when to stop is the closure's job (e.g.
    /// `app.run(|| { window.poll_events(); !window.close_requested() })`),
    /// not something `App` can know how to do generically.
    ///
    /// Returns the first `init` error encountered, if any, the same as
    /// [`App::run_for`].
    pub fn run(&mut self, mut should_continue: impl FnMut() -> bool) -> Result<(), CoreError> {
        let (initialized, init_result) = self.init_all();

        if init_result.is_ok() {
            let mut previous_tick = std::time::Instant::now();
            while should_continue() {
                let now = std::time::Instant::now();
                let dt = now.duration_since(previous_tick);
                previous_tick = now;
                for subsystem in &mut self.subsystems {
                    subsystem.tick(dt);
                }
            }
        }

        self.shutdown_all(initialized);
        init_result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingSubsystem {
        name: &'static str,
        log: Arc<Mutex<Vec<String>>>,
        fail_init: bool,
    }

    impl Subsystem for RecordingSubsystem {
        fn name(&self) -> &str {
            self.name
        }

        fn init(&mut self) -> Result<(), crate::SubsystemError> {
            if self.fail_init {
                return Err(format!("{} refused to initialize", self.name).into());
            }
            self.log.lock().unwrap().push(format!("{}:init", self.name));
            Ok(())
        }

        fn tick(&mut self, _dt: std::time::Duration) {
            self.log.lock().unwrap().push(format!("{}:tick", self.name));
        }

        fn shutdown(&mut self) {
            self.log
                .lock()
                .unwrap()
                .push(format!("{}:shutdown", self.name));
        }
    }

    #[test]
    fn runs_init_tick_shutdown_in_the_right_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_subsystem(RecordingSubsystem {
            name: "a",
            log: log.clone(),
            fail_init: false,
        });
        app.add_subsystem(RecordingSubsystem {
            name: "b",
            log: log.clone(),
            fail_init: false,
        });

        app.run_for(2, std::time::Duration::from_millis(16))
            .expect("both subsystems should initialize");

        let events = log.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                "a:init",
                "b:init",
                "a:tick",
                "b:tick",
                "a:tick",
                "b:tick",
                "b:shutdown",
                "a:shutdown",
            ]
        );
    }

    #[test]
    fn a_failed_init_still_shuts_down_what_already_started() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_subsystem(RecordingSubsystem {
            name: "a",
            log: log.clone(),
            fail_init: false,
        });
        app.add_subsystem(RecordingSubsystem {
            name: "b",
            log: log.clone(),
            fail_init: true,
        });

        let result = app.run_for(5, std::time::Duration::from_millis(16));

        assert!(result.is_err());
        let events = log.lock().unwrap().clone();
        // `a` initialized, `b` failed to — no ticks should have run, and
        // only `a` (the subsystem that actually started) should shut down.
        assert_eq!(events, vec!["a:init", "a:shutdown"]);
    }

    #[test]
    fn plugin_dirs_are_recorded_verbatim() {
        let mut app = App::new();
        app.add_plugin_dir("plugins");
        app.add_plugin_dir("more-plugins");
        assert_eq!(
            app.plugin_dirs(),
            &[PathBuf::from("plugins"), PathBuf::from("more-plugins")]
        );
    }

    #[derive(Default)]
    struct DtRecordingSubsystem {
        recorded: Arc<Mutex<Vec<std::time::Duration>>>,
    }

    impl Subsystem for DtRecordingSubsystem {
        fn name(&self) -> &str {
            "dt-recorder"
        }

        fn tick(&mut self, dt: std::time::Duration) {
            self.recorded.lock().unwrap().push(dt);
        }
    }

    #[test]
    fn run_for_passes_the_same_fixed_dt_to_every_tick() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_subsystem(DtRecordingSubsystem {
            recorded: recorded.clone(),
        });

        let fixed_dt = std::time::Duration::from_millis(16);
        app.run_for(4, fixed_dt).expect("should initialize");

        let dts = recorded.lock().unwrap().clone();
        assert_eq!(dts, vec![fixed_dt; 4]);
    }

    #[test]
    fn run_uses_real_elapsed_wall_clock_time_as_dt() {
        // A fixed dt (run_for) can never produce this by construction;
        // proving run() doesn't just fake it the same way means actually
        // sleeping between ticks and checking the recorded dt reflects
        // real elapsed time, not asserting the loop's intent from its
        // own source.
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_subsystem(DtRecordingSubsystem {
            recorded: recorded.clone(),
        });

        const SLEEP: std::time::Duration = std::time::Duration::from_millis(30);
        const ITERATIONS: usize = 3;
        let mut remaining = ITERATIONS;
        app.run(|| {
            if remaining == 0 {
                return false;
            }
            remaining -= 1;
            std::thread::sleep(SLEEP);
            true
        })
        .expect("should initialize");

        let dts = recorded.lock().unwrap().clone();
        assert_eq!(dts.len(), ITERATIONS);
        // The first tick's dt covers the time from App::run's own start
        // to the first should_continue call's sleep, so it's this sleep
        // or more; every dt is at least the sleep duration, since that's
        // real time that provably elapsed before each tick ran, and none
        // are wildly larger (which would mean something other than the
        // intended sleeps was being measured).
        for dt in dts {
            assert!(
                dt >= SLEEP,
                "dt {dt:?} should be at least the {SLEEP:?} sleep"
            );
            assert!(
                dt < SLEEP * 10,
                "dt {dt:?} is suspiciously larger than the {SLEEP:?} sleep"
            );
        }
    }

    #[test]
    fn run_checks_should_continue_before_the_first_tick() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_subsystem(DtRecordingSubsystem {
            recorded: recorded.clone(),
        });

        app.run(|| false).expect("should initialize");

        assert!(
            recorded.lock().unwrap().is_empty(),
            "should_continue returning false immediately must mean zero ticks"
        );
    }

    #[test]
    fn run_still_shuts_down_only_what_actually_initialized() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_subsystem(RecordingSubsystem {
            name: "a",
            log: log.clone(),
            fail_init: false,
        });
        app.add_subsystem(RecordingSubsystem {
            name: "b",
            log: log.clone(),
            fail_init: true,
        });

        let result = app.run(|| true);

        assert!(result.is_err());
        let events = log.lock().unwrap().clone();
        assert_eq!(events, vec!["a:init", "a:shutdown"]);
    }
}
