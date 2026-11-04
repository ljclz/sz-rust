// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::validator::http_comparator::HttpResponse;
use crate::validator::{HttpRequestSample, InterfaceValidationResult};

pub struct ResponseComparator;

impl ResponseComparator {
    pub fn compare(
        interface_id: String,
        request: HttpRequestSample,
        tp6_response: Option<&HttpResponse>,
        sz_rust_response: Option<&HttpResponse>,
    ) -> InterfaceValidationResult {
        match (tp6_response, sz_rust_response) {
            (Some(tp6), Some(sz)) => {
                let status_match = tp6.status == sz.status;
                let body_match = json_semantic_equal(&tp6.body, &sz.body);
                let is_consistent = status_match && body_match;
                let diff = if is_consistent {
                    None
                } else {
                    Some(format!(
                        "status: {} vs {}, body differs: {}",
                        tp6.status, sz.status, !body_match
                    ))
                };
                InterfaceValidationResult {
                    interface_id,
                    request,
                    tp6_status: Some(tp6.status),
                    sz_rust_status: Some(sz.status),
                    is_consistent,
                    diff,
                }
            }
            (Some(tp6), None) => InterfaceValidationResult {
                interface_id,
                request,
                tp6_status: Some(tp6.status),
                sz_rust_status: None,
                is_consistent: false,
                diff: Some("sz-rust service unreachable".into()),
            },
            (None, Some(sz)) => InterfaceValidationResult {
                interface_id,
                request,
                tp6_status: None,
                sz_rust_status: Some(sz.status),
                is_consistent: false,
                diff: Some("TP6 service unreachable".into()),
            },
            (None, None) => InterfaceValidationResult {
                interface_id,
                request,
                tp6_status: None,
                sz_rust_status: None,
                is_consistent: false,
                diff: Some("both services unreachable".into()),
            },
        }
    }
}

fn json_semantic_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Object(a_obj), serde_json::Value::Object(b_obj)) => {
            if a_obj.len() != b_obj.len() {
                return false;
            }
            a_obj
                .iter()
                .all(|(k, v)| b_obj.get(k).is_some_and(|bv| json_semantic_equal(v, bv)))
        }
        (serde_json::Value::Array(a_arr), serde_json::Value::Array(b_arr)) => {
            a_arr.len() == b_arr.len()
                && a_arr
                    .iter()
                    .zip(b_arr)
                    .all(|(a, b)| json_semantic_equal(a, b))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_responses() {
        let tp6 = HttpResponse {
            status: 200,
            body: serde_json::json!({"code": 0, "data": {"id": 1}}),
        };
        let sz = HttpResponse {
            status: 200,
            body: serde_json::json!({"code": 0, "data": {"id": 1}}),
        };
        let result = ResponseComparator::compare(
            "test".into(),
            HttpRequestSample::get("/test"),
            Some(&tp6),
            Some(&sz),
        );
        assert!(result.is_consistent);
    }

    #[test]
    fn different_status() {
        let tp6 = HttpResponse {
            status: 200,
            body: serde_json::Value::Null,
        };
        let sz = HttpResponse {
            status: 404,
            body: serde_json::Value::Null,
        };
        let result = ResponseComparator::compare(
            "test".into(),
            HttpRequestSample::get("/test"),
            Some(&tp6),
            Some(&sz),
        );
        assert!(!result.is_consistent);
    }

    #[test]
    fn json_key_order_independent() {
        let tp6 = HttpResponse {
            status: 200,
            body: serde_json::json!({"a": 1, "b": 2}),
        };
        let sz = HttpResponse {
            status: 200,
            body: serde_json::json!({"b": 2, "a": 1}),
        };
        let result = ResponseComparator::compare(
            "test".into(),
            HttpRequestSample::get("/test"),
            Some(&tp6),
            Some(&sz),
        );
        assert!(result.is_consistent);
    }
}
