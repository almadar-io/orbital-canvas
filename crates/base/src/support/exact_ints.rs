//! A scene as the kernel hands it, with each number that is exactly an int written as that int (EXEC-EVAL-11:
//! `Int(n)` equals `Number(x)` exactly when `x` is `n`), so every reader takes `2.0` where it takes `2`.

use serde_json::Value;

/// `value` with every float that is exactly an int (at most 2^53, core's `exact_int`) written as a JSON integer.
pub fn exact_ints(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => match n.as_f64().and_then(orbital_core_base::value::exact_int) {
            Some(i) => Value::from(i),
            None => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(exact_ints).collect()),
        Value::Object(fields) => Value::Object(fields.into_iter().map(|(k, v)| (k, exact_ints(v))).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::exact_ints;
    use serde_json::json;

    #[test]
    fn a_float_that_is_exactly_an_int_is_written_as_that_int_at_any_depth() {
        let read = exact_ints(json!({"level": 2.0, "items": [3.0, {"x": -4.0}], "n": 0.0}));
        assert_eq!(read["level"].as_u64(), Some(2));
        assert_eq!(read["items"][0].as_u64(), Some(3));
        assert_eq!(read["items"][1]["x"].as_i64(), Some(-4));
        assert_eq!(read["n"].as_u64(), Some(0));
    }

    #[test]
    fn control_a_fraction_an_int_and_every_other_value_are_kept_as_they_are() {
        let given = json!({"x": 0.5, "w": 2.5, "i": 7, "s": "2.0", "b": true, "z": null});
        assert_eq!(exact_ints(given.clone()), given);
    }

    #[test]
    fn edge_a_float_beyond_two_to_the_53_stays_a_float() {
        let beyond = 9_007_199_254_740_994.0_f64;
        let read = exact_ints(json!({"big": beyond, "top": 9_007_199_254_740_992.0_f64}));
        assert!(read["big"].is_f64(), "beyond 2^53 a float is not exactly one int");
        assert_eq!(read["top"].as_i64(), Some(9_007_199_254_740_992));
    }
}
