//! Slang 的值模型。
//!
//! 只承载模板/脚本能操作的数据：`null` / 布尔 / 数字 / 字符串 / 数组 / 对象。
//! 没有函数值、没有 ref/computed：Slang 2 的脚本是模块级纯函数，
//! 交互由浏览器里的 Alpine 负责，不需要把可调用对象塞进值模型。

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Default for Value {
    fn default() -> Self {
        Value::Null
    }
}

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        Value::from_json(&value)
    }
}

impl From<&serde_json::Value> for Value {
    fn from(value: &serde_json::Value) -> Self {
        Value::from_json(value)
    }
}

impl Value {
    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(value) => Value::Bool(*value),
            serde_json::Value::Number(number) => {
                if let Some(value) = number.as_i64() {
                    Value::Int(value)
                } else if let Some(value) = number.as_u64() {
                    if value <= i64::MAX as u64 {
                        Value::Int(value as i64)
                    } else {
                        Value::Float(value as f64)
                    }
                } else {
                    Value::Float(number.as_f64().unwrap_or(0.0))
                }
            }
            serde_json::Value::String(value) => Value::Str(value.clone()),
            serde_json::Value::Array(items) => {
                Value::List(items.iter().map(Value::from_json).collect())
            }
            serde_json::Value::Object(map) => Value::Map(
                map.iter()
                    .map(|(key, value)| (key.clone(), Value::from_json(value)))
                    .collect(),
            ),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "object",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Float(_))
    }

    /// 模板真值：false/null/0/""/[]/{} 为假；其余为真。
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(value) => *value,
            Value::Int(value) => *value != 0,
            Value::Float(value) => *value != 0.0,
            Value::Str(value) => !value.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::Map(map) => !map.is_empty(),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            Value::Float(value) if value.fract() == 0.0 => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(value) => Some(*value as f64),
            Value::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Map(map) => Some(map),
            _ => None,
        }
    }

    pub fn get_field(&self, name: &str) -> Value {
        match self {
            Value::Map(map) => match map.get(name) {
                Some(value) => value.clone(),
                None if name == "length" => Value::Int(map.len() as i64),
                None => Value::Null,
            },
            // `.length` 是模板里最常见的“伪字段”，直接映射。
            Value::Str(value) if name == "length" => Value::Int(value.chars().count() as i64),
            Value::List(items) if name == "length" => Value::Int(items.len() as i64),
            _ => Value::Null,
        }
    }

    pub fn get_index(&self, index: &Value) -> Value {
        match (self, index) {
            (Value::List(items), Value::Int(index)) => {
                if *index >= 0 {
                    items.get(*index as usize).cloned().unwrap_or(Value::Null)
                } else {
                    let index = items.len() as i64 + *index;
                    if index >= 0 {
                        items.get(index as usize).cloned().unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                }
            }
            (Value::Map(map), Value::Str(key)) => map.get(key).cloned().unwrap_or(Value::Null),
            (Value::Str(value), Value::Int(index)) => {
                let chars: Vec<char> = value.chars().collect();
                let position = if *index < 0 {
                    chars.len() as i64 + *index
                } else {
                    *index
                };
                if position >= 0 {
                    chars
                        .get(position as usize)
                        .map(|ch| Value::Str(ch.to_string()))
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }
            _ => Value::Null,
        }
    }

    /// 文本输出：标量原样；列表/对象用 JSON 紧凑形式（便于调试/展示）。
    pub fn to_text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) => {
                if value.fract() == 0.0 {
                    format!("{value:.0}")
                } else {
                    value.to_string()
                }
            }
            Value::Str(value) => value.clone(),
            Value::List(_) | Value::Map(_) => {
                serde_json::to_string(&self.to_json()).unwrap_or_default()
            }
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(value) => serde_json::Value::Bool(*value),
            Value::Int(value) => serde_json::Value::Number((*value).into()),
            Value::Float(value) => serde_json::Number::from_f64(*value)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Str(value) => serde_json::Value::String(value.clone()),
            Value::List(items) => {
                serde_json::Value::Array(items.iter().map(Value::to_json).collect())
            }
            Value::Map(map) => serde_json::Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
        }
    }

    /// 宽松字符串转换：只用于 `+` 拼接与 `string` 属性，不用来把数组变文本。
    pub fn into_text(self) -> String {
        self.to_text()
    }

    pub fn equals(&self, other: &Value) -> bool {
        self == other
    }

    pub fn compare(&self, other: &Value) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            (Value::Null, Value::Null) => Some(std::cmp::Ordering::Equal),
            _ => None,
        }
    }

    pub fn add(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a
                .checked_add(*b)
                .map(Value::Int)
                .ok_or_else(|| "整数相加溢出".to_string()),
            (a, b) if a.is_number() && b.is_number() => Ok(Value::Float(
                a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0),
            )),
            _ => Ok(Value::Str(format!("{}{}", self.to_text(), other.to_text()))),
        }
    }

    pub fn sub(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a
                .checked_sub(*b)
                .map(Value::Int)
                .ok_or_else(|| "整数相减溢出".to_string()),
            _ => numeric_op(self, other, |a, b| a - b),
        }
    }

    pub fn mul(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a
                .checked_mul(*b)
                .map(Value::Int)
                .ok_or_else(|| "整数相乘溢出".to_string()),
            _ => numeric_op(self, other, |a, b| a * b),
        }
    }

    pub fn div(&self, other: &Value) -> Result<Value, String> {
        if other.as_f64() == Some(0.0) {
            return Err("除数为 0".to_string());
        }
        numeric_op(self, other, |a, b| a / b)
    }

    pub fn rem(&self, other: &Value) -> Result<Value, String> {
        if other.as_f64() == Some(0.0) {
            return Err("取余的除数为 0".to_string());
        }
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
            _ => numeric_op(self, other, |a, b| a % b),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

fn numeric_op(left: &Value, right: &Value, op: impl Fn(f64, f64) -> f64) -> Result<Value, String> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => {
            let result = match op(*a as f64, *b as f64) {
                value if value.fract() == 0.0 && value.is_finite() => Value::Int(value as i64),
                value => Value::Float(value),
            };
            Ok(result)
        }
        (a, b) if a.is_number() && b.is_number() => Ok(Value::Float(op(
            a.as_f64().unwrap_or(0.0),
            b.as_f64().unwrap_or(0.0),
        ))),
        _ => Err(format!(
            "不能对 {} 和 {} 做算术运算",
            left.type_name(),
            right.type_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthiness_matches_template_conventions() {
        assert!(!Value::Null.is_truthy());
        assert!(!Value::Int(0).is_truthy());
        assert!(!Value::Str(String::new()).is_truthy());
        assert!(!Value::List(Vec::new()).is_truthy());
        assert!(Value::Str("0".to_string()).is_truthy());
    }

    #[test]
    fn field_access_on_missing_returns_null() {
        let value = Value::Map(BTreeMap::from([("a".to_string(), Value::Int(1))]));
        assert_eq!(value.get_field("a"), Value::Int(1));
        assert_eq!(value.get_field("missing"), Value::Null);
        assert_eq!(Value::Null.get_field("a"), Value::Null);
    }

    #[test]
    fn json_round_trip_preserves_scalars() {
        let json = serde_json::json!({"a": 1, "b": [true, null, "x"]});
        let value = Value::from_json(&json);
        assert_eq!(value.to_json(), json);
    }

    #[test]
    fn length_is_a_virtual_field() {
        assert_eq!(
            Value::List(vec![Value::Int(1), Value::Int(2)]).get_field("length"),
            Value::Int(2)
        );
        assert_eq!(
            Value::Str("中文".to_string()).get_field("length"),
            Value::Int(2)
        );
    }
}
