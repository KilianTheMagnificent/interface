//! Provider-reported usage only. Any incomplete step makes that aggregate
//! unknown; an absent cost must never be presented as a free model call.
use serde_json::Value;

pub use interface_engine::worker::WorkerUsage;

#[derive(Default)]
pub struct WorkerUsageAccumulator {
    total: WorkerUsage,
    seen: bool,
}

fn sum(previous: Option<u64>, value: &Value, seen: bool) -> Option<u64> {
    let value = value.as_u64()?;
    if seen {
        previous?.checked_add(value)
    } else {
        Some(value)
    }
}

impl WorkerUsageAccumulator {
    pub fn observe_step(&mut self, part: &Value) {
        let tokens = &part["tokens"];
        self.total.input_tokens = sum(self.total.input_tokens, &tokens["input"], self.seen);
        self.total.output_tokens = sum(self.total.output_tokens, &tokens["output"], self.seen);
        self.total.reasoning_tokens =
            sum(self.total.reasoning_tokens, &tokens["reasoning"], self.seen);
        self.total.cache_read_tokens = sum(
            self.total.cache_read_tokens,
            &tokens["cache"]["read"],
            self.seen,
        );
        self.total.cache_write_tokens = sum(
            self.total.cache_write_tokens,
            &tokens["cache"]["write"],
            self.seen,
        );
        self.total.cost_usd = part["cost"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .and_then(|cost| {
                if self.seen {
                    self.total.cost_usd.map(|total| total + cost)
                } else {
                    Some(cost)
                }
            })
            .filter(|v| v.is_finite());
        self.seen = true;
    }

    #[must_use]
    pub fn snapshot(&self) -> WorkerUsage {
        self.total.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn explicit_zero_is_known_missing_or_invalid_is_sticky_unknown() {
        let mut usage = WorkerUsageAccumulator::default();
        assert_eq!(usage.snapshot().cost_usd, None);
        usage.observe_step(&json!({"cost":0,"tokens":{"input":0,"output":2}}));
        assert_eq!(usage.snapshot().cost_usd, Some(0.0));
        assert_eq!(usage.snapshot().input_tokens, Some(0));
        usage.observe_step(&json!({"tokens":{"input":4,"output":1.5}}));
        assert_eq!(usage.snapshot().input_tokens, Some(4));
        assert_eq!(usage.snapshot().output_tokens, None);
        usage.observe_step(&json!({"cost":0.5,"tokens":{"input":-1,"output":2}}));
        assert_eq!(usage.snapshot().input_tokens, None);
        assert_eq!(usage.snapshot().output_tokens, None);
        assert_eq!(usage.snapshot().cost_usd, None);
    }
    #[test]
    fn adds_complete_steps_and_refuses_overflow() {
        let mut usage = WorkerUsageAccumulator::default();
        for _ in 0..2 {
            usage.observe_step(&json!({"cost":0.25,"tokens":{"input":2,"output":3,"reasoning":1,"cache":{"read":5,"write":0}}}));
        }
        assert_eq!(
            usage.snapshot(),
            WorkerUsage {
                input_tokens: Some(4),
                output_tokens: Some(6),
                reasoning_tokens: Some(2),
                cache_read_tokens: Some(10),
                cache_write_tokens: Some(0),
                cost_usd: Some(0.5)
            }
        );
        usage.observe_step(&json!({"tokens":{"input":u64::MAX}}));
        usage.observe_step(&json!({"tokens":{"input":0}}));
        assert_eq!(usage.snapshot().input_tokens, None);
    }
}
