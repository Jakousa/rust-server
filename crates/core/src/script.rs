//! Genre-agnostic sandboxed script execution abstractions and fuel metering interfaces.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Configuration for script execution fuel and execution time budgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptFuelConfig {
    pub default_fuel: u64,
    pub max_fuel: u64,
    pub max_execution_time_micros: u64,
}

impl Default for ScriptFuelConfig {
    fn default() -> Self {
        ScriptFuelConfig {
            default_fuel: 50_000,
            max_fuel: 500_000,
            max_execution_time_micros: 500,
        }
    }
}

/// Errors that can occur during sandboxed script compilation, linking, or execution.
#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("Out of fuel")]
    OutOfFuel,
    #[error("Trap: {0}")]
    Trap(String),
    #[error("Module not found: {0}")]
    ModuleNotFound(String),
    #[error("Export not found: {0}")]
    ExportNotFound(String),
    #[error("Compilation error: {0}")]
    CompilationError(String),
    #[error("Memory out of bounds: requested {requested}, size {size}")]
    MemoryOutOfBounds { requested: usize, size: usize },
    #[error("Execution error: {0}")]
    ExecutionError(String),
    #[error("IO error: {0}")]
    Io(String),
}

/// Represents a value that can be passed to or returned from a script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScriptValue {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Bytes(Vec<u8>),
    String(String),
}

impl ScriptValue {
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            ScriptValue::I32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ScriptValue::I64(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            ScriptValue::F32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ScriptValue::F64(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            ScriptValue::String(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            ScriptValue::Bytes(v) => Some(v),
            _ => None,
        }
    }
}

impl From<i32> for ScriptValue {
    fn from(v: i32) -> Self {
        ScriptValue::I32(v)
    }
}

impl From<i64> for ScriptValue {
    fn from(v: i64) -> Self {
        ScriptValue::I64(v)
    }
}

impl From<f32> for ScriptValue {
    fn from(v: f32) -> Self {
        ScriptValue::F32(v)
    }
}

impl From<f64> for ScriptValue {
    fn from(v: f64) -> Self {
        ScriptValue::F64(v)
    }
}

impl From<Vec<u8>> for ScriptValue {
    fn from(v: Vec<u8>) -> Self {
        ScriptValue::Bytes(v)
    }
}

impl From<String> for ScriptValue {
    fn from(v: String) -> Self {
        ScriptValue::String(v)
    }
}

impl From<&str> for ScriptValue {
    fn from(v: &str) -> Self {
        ScriptValue::String(v.to_string())
    }
}

/// A trait for executing scripts with fuel metering.
pub trait FuelMeteredExecutor: Send + Sync {
    fn execute(
        &self,
        module_id: &str,
        function_name: &str,
        args: &[ScriptValue],
        fuel: u64,
    ) -> Result<(Vec<ScriptValue>, u64), ScriptError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_fuel_config_default() {
        let config = ScriptFuelConfig::default();
        assert_eq!(config.default_fuel, 50_000);
        assert_eq!(config.max_fuel, 500_000);
        assert_eq!(config.max_execution_time_micros, 500);
    }

    #[test]
    fn test_script_value_conversions() {
        let i32_val: ScriptValue = 42.into();
        assert_eq!(i32_val.as_i32(), Some(42));
        assert_eq!(i32_val.as_i64(), None);

        let i64_val: ScriptValue = 1234567890i64.into();
        assert_eq!(i64_val.as_i64(), Some(1234567890));
        assert_eq!(i32_val.as_i64(), None);

        let f32_val: ScriptValue = 1.25f32.into();
        assert_eq!(f32_val.as_f32(), Some(1.25));
        assert_eq!(f32_val.as_f64(), None);

        let f64_val: ScriptValue = 2.5f64.into();
        assert_eq!(f64_val.as_f64(), Some(2.5));
        assert_eq!(f64_val.as_f32(), None);

        let string_val: ScriptValue = "hello".into();
        assert_eq!(string_val.as_str(), Some("hello"));
        assert_eq!(string_val.as_bytes(), None);

        let str_val: ScriptValue = "world".to_string().into();
        assert_eq!(str_val.as_str(), Some("world"));

        let bytes_val: ScriptValue = vec![1, 2, 3].into();
        assert_eq!(bytes_val.as_bytes(), Some(&[1, 2, 3][..]));
        assert_eq!(bytes_val.as_str(), None);
    }

    #[test]
    fn test_script_error_formatting() {
        assert_eq!(format!("{}", ScriptError::OutOfFuel), "Out of fuel");
        assert_eq!(
            format!("{}", ScriptError::Trap("trap".to_string())),
            "Trap: trap"
        );
        assert_eq!(
            format!("{}", ScriptError::ModuleNotFound("module".to_string())),
            "Module not found: module"
        );
        assert_eq!(
            format!("{}", ScriptError::ExportNotFound("export".to_string())),
            "Export not found: export"
        );
        assert_eq!(
            format!("{}", ScriptError::CompilationError("error".to_string())),
            "Compilation error: error"
        );
        assert_eq!(
            format!(
                "{}",
                ScriptError::MemoryOutOfBounds {
                    requested: 10,
                    size: 5
                }
            ),
            "Memory out of bounds: requested 10, size 5"
        );
        assert_eq!(
            format!("{}", ScriptError::ExecutionError("error".to_string())),
            "Execution error: error"
        );
        assert_eq!(
            format!("{}", ScriptError::Io("io error".to_string())),
            "IO error: io error"
        );
    }
}
