use velyr_engine_api::{Isolate, JsException, Source};

/// Bootstrap script (fragments share one private scope, see `00_head.js` / `90_exports.js`).
pub const CRYPTO_JS: &str = concat!(
    include_str!("js/00_head.js"),
    include_str!("js/10_random.js"),
    include_str!("js/20_digest.js"),
    include_str!("js/30_keys.js"),
    include_str!("js/40_secret.js"),
    include_str!("js/50_asym.js"),
    include_str!("js/60_subtle.js"),
    include_str!("js/70_derive.js"),
    include_str!("js/80_rsa.js"),
    include_str!("js/90_exports.js"),
);

/// Runs [`CRYPTO_JS`]. Call after `install_op_context` and `install_events` (it throws `DOMException`).
pub fn install_web_crypto(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate
        .eval_script(&Source {
            name: "velyr:crypto".into(),
            text: CRYPTO_JS.into(),
        })
        .map(drop)
}
