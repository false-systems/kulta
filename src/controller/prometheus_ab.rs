//! Statistical analysis for A/B testing experiments
//!
//! Implements Z-test for proportions to determine statistical significance
//! between variant A (control) and variant B (experiment).

use crate::crd::rollout::{ABConclusionReason, ABMetricDirection, ABMetricResult, ABVariant};

/// Result of statistical comparison between variants
#[derive(Debug, Clone)]
pub struct ABComparisonResult {
    /// Is the difference statistically significant?
    pub is_significant: bool,
    /// Confidence level achieved (0.0 to 1.0)
    pub confidence: f64,
    /// Which variant performed better, or None if no significant difference
    pub winner: Option<ABVariant>,
    /// Effect size (relative difference)
    pub effect_size: f64,
    /// Sample size for variant A
    pub sample_size_a: i64,
    /// Sample size for variant B
    pub sample_size_b: i64,
}

/// Calculate statistical significance using Z-test for proportions
///
/// This is the simplest viable approach that's still statistically sound.
/// Used for metrics like error rate and conversion rate.
///
/// # Arguments
/// * `rate_a` - Rate for variant A (e.g., 0.02 for 2% error rate)
/// * `rate_b` - Rate for variant B
/// * `n_a` - Sample size for variant A
/// * `n_b` - Sample size for variant B
/// * `confidence_level` - Required confidence (e.g., 0.95)
/// * `direction` - Expected direction of improvement
///
/// # Returns
/// ABComparisonResult with significance determination
pub fn calculate_ab_significance(
    rate_a: f64,
    rate_b: f64,
    n_a: i64,
    n_b: i64,
    confidence_level: f64,
    direction: &ABMetricDirection,
) -> ABComparisonResult {
    // Minimum sample size check (need at least 30 for CLT)
    if n_a < 30 || n_b < 30 {
        return ABComparisonResult {
            is_significant: false,
            confidence: 0.0,
            winner: None,
            effect_size: 0.0,
            sample_size_a: n_a,
            sample_size_b: n_b,
        };
    }

    // Pooled proportion
    let p_pooled = (rate_a * n_a as f64 + rate_b * n_b as f64) / (n_a + n_b) as f64;

    // Standard error using pooled proportion
    let se = (p_pooled * (1.0 - p_pooled) * (1.0 / n_a as f64 + 1.0 / n_b as f64)).sqrt();

    // Avoid division by zero or NaN
    if se == 0.0 || se.is_nan() || se.is_infinite() {
        return ABComparisonResult {
            is_significant: false,
            confidence: 0.0,
            winner: None,
            effect_size: 0.0,
            sample_size_a: n_a,
            sample_size_b: n_b,
        };
    }

    // Z-score (difference between variants normalized by standard error)
    let z_score = (rate_b - rate_a) / se;

    // Convert to confidence using normal distribution CDF
    // P-value = 2 * (1 - CDF(|z|)) for two-tailed test
    let p_value = 2.0 * (1.0 - normal_cdf(z_score.abs()));
    let achieved_confidence = 1.0 - p_value;

    // Effect size (relative difference)
    let effect_size = if rate_a > 0.0 {
        (rate_b - rate_a) / rate_a
    } else if rate_b > 0.0 {
        1.0 // B is better when A is 0
    } else {
        0.0 // Both are 0
    };

    // Determine winner based on direction and significance
    let winner = if achieved_confidence >= confidence_level {
        match direction {
            ABMetricDirection::Lower => {
                // Lower is better (e.g., error rate, latency)
                if rate_b < rate_a {
                    Some(ABVariant::B)
                } else {
                    Some(ABVariant::A)
                }
            }
            ABMetricDirection::Higher => {
                // Higher is better (e.g., conversion rate)
                if rate_b > rate_a {
                    Some(ABVariant::B)
                } else {
                    Some(ABVariant::A)
                }
            }
        }
    } else {
        None
    };

    ABComparisonResult {
        is_significant: achieved_confidence >= confidence_level,
        confidence: achieved_confidence,
        winner,
        effect_size,
        sample_size_a: n_a,
        sample_size_b: n_b,
    }
}

/// Evaluate all A/B metrics and return results
///
/// # Arguments
/// * `metrics` - List of metrics to evaluate with their values and directions
/// * `confidence_level` - Required confidence level (default 0.95)
///
/// # Returns
/// Vec of ABMetricResult for each metric
pub fn evaluate_ab_metrics(
    metrics: &[(String, f64, f64, i64, i64, ABMetricDirection)],
    confidence_level: f64,
) -> Vec<ABMetricResult> {
    metrics
        .iter()
        .map(|(name, rate_a, rate_b, n_a, n_b, direction)| {
            let result = calculate_ab_significance(
                *rate_a,
                *rate_b,
                *n_a,
                *n_b,
                confidence_level,
                direction,
            );
            ABMetricResult {
                name: name.clone(),
                value_a: *rate_a,
                value_b: *rate_b,
                confidence: result.confidence,
                is_significant: result.is_significant,
                winner: result.winner,
            }
        })
        .collect()
}

/// Determine overall experiment conclusion from metric results
///
/// # Returns
/// * `Some((winner, reason))` if experiment should conclude
/// * `None` if experiment should continue
pub fn determine_experiment_conclusion(
    results: &[ABMetricResult],
) -> Option<(ABVariant, ABConclusionReason)> {
    if results.is_empty() {
        return None;
    }

    // Check if all significant metrics agree on a winner
    let significant_results: Vec<&ABMetricResult> =
        results.iter().filter(|r| r.is_significant).collect();

    if significant_results.is_empty() {
        return None;
    }

    // Count winners
    let mut a_wins = 0;
    let mut b_wins = 0;

    for result in &significant_results {
        match &result.winner {
            Some(ABVariant::A) => a_wins += 1,
            Some(ABVariant::B) => b_wins += 1,
            None => {}
        }
    }

    // All significant metrics agree
    if a_wins > 0 && b_wins == 0 {
        Some((ABVariant::A, ABConclusionReason::ConsensusReached))
    } else if b_wins > 0 && a_wins == 0 {
        Some((ABVariant::B, ABConclusionReason::ConsensusReached))
    } else if a_wins == significant_results.len() || b_wins == significant_results.len() {
        let winner = if a_wins > b_wins {
            ABVariant::A
        } else {
            ABVariant::B
        };
        Some((winner, ABConclusionReason::SignificanceReached))
    } else {
        None
    }
}

/// Normal CDF approximation using Abramowitz and Stegun formula
///
/// Approximates the cumulative distribution function of the standard normal distribution.
/// Accuracy: |error| < 7.5e-8
fn normal_cdf(x: f64) -> f64 {
    // Constants for the approximation
    const A1: f64 = 0.254829592;
    const A2: f64 = -0.284496736;
    const A3: f64 = 1.421413741;
    const A4: f64 = -1.453152027;
    const A5: f64 = 1.061405429;
    const P: f64 = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs() / std::f64::consts::SQRT_2;

    // Horner's method for polynomial evaluation
    let t = 1.0 / (1.0 + P * x);
    let y = 1.0 - (((((A5 * t + A4) * t) + A3) * t + A2) * t + A1) * t * (-x * x).exp();

    0.5 * (1.0 + sign * y)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn test_normal_cdf_at_zero() {
        // CDF at 0 should be 0.5
        let result = normal_cdf(0.0);
        assert!((result - 0.5).abs() < 0.0001);
    }

    #[test]
    fn test_normal_cdf_at_positive_infinity() {
        // CDF approaches 1 for large positive values
        let result = normal_cdf(5.0);
        assert!(result > 0.999);
    }

    #[test]
    fn test_normal_cdf_at_negative_infinity() {
        // CDF approaches 0 for large negative values
        let result = normal_cdf(-5.0);
        assert!(result < 0.001);
    }

    #[test]
    fn test_normal_cdf_symmetry() {
        // CDF(-x) = 1 - CDF(x)
        let x = 1.96;
        let cdf_positive = normal_cdf(x);
        let cdf_negative = normal_cdf(-x);
        assert!((cdf_positive + cdf_negative - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_normal_cdf_at_1_96() {
        // CDF at 1.96 should be ~0.975 (95% confidence one-tailed)
        let result = normal_cdf(1.96);
        assert!((result - 0.975).abs() < 0.001);
    }

    #[test]
    fn test_calculate_ab_significance_clear_winner() {
        // Variant B has clearly lower error rate
        let result = calculate_ab_significance(
            0.05, // A: 5% error
            0.02, // B: 2% error
            10000,
            10000,
            0.95,
            &ABMetricDirection::Lower,
        );

        assert!(result.is_significant);
        assert!(result.confidence > 0.95);
        assert_eq!(result.winner, Some(ABVariant::B));
        assert!(result.effect_size < 0.0); // B is lower, so negative effect
    }

    #[test]
    fn test_calculate_ab_significance_no_difference() {
        // Same rates - no significant difference
        let result = calculate_ab_significance(
            0.05, // A: 5% error
            0.05, // B: 5% error
            10000,
            10000,
            0.95,
            &ABMetricDirection::Lower,
        );

        assert!(!result.is_significant);
        assert!(result.winner.is_none());
        assert!((result.effect_size - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_calculate_ab_significance_insufficient_samples() {
        // Too few samples
        let result = calculate_ab_significance(
            0.05,
            0.02,
            20, // Less than 30
            20,
            0.95,
            &ABMetricDirection::Lower,
        );

        assert!(!result.is_significant);
        assert_eq!(result.confidence, 0.0);
        assert!(result.winner.is_none());
    }

    #[test]
    fn test_calculate_ab_significance_higher_is_better() {
        // Testing conversion rate where higher is better
        let result = calculate_ab_significance(
            0.10, // A: 10% conversion
            0.15, // B: 15% conversion
            10000,
            10000,
            0.95,
            &ABMetricDirection::Higher,
        );

        assert!(result.is_significant);
        assert_eq!(result.winner, Some(ABVariant::B));
        assert!(result.effect_size > 0.0); // B is higher
    }

    #[test]
    fn test_calculate_ab_significance_a_wins_when_lower_expected() {
        // A has lower error rate when lower is better
        let result = calculate_ab_significance(
            0.01, // A: 1% error
            0.05, // B: 5% error
            10000,
            10000,
            0.95,
            &ABMetricDirection::Lower,
        );

        assert!(result.is_significant);
        assert_eq!(result.winner, Some(ABVariant::A));
    }

    #[test]
    fn test_evaluate_ab_metrics_multiple() {
        let metrics = vec![
            (
                "error-rate".to_string(),
                0.05,
                0.02,
                10000i64,
                10000i64,
                ABMetricDirection::Lower,
            ),
            (
                "latency-p95".to_string(),
                0.200,
                0.150,
                10000i64,
                10000i64,
                ABMetricDirection::Lower,
            ),
        ];

        let results = evaluate_ab_metrics(&metrics, 0.95);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "error-rate");
        assert_eq!(results[1].name, "latency-p95");
    }

    #[test]
    fn test_determine_experiment_conclusion_consensus_b() {
        let results = vec![
            ABMetricResult {
                name: "error-rate".to_string(),
                value_a: 0.05,
                value_b: 0.02,
                confidence: 0.98,
                is_significant: true,
                winner: Some(ABVariant::B),
            },
            ABMetricResult {
                name: "latency".to_string(),
                value_a: 0.2,
                value_b: 0.15,
                confidence: 0.97,
                is_significant: true,
                winner: Some(ABVariant::B),
            },
        ];

        let conclusion = determine_experiment_conclusion(&results);
        assert!(conclusion.is_some());
        let (winner, reason) = conclusion.unwrap();
        assert_eq!(winner, ABVariant::B);
        assert_eq!(reason, ABConclusionReason::ConsensusReached);
    }

    #[test]
    fn test_determine_experiment_conclusion_no_significant_results() {
        let results = vec![ABMetricResult {
            name: "error-rate".to_string(),
            value_a: 0.05,
            value_b: 0.048,
            confidence: 0.60,
            is_significant: false,
            winner: None,
        }];

        let conclusion = determine_experiment_conclusion(&results);
        assert!(conclusion.is_none());
    }

    #[test]
    fn test_determine_experiment_conclusion_mixed_results() {
        let results = vec![
            ABMetricResult {
                name: "error-rate".to_string(),
                value_a: 0.05,
                value_b: 0.02,
                confidence: 0.98,
                is_significant: true,
                winner: Some(ABVariant::B),
            },
            ABMetricResult {
                name: "latency".to_string(),
                value_a: 0.15,
                value_b: 0.20,
                confidence: 0.97,
                is_significant: true,
                winner: Some(ABVariant::A), // Conflicting!
            },
        ];

        // Mixed results - should not conclude
        let conclusion = determine_experiment_conclusion(&results);
        assert!(conclusion.is_none());
    }

    #[test]
    fn test_effect_size_calculation() {
        // 50% reduction in error rate
        let result = calculate_ab_significance(
            0.10, // A: 10% error
            0.05, // B: 5% error
            10000,
            10000,
            0.95,
            &ABMetricDirection::Lower,
        );

        // Effect size should be -0.5 (50% reduction)
        assert!((result.effect_size - (-0.5)).abs() < 0.01);
    }
}
