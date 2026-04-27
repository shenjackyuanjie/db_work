pub(super) fn softmax(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp_values: Vec<f32> = logits.iter().map(|value| (value - max).exp()).collect();
    let sum: f32 = exp_values.iter().sum();
    if sum == 0.0 {
        return vec![0.0; logits.len()];
    }
    exp_values.into_iter().map(|value| value / sum).collect()
}

pub(super) fn argmax_with_confidence(probabilities: &[f32]) -> (usize, f64) {
    let (index, confidence) =
        probabilities
            .iter()
            .copied()
            .enumerate()
            .fold((0usize, 0.0f32), |acc, (index, value)| {
                if value > acc.1 { (index, value) } else { acc }
            });
    (index, confidence as f64)
}
