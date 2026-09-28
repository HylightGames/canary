//! Linear per-schema migration chains.
//!
//! A [`MigrationChain`] owns one [`SchemaId`](crate::schema::SchemaId) and a
//! chain of version-to-version steps. Payloads advance one step at a time —
//! skipping versions is never allowed, so every intermediate shape stays
//! representable and testable. Each step is a pure function over the
//! version-tagged unknown body (`serde_json::Value`); it returns the body
//! for the next version or a typed error. After the final step the caller
//! validates the result against the target shape and reports
//! [`StateError::MigrationInvalid`](crate::StateError) on failure.

use serde_json::Value;

use crate::error::StateError;
use crate::schema::{SchemaId, SchemaVersion};

/// One version step: `version` → `version + 1` for the chain's schema.
pub struct MigrationStep {
    /// The version this step migrates from.
    pub from: u32,
    /// Human-readable account of what the step changes.
    pub description: &'static str,
    /// The pure transform itself.
    pub run: Box<dyn Fn(Value) -> Result<Value, StateError> + Send + Sync>,
}

/// Linear chain of steps for one schema.
pub struct MigrationChain {
    schema: SchemaId,
    steps: Vec<MigrationStep>,
}

/// Why a chain refused to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    /// No step leaves `from`: the chain cannot start or continue.
    NoStep {
        /// The version with no outgoing step.
        from: u32,
    },
    /// A step failed mid-chain.
    StepFailed {
        /// The version the failing step left.
        from: u32,
        /// Why it failed.
        reason: String,
    },
}

impl MigrationChain {
    /// Starts an empty chain for `schema`.
    pub fn new(schema: SchemaId) -> Self {
        Self {
            schema,
            steps: Vec::new(),
        }
    }

    /// Appends a step. Steps must arrive in ascending `from` order without
    /// gaps; violations are refused so chains stay linear by construction.
    pub fn push(&mut self, step: MigrationStep) -> Result<(), MigrationError> {
        let expected = self.steps.last().map_or(0, |last| last.from + 1);
        if step.from != expected {
            return Err(MigrationError::NoStep { from: step.from });
        }
        self.steps.push(step);
        Ok(())
    }

    /// The schema this chain migrates.
    #[must_use]
    pub fn schema(&self) -> &SchemaId {
        &self.schema
    }

    /// Migrates `body` from `from` to `to`, one step at a time.
    pub fn migrate(
        &self,
        body: Value,
        from: SchemaVersion,
        to: SchemaVersion,
    ) -> Result<Value, StateError> {
        if from.0 > to.0 {
            return Err(StateError::NoMigrationPath {
                schema: self.schema.as_str().to_owned(),
                from: from.0,
                to: to.0,
            });
        }
        let mut current = body;
        let mut version = from.0;
        while version < to.0 {
            let step = self
                .steps
                .iter()
                .find(|s| s.from == version)
                .ok_or_else(|| StateError::NoMigrationPath {
                    schema: self.schema.as_str().to_owned(),
                    from: version,
                    to: to.0,
                })?;
            current = (step.run)(current)?;
            version += 1;
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rename_chain() -> MigrationChain {
        let mut chain = MigrationChain::new(crate::schema::SchemaId::new("canary.test"));
        chain
            .push(MigrationStep {
                from: 0,
                description: "rename 'hp' to 'health'",
                run: Box::new(|mut body: Value| {
                    if let Some(hp) = body.get("hp").cloned() {
                        if let Some(map) = body.as_object_mut() {
                            map.remove("hp");
                            map.insert("health".to_owned(), hp);
                        }
                    }
                    Ok(body)
                }),
            })
            .expect("linear push");
        chain
    }

    #[test]
    fn migration_renames_and_advances() {
        let out = rename_chain()
            .migrate(json!({"hp": 10}), SchemaVersion(0), SchemaVersion(1))
            .expect("migrate");
        assert_eq!(out, json!({"health": 10}));
    }

    #[test]
    fn missing_step_is_a_typed_error() {
        let err = rename_chain()
            .migrate(json!({}), SchemaVersion(1), SchemaVersion(2))
            .unwrap_err();
        assert!(matches!(
            err,
            StateError::NoMigrationPath { from: 1, to: 2, .. }
        ));
    }

    #[test]
    fn backward_migration_is_refused() {
        let err = rename_chain()
            .migrate(json!({}), SchemaVersion(2), SchemaVersion(1))
            .unwrap_err();
        assert!(matches!(err, StateError::NoMigrationPath { .. }));
    }
}
