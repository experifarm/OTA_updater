//! WebAssembly entry points for the OTA visualization.
//!
//! `run_scenario(config_json)` runs a complete, deterministic OTA scenario with the real
//! `ota_updater` engine and returns the event log as JSON (see `docs/VISUALIZATION.md` §3.2).
//! `run_lesson(name, args_json)` runs one interactive chapter lesson (see [`lessons`]).

pub mod lessons;
pub mod sim;
mod toy;

use wasm_bindgen::prelude::*;

/// Run a scenario described by `config_json` (a `ScenarioConfig`) and return the event log as JSON.
#[wasm_bindgen]
pub fn run_scenario(config_json: &str) -> Result<String, JsError> {
    let cfg: sim::ScenarioConfig = serde_json::from_str(config_json).map_err(|e| JsError::new(&e.to_string()))?;
    let log = sim::run(&cfg).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&log).map_err(|e| JsError::new(&e.to_string()))
}

/// Run the chapter lesson `name` with JSON arguments and return its result as JSON.
#[wasm_bindgen]
pub fn run_lesson(name: &str, args_json: &str) -> Result<String, JsError> {
    let args: serde_json::Value = serde_json::from_str(args_json).map_err(|e| JsError::new(&e.to_string()))?;
    let out = lessons::run(name, &args).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}

/// Engine version shown in the UI.
#[wasm_bindgen]
pub fn engine_version() -> String {
    format!("ota_viz {}", env!("CARGO_PKG_VERSION"))
}
