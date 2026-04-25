//! Optional schemars-based scaffold generator for `Config`.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p crab-config --example gen-schema --features gen-schema
//! ```
//!
//! The output is a *scaffold* — a starting point for hand-edits to
//! `assets/config.schema.json`. It is intentionally NOT committed and not
//! consumed by CI (`docs/config.md` §12.1). The hand-written schema is the
//! source of truth; this tool exists only to bootstrap the next big
//! restructure when adding many fields at once would be tedious.
//!
//! The `gen-schema` feature is gated so production binaries never link
//! schemars.

#[cfg(feature = "gen-schema")]
fn main() {
    let schema = schemars::schema_for!(crab_config::Config);
    let json =
        serde_json::to_string_pretty(&schema).expect("schemars output should serialize as JSON");
    println!("{json}");
}

#[cfg(not(feature = "gen-schema"))]
fn main() {
    eprintln!(
        "this example requires the `gen-schema` feature: \
         cargo run -p crab-config --example gen-schema --features gen-schema"
    );
    std::process::exit(2);
}
