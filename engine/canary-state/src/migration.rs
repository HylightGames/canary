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

    /// Migrates canonical record fields from `from` to `to` through the
    /// JSON bridge: fields become JSON, the linear chain runs one step at
    /// a time (no skipping), and the result converts back and revalidates.
    /// The bridge is lossy only where JSON is: `Bytes` values cross as
    /// number arrays (see [`SnapshotValue::to_json`](crate::SnapshotValue::to_json)),
    /// so chains over byte-carrying schemas should avoid byte fields or
    /// restore them explicitly. A failed step or a non-canonical result
    /// fails with a typed error before any caller mutates world state.
    pub fn migrate_fields(
        &self,
        fields: &std::collections::BTreeMap<String, crate::SnapshotValue>,
        from: SchemaVersion,
        to: SchemaVersion,
    ) -> Result<std::collections::BTreeMap<String, crate::SnapshotValue>, StateError> {
        // Validate at entry: the JSON bridge cannot represent non-finite
        // floats (`serde_json` narrows them to `Null`), so an unvalidated
        // `F64(NaN)` input would silently become `Null` on the way in
        // instead of failing typed.
        fields
            .values()
            .try_for_each(crate::SnapshotValue::validate)?;
        let body = Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect(),
        );
        let migrated = self.migrate(body, from, to)?;
        let object = migrated
            .as_object()
            .ok_or_else(|| StateError::MigrationInvalid {
                schema: self.schema.as_str().to_owned(),
                to: to.0,
                reason: "migration must leave a JSON object of fields".to_owned(),
            })?;
        let out: std::collections::BTreeMap<String, crate::SnapshotValue> = object
            .iter()
            .map(|(key, value)| (key.clone(), crate::SnapshotValue::from_json(value)))
            .collect();
        out.values().try_for_each(crate::SnapshotValue::validate)?;
        Ok(out)
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

    fn two_step_chain() -> MigrationChain {
        let mut chain = MigrationChain::new(crate::schema::SchemaId::new("canary.widget"));
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
            .push(MigrationStep {
                from: 1,
                description: "split 'health' into 'health' plus 'max_health'",
                run: Box::new(|mut body: Value| {
                    if let Some(map) = body.as_object_mut() {
                        if let Some(health) = map.get("health").cloned() {
                            map.entry("max_health".to_owned())
                                .or_insert_with(|| health.clone());
                        }
                    }
                    Ok(body)
                }),
            })
            .expect("linear push");
        chain
    }

    #[test]
    fn linear_chain_applies_each_step_in_order_with_no_skipping() {
        let out = two_step_chain()
            .migrate(json!({"hp": 10}), SchemaVersion(0), SchemaVersion(2))
            .expect("migrate");
        assert_eq!(out, json!({"health": 10, "max_health": 10}));

        // Each intermediate shape stays representable: stop after step one.
        let mid = two_step_chain()
            .migrate(json!({"hp": 10}), SchemaVersion(0), SchemaVersion(1))
            .expect("migrate");
        assert_eq!(mid, json!({"health": 10}));

        // No skipping: v0 straight to v2 without the v1 step is refused.
        let mut gapped = MigrationChain::new(crate::schema::SchemaId::new("canary.widget"));
        gapped
            .push(MigrationStep {
                from: 1,
                description: "orphan step with no v0 predecessor",
                run: Box::new(Ok),
            })
            .expect_err("gapped push refused");
    }

    #[test]
    fn migrate_fields_bridges_canonical_values_through_the_chain() {
        use std::collections::BTreeMap;
        let fields = BTreeMap::from([("hp".to_owned(), crate::SnapshotValue::I64(10))]);
        let out = two_step_chain()
            .migrate_fields(&fields, SchemaVersion(0), SchemaVersion(2))
            .expect("migrate fields");
        assert_eq!(
            out.get("health"),
            Some(&crate::SnapshotValue::I64(10)),
            "renamed field survives the bridge"
        );
        assert_eq!(
            out.get("max_health"),
            Some(&crate::SnapshotValue::I64(10)),
            "derived field survives the bridge"
        );
        assert!(!out.contains_key("hp"), "old key is gone");
    }

    #[test]
    fn migrate_fields_rejects_a_non_object_result() {
        let mut chain = MigrationChain::new(crate::schema::SchemaId::new("canary.widget"));
        chain
            .push(MigrationStep {
                from: 0,
                description: "collapse the body to a scalar",
                run: Box::new(|_| Ok(json!(42))),
            })
            .expect("linear push");
        let fields =
            std::collections::BTreeMap::from([("hp".to_owned(), crate::SnapshotValue::I64(1))]);
        let err = chain
            .migrate_fields(&fields, SchemaVersion(0), SchemaVersion(1))
            .unwrap_err();
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
    }

    #[test]
    fn migrate_fields_rejects_non_finite_inputs_before_the_bridge() {
        // Without entry validation the JSON bridge would narrow `NaN` to
        // `Null` (`serde_json` has no NaN arm) and the migration would
        // succeed with silently wrong fields.
        let fields = std::collections::BTreeMap::from([(
            "x".to_owned(),
            crate::SnapshotValue::F64(f64::NAN),
        )]);
        let err = two_step_chain()
            .migrate_fields(&fields, SchemaVersion(0), SchemaVersion(2))
            .unwrap_err();
        assert!(
            matches!(err, StateError::MigrationInvalid { .. }),
            "non-finite input must fail typed, got {err:?}"
        );
    }
}
