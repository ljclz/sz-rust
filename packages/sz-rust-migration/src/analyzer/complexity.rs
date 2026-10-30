// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::analyzer::controller_parser::Tp6Controller;
use crate::analyzer::model_parser::Tp6Model;
use crate::analyzer::route_parser::Tp6Route;
use crate::phase::{ComplexityLevel, ComplexitySummary};

pub struct ComplexityAssessor;

impl ComplexityAssessor {
    pub fn assess(
        routes: &[Tp6Route],
        controllers: &[Tp6Controller],
        models: &[Tp6Model],
    ) -> ComplexitySummary {
        let mut summary = ComplexitySummary::default();
        for controller in controllers {
            let level = Self::assess_controller(controller, models);
            match level {
                ComplexityLevel::Low => summary.low += 1,
                ComplexityLevel::Medium => summary.medium += 1,
                ComplexityLevel::High => summary.high += 1,
            }
        }
        let _ = routes;
        summary
    }

    fn assess_controller(controller: &Tp6Controller, models: &[Tp6Model]) -> ComplexityLevel {
        let action_count = controller.actions.len();
        let has_model_dep = models
            .iter()
            .any(|m| m.class_name == controller.class_name.trim_end_matches("Controller"));
        let relation_count = models
            .iter()
            .filter(|m| m.class_name == controller.class_name.trim_end_matches("Controller"))
            .map(|m| m.relations.len())
            .sum::<usize>();

        let score = action_count + relation_count * 2 + if has_model_dep { 1 } else { 0 };
        if score <= 3 {
            ComplexityLevel::Low
        } else if score <= 8 {
            ComplexityLevel::Medium
        } else {
            ComplexityLevel::High
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_assessment() {
        let summary = ComplexityAssessor::assess(&[], &[], &[]);
        assert_eq!(summary.low, 0);
        assert_eq!(summary.medium, 0);
        assert_eq!(summary.high, 0);
    }

    #[test]
    fn simple_controller_low_complexity() {
        let controllers = vec![Tp6Controller {
            class_name: "SimpleController".into(),
            actions: vec![],
            middleware_applied: vec![],
            dependencies: vec![],
        }];
        let summary = ComplexityAssessor::assess(&[], &controllers, &[]);
        assert_eq!(summary.low, 1);
    }
}
