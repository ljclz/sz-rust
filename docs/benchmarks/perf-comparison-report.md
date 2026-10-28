# 性能基线对比报告

> 基线: `pre-optimization` | 当前: `new`
> 来源: `F:\cargo-target\criterion/`

## 延迟对比（P50/P99）

| 基准组 | 基线 P50 (ns) | 当前 P50 (ns) | 变化 (%) |
|--------|--------------|--------------|---------|
| cache_read_write/cache_get_hit | 57 | 55 | -4.1% |
| cache_read_write/cache_get_miss | 14 | 14 | -2.3% |
| cache_read_write/cache_remove_existing | 112 | 99 | -11.5% |
| cache_read_write/cache_set_new_key | 305 | 280 | -8.1% |
| capitalize_first/capitalize_first_already_upper | 2 | 2 | -22.9% |
| capitalize_first/capitalize_first_needs_upper | 45 | 40 | -11.5% |
| capitalize_first/capitalize_first_needs_upper_24_bytes | 48 | 41 | -15.1% |
| capitalize_first/capitalize_first_needs_upper_25_bytes | 43 | 42 | -3.0% |
| circuit_breaker/record_success | 71246 | 69981 | -1.8% |
| circuit_breaker/state_query_closed | 37 | 35 | -4.7% |
| container_lookup/bind_and_make_transient | 118 | 119 | +1.2% |
| container_lookup/make_missing | 18 | 18 | +2.9% |
| container_lookup/scoped_make | 42 | 39 | -5.8% |
| container_lookup/singleton_reuse | 28 | 26 | -5.1% |
| framework_vs_native/native_match_static | 55 | 54 | -1.8% |
| framework_vs_native/native_match_with_query | 72 | 72 | -0.1% |
| framework_vs_native/parse_path_long_framework | 59 | 60 | +0.8% |
| framework_vs_native/parse_path_static_framework | 60 | 61 | +2.0% |
| handler_ref_parse/parse_simple | 97 | 94 | -2.1% |
| handler_ref_parse/parse_with_slash | 101 | 99 | -1.9% |
| json_dto/deserialize_medium_dto | 5409 | 5329 | -1.5% |
| json_serialization/deserialize_medium | 51625 | 44484 | -13.8% |
| json_serialization/deserialize_small | 940 | 624 | -33.6% |
| json_serialization/serialize_medium | 4970 | 4761 | -4.2% |
| json_serialization/serialize_small | 106 | 98 | -7.3% |
| middleware_chain/contains_auth | 3 | 2 | -2.0% |
| middleware_chain/default_chain | 36 | 32 | -9.7% |
| middleware_chain/has_duplicates | 27 | 23 | -13.8% |
| middleware_chain/push_5 | 62 | 57 | -7.8% |
| middleware_chain/remove_from_auth | 35 | 33 | -3.6% |
| middleware_chain/service_builder_order | 34 | 35 | +1.6% |
| orm_query_build/orm_entity_get_attribute_all_fields | 39 | 37 | -4.8% |
| orm_query_build/orm_paginate_by_page10_size100 | 5667 | 5600 | -1.2% |
| orm_query_build/orm_paginate_by_page1_size20 | 7571 | 7524 | -0.6% |
| orm_query_build/orm_where_condition_build_1 | 39 | 37 | -6.7% |
| orm_query_build/orm_where_condition_build_20 | 803 | 778 | -3.2% |
| orm_query_build/orm_where_condition_build_5 | 210 | 210 | +0.1% |
| point_lookup/btree | — | 10884619 | — |
| point_lookup/lsm | — | 13142019 | — |
| range_scan/btree | — | 79161 | — |
| range_scan/lsm | — | 9428606 | — |
| rate_limiting/sliding_window_acquire | 485 | 474 | -2.3% |
| rate_limiting/token_bucket_acquire | 110 | 109 | -1.0% |
| route_config/find_conflicts | 28019 | 21700 | -22.6% |
| route_config/load_yaml_medium | 36152 | 25390 | -29.8% |
| route_config/load_yaml_small | 6888 | 6874 | -0.2% |
| route_matching/parse_path_long | 59 | 60 | +2.2% |
| route_matching/parse_path_root | 3 | 3 | -2.3% |
| route_matching/parse_path_static | 60 | 61 | +0.5% |
| write_throughput/btree | — | 8988038100 | — |
| write_throughput/lsm | — | 9716187 | — |

## 来源标注
- 基线数据: `F:\cargo-target\criterion/<bench>/<sub>/pre-optimization/estimates.json`
- 当前数据: `F:\cargo-target\criterion/<bench>/<sub>/new/estimates.json`