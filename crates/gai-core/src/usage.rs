use serde::{Deserialize, Serialize};
use std::ops::AddAssign;

/// Token usage for one or more model calls.
///
/// `input_tokens` counts only uncached input; cache reads and writes are kept
/// apart because they are priced differently. Dialects whose providers report
/// cached tokens inside the prompt total subtract them first.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Reasoning tokens, when reported separately. Already included in
    /// `output_tokens`.
    #[serde(default)]
    pub reasoning_tokens: u64,
}

impl Usage {
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens
    }

    pub fn total(&self) -> u64 {
        self.total_input() + self.output_tokens
    }

    /// Field-wise maximum. Streaming providers report cumulative snapshots,
    /// so merging snapshots this way yields the final figures.
    pub fn merge_max(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.max(other.cache_read_tokens);
        self.cache_write_tokens = self.cache_write_tokens.max(other.cache_write_tokens);
        self.reasoning_tokens = self.reasoning_tokens.max(other.reasoning_tokens);
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, o: Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cache_write_tokens += o.cache_write_tokens;
        self.reasoning_tokens += o.reasoning_tokens;
    }
}

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    /// Defaults to 10% of `input` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// Defaults to 125% of `input` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

impl Pricing {
    pub const fn new(input: f64, output: f64) -> Self {
        Self {
            input,
            output,
            cache_read: None,
            cache_write: None,
        }
    }

    pub const fn with_cache_read(mut self, price: f64) -> Self {
        self.cache_read = Some(price);
        self
    }

    pub fn cost(&self, u: &Usage) -> f64 {
        let per = |tokens: u64, price: f64| tokens as f64 * price / 1_000_000.0;
        per(u.input_tokens, self.input)
            + per(u.output_tokens, self.output)
            + per(
                u.cache_read_tokens,
                self.cache_read.unwrap_or(self.input * 0.1),
            )
            + per(
                u.cache_write_tokens,
                self.cache_write.unwrap_or(self.input * 1.25),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_prices_each_bucket() {
        let p = Pricing::new(4.0, 20.0).with_cache_read(0.2);
        let u = Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        };
        assert!((p.cost(&u) - (4.0 + 10.0 + 0.2)).abs() < 1e-9);
    }

    #[test]
    fn merge_max_keeps_latest_snapshot() {
        let mut a = Usage {
            input_tokens: 10,
            output_tokens: 1,
            ..Default::default()
        };
        a.merge_max(&Usage {
            output_tokens: 7,
            ..Default::default()
        });
        assert_eq!((a.input_tokens, a.output_tokens), (10, 7));
    }
}
