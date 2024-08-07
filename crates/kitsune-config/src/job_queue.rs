use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::num::NonZero;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Configuration {
    pub redis_url: SmolStr,
    pub num_workers: NonZero<usize>,
}
