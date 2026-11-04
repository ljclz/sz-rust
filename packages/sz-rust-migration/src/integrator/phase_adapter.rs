// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use sz_rust_sdd::model::Stage;

use crate::phase::MigrationPhase;

pub fn migration_phase_to_sdd_stage(phase: MigrationPhase) -> Stage {
    match phase {
        MigrationPhase::Analysis => Stage::Spec,
        MigrationPhase::Generation => Stage::Coding,
        MigrationPhase::Validation => Stage::Coding,
        MigrationPhase::Reporting => Stage::Coding,
        MigrationPhase::CaseDoc => Stage::Coding,
    }
}

impl From<MigrationPhase> for Stage {
    fn from(phase: MigrationPhase) -> Self {
        migration_phase_to_sdd_stage(phase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_maps_to_spec() {
        assert_eq!(
            migration_phase_to_sdd_stage(MigrationPhase::Analysis),
            Stage::Spec
        );
    }

    #[test]
    fn generation_maps_to_coding() {
        assert_eq!(
            migration_phase_to_sdd_stage(MigrationPhase::Generation),
            Stage::Coding
        );
    }

    #[test]
    fn all_non_analysis_map_to_coding() {
        assert_eq!(Stage::from(MigrationPhase::Validation), Stage::Coding);
        assert_eq!(Stage::from(MigrationPhase::Reporting), Stage::Coding);
        assert_eq!(Stage::from(MigrationPhase::CaseDoc), Stage::Coding);
    }
}
