//! The GUI's OpenAPI schema must include the optional monitor modes.
#![cfg(feature = "utoipa")]

use camilladsp_schema::config::{
    CompressorParameters, LookaheadLimiterProcessorParameters, NoiseGateParameters,
};
use serde_json::json;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(
    CompressorParameters,
    NoiseGateParameters,
    LookaheadLimiterProcessorParameters
)))]
struct MonitorApi;

#[test]
fn monitor_modes_are_registered_and_optional_in_processor_schemas() {
    let api = serde_json::to_value(MonitorApi::openapi()).unwrap();
    let schemas = &api["components"]["schemas"];
    assert_eq!(schemas["MonitorMode"]["type"], "string");
    assert_eq!(schemas["MonitorMode"]["enum"], json!(["Sum", "Max", "Rms"]));

    for name in [
        "CompressorParameters",
        "NoiseGateParameters",
        "LookaheadLimiterProcessorParameters",
    ] {
        let schema = &schemas[name];
        assert!(
            !schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("monitor_mode")),
            "{name}.monitor_mode must remain optional"
        );
        let alternatives = schema["properties"]["monitor_mode"]["oneOf"]
            .as_array()
            .unwrap();
        assert!(alternatives.contains(&json!({"type": "null"})));
        assert!(alternatives.contains(&json!({"$ref": "#/components/schemas/MonitorMode"})));
    }
}
