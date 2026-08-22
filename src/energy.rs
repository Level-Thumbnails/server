use crate::db::{NoteData, Rating};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnergyConfig {
    pub(crate) enabled: bool,                     // whether the energy system is enabled
    pub(crate) max_millipoints: i32,              // maximum millipoints a user can have
    pub(crate) refill_rate: i32,                  // millipoints per hour
    pub(crate) base_cost: i32,                    // base cost in millipoints
    pub(crate) min_cost: i32,                     // lower bound for cost
    pub(crate) download_weight: f32,              // multiplier for download count in popularity score
    pub(crate) popularity_tiers: Vec<(i32, f32)>, // tiers based on likes/download count (presorted by descending threshold)
    pub(crate) rated_weight: f32,                 // multiplier for rated levels
    pub(crate) creator_mult: f32,                 // multiplier for creator submissions
    pub(crate) creator_min_downloads: i32,        // minimum downloads for creator multiplier to apply
}

impl Default for EnergyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_millipoints: 5000,
            refill_rate: 200,
            base_cost: 1200,
            min_cost: 240,
            download_weight: 0.05,
            popularity_tiers: vec![
                (100_000, 0.2),
                (10_000, 0.4),
                (1000, 0.6),
                (100, 0.8),
                (20, 1.0),
                (0, 2.0),
            ],
            rated_weight: 0.4,
            creator_mult: 0.7,
            creator_min_downloads: 100,
        }
    }
}

pub fn calculate_submission_cost(note: &NoteData, is_creator: bool, config: &EnergyConfig) -> i32 {
    let popularity_mult = popularity_mult(note.likes, note.downloads, config);

    let rating_mult = if matches!(note.rating, Rating::NA) {
        1.0
    } else {
        config.rated_weight
    };

    let creator_mult = if is_creator && note.downloads >= config.creator_min_downloads as i64 {
        config.creator_mult
    } else {
        1.0
    };

    let mult = popularity_mult * rating_mult * creator_mult;
    ((config.base_cost as f32 * mult).round() as i32).max(config.min_cost)
}

fn popularity_mult(likes: i64, downloads: i64, config: &EnergyConfig) -> f32 {
    let score = likes.abs().saturating_add((downloads as f32 * config.download_weight) as i64);

    for (threshold, multiplier) in &config.popularity_tiers {
        if score >= *threshold as i64 {
            return *multiplier;
        }
    }

    1.0
}
