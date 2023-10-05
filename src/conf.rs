use std::{
    fs,
    path::{Path, PathBuf},
};

use color_eyre::Result;
use serde::Deserialize;
use toml;

#[derive(Debug, Deserialize)]
pub struct MqttConf {
    pub host: String,
    pub timeout: Option<f32>,
    pub id: String,
}

const DATABITS_DEFAULT: u8 = 8;
const STOPBITS_DEFAULT: u8 = 1;

fn databits_default() -> u8 {
    DATABITS_DEFAULT
}
fn stopbits_default() -> u8 {
    STOPBITS_DEFAULT
}

#[derive(Debug, Deserialize)]
pub enum Parity {
    None,
    Even,
    Odd,
    Mark,
    Space,
}

impl From<Parity> for rppal::uart::Parity {
    fn from(value: Parity) -> Self {
        match value {
            Parity::None => Self::None,
            Parity::Even => Self::Even,
            Parity::Odd => Self::Odd,
            Parity::Mark => Self::Mark,
            Parity::Space => Self::Space,
        }
    }
}
impl Default for Parity {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Deserialize)]
pub struct SerialConf {
    pub path: PathBuf,
    pub baudrate: u32,
    #[serde(default)]
    pub parity: Parity,
    #[serde(default = "databits_default")]
    pub databits: u8,
    #[serde(default = "stopbits_default")]
    pub stopbits: u8,
}

#[derive(Debug, Deserialize)]
pub struct Conf {
    pub make: String,
    pub model: String,
    pub district: String,
    pub city: String,
    pub region: String,
    pub country: String,
    pub latitude: f64,
    pub longitude: f64,
    pub elevation: f64,
    pub mqtt: MqttConf,
    pub serial: SerialConf,
}

impl Conf {
    pub fn load(path: impl AsRef<Path>) -> Result<Conf> {
        let contents = fs::read_to_string(path)?;
        let conf: Conf = toml::from_str(&contents)?;
        Ok(conf)
    }
}
