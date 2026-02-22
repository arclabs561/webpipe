//! Minimal, deterministic text normalization helpers.
//!
//! Note: this module is webpipe-local specific glue that composes the workspace `textprep` crate
//! with a more aggressive “punctuation-as-separator” policy to improve retrieval/matching.

/// Conservative “scrub” used for matching/search keys.
///
/// - Unicode normalization + lowercase + diacritics stripping (via `textprep`)
/// - treat non-alphanumeric as separators (collapse to single spaces)
pub fn scrub(s: &str) -> String {
    // Start with a robust search-key normalization. This is intentionally lossy:
    // it is used only for matching/scoring, not for display.
    let s0 = textprep_crate::scrub_with(s, &textprep_crate::ScrubConfig::search_key());

    // Then apply a strict token separator policy: anything non-alphanumeric becomes a space.
    // This keeps behavior close to the old scrub (hyphens/underscores/slashes split tokens),
    // but also fixes cases like "v1.2" or "foo.bar" which were previously merged.
    let mut out = String::with_capacity(s0.len());
    let mut last_space = true;
    for ch in s0.chars() {
        // Fold a small set of common math/ML symbols into ASCII tokens.
        //
        // Rationale: PDFs and academic text often contain Greek letters where users naturally type
        // "alpha"/"beta"/"theta"/... in queries. Folding improves query overlap without affecting
        // display text (scrub is matching-only).
        match ch {
            // Common Greek letters used in ML/math text.
            'α' | 'Α' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("alpha ");
                last_space = true;
                continue;
            }
            'β' | 'Β' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("beta ");
                last_space = true;
                continue;
            }
            'γ' | 'Γ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("gamma ");
                last_space = true;
                continue;
            }
            'δ' | 'Δ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("delta ");
                last_space = true;
                continue;
            }
            'ε' | 'ϵ' | 'Ε' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("epsilon ");
                last_space = true;
                continue;
            }
            'λ' | 'Λ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("lambda ");
                last_space = true;
                continue;
            }
            'μ' | 'µ' | 'Μ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("mu ");
                last_space = true;
                continue;
            }
            'π' | 'Π' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("pi ");
                last_space = true;
                continue;
            }
            'φ' | 'ϕ' | 'Φ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("phi ");
                last_space = true;
                continue;
            }
            'ω' | 'Ω' | 'Ω' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("omega ");
                last_space = true;
                continue;
            }
            'ρ' | 'Ρ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("rho ");
                last_space = true;
                continue;
            }
            'σ' | 'ς' | 'Σ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("sigma ");
                last_space = true;
                continue;
            }
            'θ' | 'ϑ' => {
                if !last_space {
                    out.push(' ');
                }
                out.push_str("theta ");
                last_space = true;
                continue;
            }
            _ => {}
        }
        if ch.is_alphanumeric() {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_folds_common_greek_letters_for_query_matching() {
        let s = scrub("LΩ(θ; y) = Ω∗(θ) + Ω(y) − ⟨θ, y⟩");
        assert!(
            s.contains("omega"),
            "expected omega folding; got scrub={s:?}"
        );
        assert!(
            s.contains("theta"),
            "expected theta folding; got scrub={s:?}"
        );
        // Ensure output stays lowercase and whitespace-normalized.
        assert_eq!(s, s.to_ascii_lowercase(), "expected lowercase scrub");
        assert!(
            !s.contains("  "),
            "expected scrub to not contain double spaces; got scrub={s:?}"
        );
    }

    #[test]
    fn scrub_folds_omega_and_theta_as_standalone_tokens() {
        assert_eq!(scrub("Ω"), "omega");
        assert_eq!(scrub("θ"), "theta");
        assert_eq!(scrub("Ω∗(θ)"), "omega theta");
    }

    #[test]
    fn scrub_folds_more_greek_letters_as_tokens() {
        // Keep this tight and deterministic: we only care that common Greek letters
        // used in ML/math papers become ASCII query tokens.
        let s = scrub("α β γ δ ε λ μ π φ ρ σ θ ω");
        assert_eq!(
            s,
            "alpha beta gamma delta epsilon lambda mu pi phi rho sigma theta omega"
        );
    }
}
