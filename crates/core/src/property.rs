//! Generic Dynamic Property and Document Value Primitives.
//!
//! Provides genre-agnostic, schemaless key-value storage supporting scalar,
//! nested object, and array values with safe dotted-path navigation and
//! bidirectional `serde_json::Value` conversions.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Dynamic property value variant supporting all common serialized scalar and compound data types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropertyValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<PropertyValue>),
    Object(HashMap<String, PropertyValue>),
}

impl PropertyValue {
    /// Returns `true` if the value is `PropertyValue::Null`.
    pub fn is_null(&self) -> bool {
        matches!(self, PropertyValue::Null)
    }

    /// Returns the boolean value if this is a `PropertyValue::Bool`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PropertyValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the integer value if this is a `PropertyValue::Int`, or if it is a whole float.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            PropertyValue::Int(i) => Some(*i),
            PropertyValue::Float(f)
                if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 =>
            {
                Some(*f as i64)
            }
            _ => None,
        }
    }

    /// Returns the float value if this is a `PropertyValue::Float`, or converts from `PropertyValue::Int`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PropertyValue::Float(f) => Some(*f),
            PropertyValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Returns the string slice if this is a `PropertyValue::String`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PropertyValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns the array slice if this is a `PropertyValue::Array`.
    pub fn as_array(&self) -> Option<&[PropertyValue]> {
        match self {
            PropertyValue::Array(arr) => Some(arr.as_slice()),
            _ => None,
        }
    }

    /// Returns a reference to the nested object map if this is a `PropertyValue::Object`.
    pub fn as_object(&self) -> Option<&HashMap<String, PropertyValue>> {
        match self {
            PropertyValue::Object(map) => Some(map),
            _ => None,
        }
    }
}

impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertyValue::Null => write!(f, "null"),
            PropertyValue::Bool(b) => write!(f, "{}", b),
            PropertyValue::Int(i) => write!(f, "{}", i),
            PropertyValue::Float(v) => write!(f, "{}", v),
            PropertyValue::String(s) => write!(f, "\"{}\"", s),
            PropertyValue::Array(arr) => write!(f, "[{} elements]", arr.len()),
            PropertyValue::Object(obj) => write!(f, "{{{} keys}}", obj.len()),
        }
    }
}

// Convenient conversion implementations for PropertyValue
impl From<bool> for PropertyValue {
    fn from(b: bool) -> Self {
        PropertyValue::Bool(b)
    }
}

impl From<i64> for PropertyValue {
    fn from(i: i64) -> Self {
        PropertyValue::Int(i)
    }
}

impl From<i32> for PropertyValue {
    fn from(i: i32) -> Self {
        PropertyValue::Int(i as i64)
    }
}

impl From<i16> for PropertyValue {
    fn from(i: i16) -> Self {
        PropertyValue::Int(i as i64)
    }
}

impl From<i8> for PropertyValue {
    fn from(i: i8) -> Self {
        PropertyValue::Int(i as i64)
    }
}

impl From<u32> for PropertyValue {
    fn from(u: u32) -> Self {
        PropertyValue::Int(u as i64)
    }
}

impl From<u16> for PropertyValue {
    fn from(u: u16) -> Self {
        PropertyValue::Int(u as i64)
    }
}

impl From<u8> for PropertyValue {
    fn from(u: u8) -> Self {
        PropertyValue::Int(u as i64)
    }
}

impl From<f64> for PropertyValue {
    fn from(f: f64) -> Self {
        PropertyValue::Float(f)
    }
}

impl From<f32> for PropertyValue {
    fn from(f: f32) -> Self {
        PropertyValue::Float(f as f64)
    }
}

impl From<String> for PropertyValue {
    fn from(s: String) -> Self {
        PropertyValue::String(s)
    }
}

impl From<&str> for PropertyValue {
    fn from(s: &str) -> Self {
        PropertyValue::String(s.to_string())
    }
}

impl From<Vec<PropertyValue>> for PropertyValue {
    fn from(arr: Vec<PropertyValue>) -> Self {
        PropertyValue::Array(arr)
    }
}

impl From<HashMap<String, PropertyValue>> for PropertyValue {
    fn from(obj: HashMap<String, PropertyValue>) -> Self {
        PropertyValue::Object(obj)
    }
}

impl From<serde_json::Value> for PropertyValue {
    fn from(val: serde_json::Value) -> Self {
        match val {
            serde_json::Value::Null => PropertyValue::Null,
            serde_json::Value::Bool(b) => PropertyValue::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    PropertyValue::Int(i)
                } else if let Some(u) = n.as_u64() {
                    PropertyValue::Int(u as i64)
                } else if let Some(f) = n.as_f64() {
                    PropertyValue::Float(f)
                } else {
                    PropertyValue::Null
                }
            }
            serde_json::Value::String(s) => PropertyValue::String(s),
            serde_json::Value::Array(arr) => {
                PropertyValue::Array(arr.into_iter().map(PropertyValue::from).collect())
            }
            serde_json::Value::Object(map) => {
                let mut obj = HashMap::with_capacity(map.len());
                for (k, v) in map {
                    obj.insert(k, PropertyValue::from(v));
                }
                PropertyValue::Object(obj)
            }
        }
    }
}

impl From<PropertyValue> for serde_json::Value {
    fn from(prop: PropertyValue) -> Self {
        match prop {
            PropertyValue::Null => serde_json::Value::Null,
            PropertyValue::Bool(b) => serde_json::Value::Bool(b),
            PropertyValue::Int(i) => serde_json::Value::Number(i.into()),
            PropertyValue::Float(f) => serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            PropertyValue::String(s) => serde_json::Value::String(s),
            PropertyValue::Array(arr) => {
                serde_json::Value::Array(arr.into_iter().map(serde_json::Value::from).collect())
            }
            PropertyValue::Object(map) => {
                let mut obj = serde_json::Map::with_capacity(map.len());
                for (k, v) in map {
                    obj.insert(k, serde_json::Value::from(v));
                }
                serde_json::Value::Object(obj)
            }
        }
    }
}

/// Generic container of dynamic, schemaless key-value properties.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DynamicProperties {
    fields: HashMap<String, PropertyValue>,
}

impl DynamicProperties {
    /// Creates a new empty `DynamicProperties`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new `DynamicProperties` with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            fields: HashMap::with_capacity(capacity),
        }
    }

    /// Retrieves an immutable reference to the property value associated with the specified key.
    pub fn get(&self, key: &str) -> Option<&PropertyValue> {
        self.fields.get(key)
    }

    /// Retrieves a mutable reference to the property value associated with the specified key.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut PropertyValue> {
        self.fields.get_mut(key)
    }

    /// Sets a property value by key, replacing any prior value.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<PropertyValue>) {
        self.fields.insert(key.into(), value.into());
    }

    /// Removes a property value by key, returning the previous value if present.
    pub fn remove(&mut self, key: &str) -> Option<PropertyValue> {
        self.fields.remove(key)
    }

    /// Returns `true` if the property set contains the specified key.
    pub fn contains_key(&self, key: &str) -> bool {
        self.fields.contains_key(key)
    }

    /// Returns the number of properties stored.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Returns `true` if no properties are stored.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Clears all properties.
    pub fn clear(&mut self) {
        self.fields.clear();
    }

    /// Returns an iterator over all `(key, value)` property pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PropertyValue)> {
        self.fields.iter()
    }

    /// Returns an iterator over all property keys.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.fields.keys()
    }

    /// Helper to get a string property slice if present and of string type.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(PropertyValue::as_str)
    }

    /// Helper to get an integer property value if present.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(PropertyValue::as_i64)
    }

    /// Helper to get a float property value (with integer auto-coercion) if present.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(PropertyValue::as_f64)
    }

    /// Helper to get a boolean property value if present.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(PropertyValue::as_bool)
    }

    /// Navigates a dotted path (e.g. `"combat.element.fire"`) through nested objects.
    ///
    /// Defensively handles missing keys or non-object intermediate segments by returning `None`
    /// without panicking.
    pub fn get_path(&self, dot_path: &str) -> Option<&PropertyValue> {
        if dot_path.is_empty() {
            return None;
        }

        let mut parts = dot_path.split('.');
        let first = parts.next()?;
        let mut current = self.fields.get(first)?;

        for part in parts {
            match current {
                PropertyValue::Object(map) => {
                    current = map.get(part)?;
                }
                _ => return None, // Non-object intermediate segment -> graceful None
            }
        }

        Some(current)
    }

    /// Merges all properties from another `DynamicProperties`, overwriting overlapping keys.
    pub fn merge(&mut self, other: &DynamicProperties) {
        for (k, v) in &other.fields {
            self.fields.insert(k.clone(), v.clone());
        }
    }

    /// Merges top-level key-values from a `serde_json::Value::Object`.
    pub fn merge_json(&mut self, value: &serde_json::Value) {
        if let serde_json::Value::Object(map) = value {
            for (k, v) in map {
                self.fields
                    .insert(k.clone(), PropertyValue::from(v.clone()));
            }
        }
    }

    /// Deserializes a `DynamicProperties` instance from a JSON string.
    pub fn from_json_str(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    /// Serializes this `DynamicProperties` instance to a JSON string.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl From<HashMap<String, PropertyValue>> for DynamicProperties {
    fn from(fields: HashMap<String, PropertyValue>) -> Self {
        Self { fields }
    }
}

impl From<serde_json::Value> for DynamicProperties {
    fn from(value: serde_json::Value) -> Self {
        let mut props = DynamicProperties::new();
        props.merge_json(&value);
        props
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_value_scalar_coercion() {
        let int_val = PropertyValue::Int(42);
        assert_eq!(int_val.as_i64(), Some(42));
        assert_eq!(int_val.as_f64(), Some(42.0));
        assert_eq!(int_val.as_str(), None);
        assert_eq!(int_val.as_bool(), None);

        let float_val = PropertyValue::Float(123.456);
        assert_eq!(float_val.as_f64(), Some(123.456));
        assert_eq!(float_val.as_i64(), None); // Fractional -> None

        let whole_float = PropertyValue::Float(100.0);
        assert_eq!(whole_float.as_f64(), Some(100.0));
        assert_eq!(whole_float.as_i64(), Some(100)); // Whole float coerces to i64

        let bool_val = PropertyValue::Bool(true);
        assert_eq!(bool_val.as_bool(), Some(true));

        let str_val = PropertyValue::String("hello world".into());
        assert_eq!(str_val.as_str(), Some("hello world"));
    }

    #[test]
    fn test_dynamic_properties_crud_and_conversions() {
        let mut props = DynamicProperties::new();
        assert!(props.is_empty());
        assert_eq!(props.len(), 0);

        props.set("name", "Fire Sword");
        props.set("durability", 150);
        props.set("damage_multiplier", 1.25);
        props.set("is_rare", true);

        assert_eq!(props.len(), 4);
        assert!(!props.is_empty());
        assert!(props.contains_key("name"));
        assert!(!props.contains_key("missing"));

        assert_eq!(props.get_str("name"), Some("Fire Sword"));
        assert_eq!(props.get_i64("durability"), Some(150));
        assert_eq!(props.get_f64("damage_multiplier"), Some(1.25));
        assert_eq!(props.get_bool("is_rare"), Some(true));

        let removed = props.remove("is_rare");
        assert_eq!(removed, Some(PropertyValue::Bool(true)));
        assert_eq!(props.len(), 3);
        assert!(!props.contains_key("is_rare"));
    }

    #[test]
    fn test_defensive_dotted_path_navigation() {
        let mut props = DynamicProperties::new();

        let mut elemental_map = HashMap::new();
        elemental_map.insert("fire".to_string(), PropertyValue::Float(25.5));
        elemental_map.insert("frost".to_string(), PropertyValue::Float(10.0));

        let mut combat_map = HashMap::new();
        combat_map.insert(
            "elemental".to_string(),
            PropertyValue::Object(elemental_map),
        );
        combat_map.insert("base_power".to_string(), PropertyValue::Int(50));

        props.set("combat", PropertyValue::Object(combat_map));
        props.set("scalar_trap", PropertyValue::Int(999));

        // 1. Successful nested lookup
        let fire = props.get_path("combat.elemental.fire");
        assert_eq!(fire.and_then(PropertyValue::as_f64), Some(25.5));

        let base_power = props.get_path("combat.base_power");
        assert_eq!(base_power.and_then(PropertyValue::as_i64), Some(50));

        // 2. Missing deep key returns None
        assert!(props.get_path("combat.elemental.lightning").is_none());
        assert!(props.get_path("non_existent.path").is_none());
        assert!(props.get_path("").is_none());

        // 3. Intermediate non-object segment returns None gracefully without panic
        assert!(props.get_path("scalar_trap.child.key").is_none());
        assert!(props.get_path("combat.base_power.invalid").is_none());
    }

    #[test]
    fn test_json_interop_and_merging() {
        let json_data = r#"{
            "speed": 6.5,
            "armor": 25,
            "tags": ["undead", "elite"],
            "metadata": {
                "author": "designer_1"
            }
        }"#;

        let props = DynamicProperties::from_json_str(json_data).expect("Must parse valid JSON");
        assert_eq!(props.get_f64("speed"), Some(6.5));
        assert_eq!(props.get_i64("armor"), Some(25));
        assert_eq!(
            props
                .get_path("metadata.author")
                .and_then(PropertyValue::as_str),
            Some("designer_1")
        );

        let tags = props.get("tags").and_then(PropertyValue::as_array).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].as_str(), Some("undead"));
        assert_eq!(tags[1].as_str(), Some("elite"));

        // Test serialization roundtrip
        let serialized = props.to_json_string().expect("Must serialize");
        let roundtrip = DynamicProperties::from_json_str(&serialized).expect("Must roundtrip");
        assert_eq!(props, roundtrip);

        // Test merge
        let mut target = DynamicProperties::new();
        target.set("speed", 5.0);
        target.set("existing", "keep_me");
        target.merge(&props);

        assert_eq!(target.get_f64("speed"), Some(6.5)); // Overwritten
        assert_eq!(target.get_str("existing"), Some("keep_me")); // Retained
        assert_eq!(target.get_i64("armor"), Some(25)); // Added
    }
}
