//! MySQL 结果集编码 — Column Definition + Row Data + EOF/OK。

use crate::packet::{write_lenenc_int, write_lenenc_string};
use crate::types::MysqlType;
use szrsql_types::value::Value;

/// 列定义（Column Definition 41）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDefinition {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub org_table: String,
    pub name: String,
    pub org_name: String,
    pub character_set: u16,
    pub column_length: u32,
    pub column_type: MysqlType,
    pub flags: u16,
    pub decimals: u8,
    pub filler: [u8; 2],
}

impl ColumnDefinition {
    pub fn new(name: impl Into<String>, column_type: MysqlType) -> Self {
        let name = name.into();
        Self {
            catalog: "def".to_string(),
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            org_name: name.clone(),
            name,
            character_set: 33,
            column_length: 255,
            column_type,
            flags: column_type.binary_flag(),
            decimals: 0,
            filler: [0x0C, 0x00],
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        write_lenenc_string(&mut buf, self.catalog.as_bytes());
        write_lenenc_string(&mut buf, self.schema.as_bytes());
        write_lenenc_string(&mut buf, self.table.as_bytes());
        write_lenenc_string(&mut buf, self.org_table.as_bytes());
        write_lenenc_string(&mut buf, self.name.as_bytes());
        write_lenenc_string(&mut buf, self.org_name.as_bytes());
        write_lenenc_int(&mut buf, 0x0C as u64);
        buf.extend_from_slice(&self.character_set.to_le_bytes());
        buf.extend_from_slice(&self.column_length.to_le_bytes());
        buf.push(self.column_type as u8);
        buf.extend_from_slice(&self.flags.to_le_bytes());
        buf.push(self.decimals);
        buf.extend_from_slice(&self.filler);
        buf
    }
}

/// 结果集编码器。
pub struct ResultSetEncoder;

impl ResultSetEncoder {
    pub fn encode_column_count(count: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(9);
        write_lenenc_int(&mut buf, count as u64);
        buf
    }

    pub fn encode_eof(warnings: u16, status_flags: u16) -> Vec<u8> {
        let mut buf = Vec::with_capacity(5);
        buf.push(0xFE);
        buf.extend_from_slice(&warnings.to_le_bytes());
        buf.extend_from_slice(&status_flags.to_le_bytes());
        buf
    }

    pub fn encode_value_text(value: &Value) -> Vec<u8> {
        match value {
            Value::Null => Vec::new(),
            Value::Bool(b) => {
                if *b {
                    b"1".to_vec()
                } else {
                    b"0".to_vec()
                }
            }
            Value::Int64(n) => n.to_string().into_bytes(),
            Value::Float64(f) => {
                if f.is_nan() || f.is_infinite() {
                    b"NULL".to_vec()
                } else if f.fract() == 0.0 {
                    format!("{:.1}", f).into_bytes()
                } else {
                    f.to_string().into_bytes()
                }
            }
            Value::Text(s) => s.as_bytes().to_vec(),
            Value::Blob(b) => b.clone(),
            Value::Date(days) => {
                let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                    .unwrap()
                    .checked_add_signed(chrono::Duration::days(*days as i64))
                    .unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
                date.format("%Y-%m-%d").to_string().into_bytes()
            }
            Value::Timestamp(micros) => {
                let secs = micros / 1_000_000;
                let nano = (micros % 1_000_000) * 1000;
                let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nano as u32)
                    .unwrap_or_else(|| {
                        chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap()
                    });
                dt.format("%Y-%m-%d %H:%M:%S").to_string().into_bytes()
            }
            Value::Decimal(unscaled, scale) => format_decimal(*unscaled, *scale).into_bytes(),
            Value::Json(v) => serde_json::to_string(v).unwrap_or_default().into_bytes(),
            Value::Array(arr) => {
                let json_arr: Vec<&Value> = arr.iter().collect();
                serde_json::to_string(&json_arr)
                    .unwrap_or_default()
                    .into_bytes()
            }
            Value::Enum(s) => s.as_bytes().to_vec(),
            Value::Range(_) => b"".to_vec(),
            Value::TsVector(tv) => tv.to_pg_string().into_bytes(),
            Value::TsQuery(_) => b"".to_vec(),
        }
    }

    pub fn encode_row(values: &[Value]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        for value in values {
            match value {
                Value::Null => {
                    buf.push(0xFB);
                }
                _ => {
                    let encoded = Self::encode_value_text(value);
                    write_lenenc_string(&mut buf, &encoded);
                }
            }
        }
        buf
    }

    pub fn encode_result_set(
        columns: &[ColumnDefinition],
        rows: &[Vec<Value>],
    ) -> Vec<Vec<u8>> {
        Self::encode_result_set_with_flags(
            columns,
            rows,
            crate::handshake::SERVER_STATUS_AUTOCOMMIT,
        )
    }

    /// 编码结果集，使用指定的 status_flags（用于多语句查询设置 SERVER_MORE_RESULTS_EXISTS）。
    pub fn encode_result_set_with_flags(
        columns: &[ColumnDefinition],
        rows: &[Vec<Value>],
        status_flags: u16,
    ) -> Vec<Vec<u8>> {
        let mut packets = Vec::with_capacity(2 + columns.len() + rows.len() + 1);
        packets.push(Self::encode_column_count(columns.len()));
        for col in columns {
            packets.push(col.encode());
        }
        packets.push(Self::encode_eof(0, status_flags));
        for row in rows {
            packets.push(Self::encode_row(row));
        }
        packets.push(Self::encode_eof(0, status_flags));
        packets
    }
}

fn format_decimal(unscaled: i128, scale: u8) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let scale = scale as u32;
    let abs = unscaled.unsigned_abs();
    let abs_str = abs.to_string();
    let int_part_len = abs_str.len().saturating_sub(scale as usize);
    let int_part = &abs_str[..int_part_len];
    let frac_part = &abs_str[int_part_len..];

    let mut result = String::new();
    if unscaled < 0 {
        result.push('-');
    }
    if int_part.is_empty() {
        result.push('0');
    } else {
        result.push_str(int_part);
    }
    result.push('.');
    let frac_padded = format!("{:0>width$}", frac_part, width = scale as usize);
    result.push_str(&frac_padded);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_definition_new() {
        let col = ColumnDefinition::new("id", MysqlType::Long);
        assert_eq!(col.name, "id");
        assert_eq!(col.column_type, MysqlType::Long);
        assert_eq!(col.catalog, "def");
    }

    #[test]
    fn test_encode_column_count_small() {
        assert_eq!(ResultSetEncoder::encode_column_count(3), vec![3]);
    }

    #[test]
    fn test_encode_column_count_large() {
        let encoded = ResultSetEncoder::encode_column_count(1000);
        assert_eq!(encoded[0], 0xFC);
    }

    #[test]
    fn test_encode_eof() {
        let encoded = ResultSetEncoder::encode_eof(5, 0x0002);
        assert_eq!(encoded[0], 0xFE);
    }

    #[test]
    fn test_encode_value_null() {
        assert!(ResultSetEncoder::encode_value_text(&Value::Null).is_empty());
    }

    #[test]
    fn test_encode_value_int64() {
        assert_eq!(
            ResultSetEncoder::encode_value_text(&Value::Int64(42)),
            b"42"
        );
    }

    #[test]
    fn test_encode_value_float64() {
        assert_eq!(
            ResultSetEncoder::encode_value_text(&Value::Float64(3.5)),
            b"3.5"
        );
    }

    #[test]
    fn test_encode_value_text_string() {
        assert_eq!(
            ResultSetEncoder::encode_value_text(&Value::Text("hello".to_string())),
            b"hello"
        );
    }

    #[test]
    fn test_encode_value_bool() {
        assert_eq!(
            ResultSetEncoder::encode_value_text(&Value::Bool(true)),
            b"1"
        );
    }

    #[test]
    fn test_encode_value_blob() {
        assert_eq!(
            ResultSetEncoder::encode_value_text(&Value::Blob(vec![1, 2, 3])),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn test_encode_value_nan() {
        assert_eq!(
            ResultSetEncoder::encode_value_text(&Value::Float64(f64::NAN)),
            b"NULL"
        );
    }

    #[test]
    fn test_encode_row_with_null() {
        let row = vec![Value::Null, Value::Int64(42)];
        let encoded = ResultSetEncoder::encode_row(&row);
        assert_eq!(encoded[0], 0xFB);
    }

    #[test]
    fn test_encode_row_all_nulls() {
        let row = vec![Value::Null, Value::Null, Value::Null];
        assert_eq!(ResultSetEncoder::encode_row(&row), vec![0xFB, 0xFB, 0xFB]);
    }

    #[test]
    fn test_encode_result_set_structure() {
        let columns = vec![
            ColumnDefinition::new("id", MysqlType::Long),
            ColumnDefinition::new("name", MysqlType::VarString),
        ];
        let rows = vec![
            vec![Value::Int64(1), Value::Text("Alice".to_string())],
            vec![Value::Int64(2), Value::Text("Bob".to_string())],
        ];
        let packets = ResultSetEncoder::encode_result_set(&columns, &rows);
        assert_eq!(packets.len(), 7);
        assert_eq!(packets[0], vec![2]);
        assert_eq!(packets[3][0], 0xFE);
        assert_eq!(packets[6][0], 0xFE);
    }

    #[test]
    fn test_format_decimal_zero_scale() {
        assert_eq!(format_decimal(12345, 0), "12345");
    }

    #[test]
    fn test_format_decimal_with_scale() {
        assert_eq!(format_decimal(12345, 2), "123.45");
        assert_eq!(format_decimal(-12345, 2), "-123.45");
    }

    #[test]
    fn test_format_decimal_small_value() {
        assert_eq!(format_decimal(5, 2), "0.05");
    }

    #[test]
    fn test_format_decimal_zero() {
        assert_eq!(format_decimal(0, 2), "0.00");
    }

    #[test]
    fn test_encode_value_date_epoch() {
        assert_eq!(
            ResultSetEncoder::encode_value_text(&Value::Date(0)),
            b"1970-01-01"
        );
    }

    #[test]
    fn test_encode_value_date_known() {
        let encoded = ResultSetEncoder::encode_value_text(&Value::Date(19723));
        assert_eq!(String::from_utf8(encoded).unwrap(), "2024-01-01");
    }
}
