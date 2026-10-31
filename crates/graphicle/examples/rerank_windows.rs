//! Offline diagnostic, not a retrieval policy. JSON-lines in and out; the
//! model receives only question and passages, never evaluation labels.
#[cfg(feature = "local-inference")]
fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    graphicle_runtime::local_inference::exit_if_loaded(code);
    if code != 0 {
        std::process::exit(code);
    }
}

#[cfg(feature = "local-inference")]
fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use graphicle_runtime::{local_inference, model::Profile};
    use serde_json::{Value, json};
    use std::io::{BufRead, Write};
    use tokenizers::{Tokenizer, TruncationParams};
    for line in std::io::stdin().lock().lines() {
        let input: Value = serde_json::from_str(&line?)?;
        let profile: Profile = serde_json::from_value(input["profile"].clone())?;
        let directory = local_inference::directory(&profile)?;
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(directory.join("MODEL.json"))?)?;
        let limit = manifest["max_length"]
            .as_u64()
            .ok_or("missing model length")? as usize;
        let mut tokenizer = Tokenizer::from_file(directory.join("tokenizer.json"))?;
        tokenizer.with_truncation(None)?;
        tokenizer.with_padding(None);
        let mut pair = tokenizer.clone();
        pair.with_truncation(Some(TruncationParams {
            max_length: limit,
            ..Default::default()
        }))?;
        let query = input["question"].as_str().ok_or("question required")?;
        let mut results = Vec::new();
        for passage in input["passages"].as_array().ok_or("passages required")? {
            let text = passage.as_str().ok_or("text required")?;
            let encoded = tokenizer.encode(text, false)?;
            let offsets = encoded.get_offsets();
            let mut starts: Vec<usize> = (0..offsets.len()).step_by(120).collect();
            if starts.len() > 8 {
                starts = (0..8)
                    .map(|i| i * (offsets.len().saturating_sub(160)) / 7)
                    .collect();
            }
            if starts.is_empty() {
                starts.push(0);
            }
            let range = |start: usize| -> (usize, usize) {
                if offsets.is_empty() {
                    (0, text.len())
                } else {
                    (
                        offsets[start].0,
                        offsets[(start + 159).min(offsets.len() - 1)].1,
                    )
                }
            };
            let mut runs = vec![("full", 0, text.len())];
            runs.extend(starts.into_iter().map(|start| {
                let (a, b) = range(start);
                ("window", a, b)
            }));
            let mut scored = Vec::new();
            for (kind, a, b) in runs {
                let window = text.get(a..b).ok_or("invalid UTF-8 boundary")?;
                let pair_tokens = pair.encode((query, window), true)?;
                let visible: Vec<_> = pair_tokens
                    .get_offsets()
                    .iter()
                    .zip(pair_tokens.get_sequence_ids())
                    .filter_map(|(&(x, y), seq)| {
                        (seq == Some(1) && x < y).then_some((a + x, a + y))
                    })
                    .collect();
                let start = std::time::Instant::now();
                let (scores, tokens) =
                    local_inference::rerank_counted(&profile, query, &[window.to_string()])?;
                scored.push(
                    json!({"kind":kind,"byte_start":a,"byte_end":b,"score":scores[0],
                    "visible":visible,"tokens":tokens,"seconds":start.elapsed().as_secs_f64()}),
                );
            }
            results.push(scored);
        }
        println!(
            "{}",
            json!({"results":results,"max_pair_tokens":limit,"window_tokens":160,"stride":120,"max_windows":8})
        );
        std::io::stdout().flush()?;
    }
    Ok(())
}

#[cfg(not(feature = "local-inference"))]
fn main() {
    panic!("Build this diagnostic with --features local-inference");
}
