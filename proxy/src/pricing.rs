//! Optional cost estimation. rtk ships **no prices**: the model registry
//! carries families and cache-rate ratios only, and every dollar figure in the
//! analytics exists only when the operator has added their own prices to
//! `models.toml` (via `RTK_PROXY_MODELS_TOML` or a registry edit). With no
//! prices configured, cost functions return `None` and the dashboards fall
//! back to token-based metrics — rtk never invents a price.

/// What a cached-prefix read costs relative to the input price (registry-driven,
/// per model/provider). Anthropic 0.1x, OpenAI 0.5x, Google ~0.25x. A ratio,
/// not a price — always available.
pub fn cache_read_mult_of(model: &str) -> f64 {
    crate::models::cache_read_mult(model)
}

/// Cache-write multiplier relative to the model's input price (Anthropic
/// bills prefix creation at 1.25x). The READ multiplier is per-provider and
/// comes from the model registry via [`cache_read_mult_of`].
pub const CACHE_WRITE_MULT: f64 = 1.25;

/// Estimated USD cost of a request, accounting for prompt caching.
/// `input_tokens` are fresh (uncached) input tokens; cache writes (creation)
/// bill at 1.25x and cache reads at the registry's per-provider rate.
/// `None` when the operator has not configured a price for the model.
pub fn cost_usd_cached(
    model: &str,
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    output_tokens: i64,
) -> Option<f64> {
    let (pin, pout) = crate::models::price(model)?;
    let inp = input_tokens.max(0) as f64;
    let create = cache_creation_tokens.max(0) as f64;
    let read = cache_read_tokens.max(0) as f64;
    let out = output_tokens.max(0) as f64;
    Some(
        (inp * pin + create * pin * CACHE_WRITE_MULT + read * pin * cache_read_mult_of(model))
            / 1_000_000.0
            + out * pout / 1_000_000.0,
    )
}

/// Saved tokens priced as a scenario band at today's prices: every saved token
/// at the cheapest of a cache read, fresh input and a cache write at the low
/// end, at the dearest at the high end. A band, not a measurement: which of
/// those a removed token would have been is not observable.
#[derive(Debug, Default, PartialEq)]
pub struct ModelledSavings {
    pub usd_low: f64,
    pub usd_high: f64,
    /// Saved tokens on models with a price, which the band covers.
    pub priced_tokens: i64,
    /// Saved tokens on models with no price, which no dollar figure covers.
    pub unpriced_tokens: i64,
}

/// Prices each `(model, saved tokens)` row with `rate`: a model's input price
/// per million tokens and its cache-read multiplier, `None` when unpriced.
pub fn modelled_savings(
    rows: impl IntoIterator<Item = (String, i64)>,
    rate: impl Fn(&str) -> Option<(f64, f64)>,
) -> ModelledSavings {
    let mut m = ModelledSavings::default();
    for (model, saved) in rows {
        let saved = saved.max(0);
        match rate(&model) {
            Some((input_price, read_mult)) => {
                let fresh = saved as f64 * input_price / 1_000_000.0;
                let mults = [read_mult, 1.0, CACHE_WRITE_MULT];
                m.usd_low += fresh * mults.into_iter().fold(f64::INFINITY, f64::min);
                m.usd_high += fresh * mults.into_iter().fold(f64::NEG_INFINITY, f64::max);
                m.priced_tokens += saved;
            }
            None => m.unpriced_tokens += saved,
        }
    }
    m
}

/// The operator's input price for `model` and its cache-read multiplier.
pub fn input_rate(model: &str) -> Option<(f64, f64)> {
    let (input_price, _) = crate::models::price(model)?;
    Some((input_price, cache_read_mult_of(model)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modelled_savings_band_runs_from_cache_read_to_cache_write() {
        let rate = |model: &str| match model {
            "priced" => Some((15.0, 0.1)),
            _ => None,
        };
        let m = modelled_savings(
            [
                ("priced".to_string(), 1_000_000),
                ("unpriced".to_string(), 400),
                ("priced".to_string(), -5),
            ],
            rate,
        );
        assert!((m.usd_low - 1.5).abs() < 1e-9, "{m:?}");
        assert!((m.usd_high - 18.75).abs() < 1e-9, "{m:?}");
        assert_eq!((m.priced_tokens, m.unpriced_tokens), (1_000_000, 400));
        // An operator's cache-read multiplier above the write multiplier
        // still gives low <= high, and fresh input stays inside the band.
        let m = modelled_savings([("dear".to_string(), 1_000_000)], |_| Some((15.0, 2.0)));
        assert!((m.usd_low - 15.0).abs() < 1e-9, "{m:?}");
        assert!((m.usd_high - 30.0).abs() < 1e-9, "{m:?}");
        assert_eq!(
            modelled_savings(Vec::new(), rate),
            ModelledSavings::default()
        );
    }

    const PRICED: &str = r#"
[[model]]
pattern = "opus"
family = "anthropic"
input = 15.0
output = 75.0
cache_read_mult = 0.1

[default]
family = "other"
cache_read_mult = 0.1
"#;

    #[test]
    fn costs_exist_only_when_the_operator_supplies_prices() {
        crate::models::install(
            crate::models::Registry::parse(crate::models::DEFAULT_TOML).unwrap(),
        );
        assert_eq!(cost_usd_cached("claude-opus-4-8", 1000, 0, 0, 500), None);
        assert_eq!(cost_usd_cached("totally-unknown", 1000, 0, 0, 500), None);

        crate::models::install(crate::models::Registry::parse(PRICED).unwrap());
        // 1000 fresh input @15, 2000 cache_creation @15*1.25, 4000 cache_read
        // @15*0.1, 500 output @75.
        let c = cost_usd_cached("claude-opus-4-8", 1000, 2000, 4000, 500).expect("priced");
        let expected = (1000.0 * 15.0 + 2000.0 * 15.0 * 1.25 + 4000.0 * 15.0 * 0.10) / 1e6
            + 500.0 * 75.0 / 1e6;
        assert!((c - expected).abs() < 1e-9, "got {c}, want {expected}");
        // A model outside the operator's table still has no price.
        assert_eq!(cost_usd_cached("gpt-4o-mini", 1000, 0, 0, 0), None);
        crate::models::install(
            crate::models::Registry::parse(crate::models::DEFAULT_TOML).unwrap(),
        );
    }
}
