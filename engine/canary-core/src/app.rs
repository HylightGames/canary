use std::path::PathBuf;

use crate::error::CoreError;
use crate::subsystem::Subsystem;

/// The entry point every Canary program (game, headless server, editor, or
/// test harness) shares: owns the top-level lifecycle (init → tick →
/// shutdown) and a registry of [`Subsystem`]s.
///
/// See `docs/architecture/core-runtime.md#the-appengine-bootstrap`.
///
/// v0.0.1-pre1 note: there is no windowed main loop yet — [`App::run_for`]
/// runs a fixed number of ticks and returns, which suits a headless boot
/// harness (see `canary-runtime`) and tests. A real windowed/timed loop is
/// later work; see `docs/roadmap/v0.0.1-roadmap.md`.
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

    /// Runs `init` on every subsystem (registration order), then `tick` on
    /// every subsystem, `ticks` times (registration order each time), then
    /// `shutdown` on every subsystem (**reverse** registration order).
    ///
    /// Returns the first `init` error encountered, if any; in that case,
    /// no subsystem's `tick` runs, but `shutdown` is still called on every
    /// subsystem that was already initialized, in reverse order, so
    /// resources they acquired during `init` aren't leaked.
    pub fn run_for(&mut self, ticks: u32) -> Result<(), CoreError> {
        let mut initialized = 0usize;
        let init_result = (|| {
            for subsystem in &mut self.subsystems {
                let name = subsystem.name().to_string();
                subsystem
                    .init()
                    .map_err(|source| CoreError::SubsystemInit { name, source })?;
                initialized += 1;
            }
            Ok(())
        })();

        if init_result.is_ok() {
            for _ in 0..ticks {
                for subsystem in &mut self.subsystems {
                    subsystem.tick();
                }
            }
        }

        // Shut down whatever was successfully initialized, in reverse
        // order, regardless of whether init or the tick loop succeeded —
        // see the doc comment above.
        for subsystem in self.subsystems[..initialized].iter_mut().rev() {
            subsystem.shutdown();
        }

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

        fn tick(&mut self) {
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

        app.run_for(2).expect("both subsystems should initialize");

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

        let result = app.run_for(5);

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
}
