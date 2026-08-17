//! Converters from the engine's value types to `serde_json::Value`.

/// Convert a `glib::Value` (returned by `evaluate_javascript`) to JSON.
///
/// Supports booleans, numbers, and strings. Values of any other type are
/// reported as `Value::Null`.
pub fn value_to_json(value: &glib::Value) -> serde_json::Value {
    use serde_json::json;

    if let Ok(s) = value.get::<String>() {
        return json!(s);
    }
    if let Ok(b) = value.get::<bool>() {
        return json!(b);
    }
    if let Ok(f) = value.get::<f64>() {
        return json!(f);
    }
    if let Ok(i) = value.get::<i64>() {
        return json!(i);
    }
    if let Ok(u) = value.get::<u64>() {
        return json!(u);
    }
    if let Ok(i) = value.get::<i32>() {
        return json!(i);
    }
    if let Ok(u) = value.get::<u32>() {
        return json!(u);
    }
    json!(null)
}

/// Convert a `javascriptcore6::Value` (script message payload) to JSON.
///
/// Recursively handles null, booleans, numbers, strings, arrays and plain
/// objects. Anything else falls back to its string representation.
pub fn jsc_value_to_json(value: &javascriptcore6::Value) -> serde_json::Value {
    use serde_json::{json, Map, Value as Json};

    if value.is_null() || value.is_undefined() {
        return Json::Null;
    }
    if value.is_boolean() {
        return json!(value.to_boolean());
    }
    if value.is_number() {
        let d = value.to_double();
        // Report whole numbers without a decimal point when they fit in an i64.
        if d.fract() == 0.0 && d.abs() < 9_007_199_254_740_992.0 {
            return json!(d as i64);
        }
        return json!(d);
    }
    if value.is_string() {
        return json!(value.to_str().to_string());
    }
    if value.is_array() {
        let mut items = Vec::new();
        let mut index: u32 = 0;
        while let Some(item) = value.object_get_property_at_index(index) {
            items.push(jsc_value_to_json(&item));
            index += 1;
            if index > 100_000 {
                break;
            }
        }
        return Json::Array(items);
    }
    if value.is_object() {
        let mut object = Map::new();
        for key in value.object_enumerate_properties() {
            if let Some(prop) = value.object_get_property(&key) {
                object.insert(key.to_string(), jsc_value_to_json(&prop));
            }
        }
        return Json::Object(object);
    }

    Json::String(value.to_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsc_null_is_json_null() {
        let context = javascriptcore6::Context::default();
        let v = javascriptcore6::Value::new_null(&context);
        assert_eq!(jsc_value_to_json(&v), serde_json::Value::Null);
    }

    #[test]
    fn jsc_string_round_trips() {
        let context = javascriptcore6::Context::default();
        let v = javascriptcore6::Value::new_string(&context, Some("hello"));
        assert_eq!(
            jsc_value_to_json(&v),
            serde_json::Value::String("hello".to_string())
        );
    }

    #[test]
    fn jsc_boolean_round_trips() {
        let context = javascriptcore6::Context::default();
        let v = javascriptcore6::Value::new_boolean(&context, true);
        assert_eq!(jsc_value_to_json(&v), serde_json::Value::Bool(true));
    }

    #[test]
    fn jsc_number_round_trips() {
        let context = javascriptcore6::Context::default();
        let v = javascriptcore6::Value::new_number(&context, 42.0);
        assert_eq!(jsc_value_to_json(&v), serde_json::Value::from(42_i64));
    }
}