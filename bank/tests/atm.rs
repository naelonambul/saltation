//! ATM contract proof, independent of the database and runnable in the Oracle harness.

use serde_json::Value;

#[test]
fn contracts_are_valid_openapi() {
    let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".to_owned());
    for name in ["atm", "internal"] {
        let text =
            std::fs::read_to_string(format!("{dir}/openapi/{name}.yaml")).expect("read contract");
        let doc: Value = serde_yaml::from_str(&text).expect("contract YAML");
        assert!(
            doc["openapi"]
                .as_str()
                .expect("OpenAPI version")
                .starts_with("3.")
        );
        assert_refs_resolve(&doc, &doc);
        for path in doc["paths"].as_object().expect("paths").values() {
            for method in [
                "get", "post", "put", "patch", "delete", "options", "head", "trace",
            ] {
                if let Some(operation) = path.get(method) {
                    assert!(
                        !operation["responses"]
                            .as_object()
                            .expect("responses")
                            .is_empty()
                    );
                }
            }
        }
        let defs = doc["components"]["schemas"]
            .to_string()
            .replace("#/components/schemas/", "#/$defs/");
        let defs: Value = serde_json::from_str(&defs).expect("defs");
        for schema_name in defs.as_object().expect("schemas").keys() {
            let schema = serde_json::json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$defs": defs,
                "$ref": format!("#/$defs/{schema_name}")
            });
            jsonschema::draft202012::new(&schema).expect("valid component schema");
        }
    }
}

fn assert_refs_resolve(value: &Value, doc: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref") {
                let reference = reference.as_str().expect("string reference");
                let pointer = reference.strip_prefix('#').expect("local reference");
                assert!(doc.pointer(pointer).is_some(), "unresolved {reference}");
            }
            for child in object.values() {
                assert_refs_resolve(child, doc);
            }
        }
        Value::Array(array) => {
            for child in array {
                assert_refs_resolve(child, doc);
            }
        }
        _ => {}
    }
}
