use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Configuration {
    pub scheme: SmolStr,
    pub domain: SmolStr,
}