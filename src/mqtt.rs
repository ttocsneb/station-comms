use color_eyre::Result;
use ordoo::or_do;
use serde::{Deserialize, Serialize};
use std::sync::mpsc;

use paho_mqtt::{Client, Message};

pub struct Mqtt {
    client: Client,
    id: String,
}

impl Mqtt {
    pub fn new(client: Client, id: String) -> Self {
        Self { client, id }
    }

    /// Start listening for requests on '/station/request/{id}'
    ///
    /// Any received requests will be sent to the `notify` channel
    pub fn listen<T>(&self, notify: mpsc::Sender<T>)
    where
        T: From<Request>,
    {
        let rx = self.client.start_consuming();
        let topic = format!("/station/request/{}", self.id);

        for msg in rx.iter() {
            if let Some(msg) = msg {
                if msg.topic() == topic {
                    let request: Request = or_do!(
                        serde_json::from_str(msg.payload_str().as_ref()),
                        e => {
                            eprintln!("could not parse request: {e}");
                            continue
                    });

                    notify.send(request.into()).unwrap();
                }
            }
        }
    }

    /// Subscribe to the requests endpoint '/station/request/{id}'
    pub fn subscribe_requests(&self) -> Result<()> {
        self.client
            .subscribe(&format!("/station/request/{}", self.id), 1)?;

        Ok(())
    }

    /// Publish a weather update.
    ///
    /// If rapid is true, then the update is sent to '/station/rapid-weather/{id}'.
    /// Otherwise the update is sent to '/station/weather/{id}'
    pub fn publish_update(&self, update: Update, rapid: bool) -> Result<()> {
        let msg = Message::new(
            format!(
                "/station/{endpoint}/{id}",
                endpoint = if rapid { "rapid-weather" } else { "weather" },
                id = self.id
            ),
            serde_json::to_string(&update)?,
            0,
        );
        Ok(self.client.publish(msg)?)
    }

    /// Publish info about the weather station to '/station/info/{id}'
    pub fn publish_info(&self, info: Info) -> Result<()> {
        let msg = Message::new(
            format!("/station/info/{id}", id = self.id),
            serde_json::to_string(&info)?,
            1,
        );
        Ok(self.client.publish(msg)?)
    }
}

#[derive(Debug, Serialize)]
pub struct SensorValue {
    pub unit: String,
    pub value: f32,
}

#[derive(Debug, Serialize)]
pub struct Update {
    pub time: String,
    pub id: String,
    pub winddir: Vec<SensorValue>,
    pub windspd: Vec<SensorValue>,
    #[serde(rename = "windgustspd-2m")]
    pub windgustspd_2m: Vec<SensorValue>,
    #[serde(rename = "windgustdir-2m")]
    pub windgustdir_2m: Vec<SensorValue>,
    #[serde(rename = "windspd-avg2m")]
    pub windspd_avg2m: Vec<SensorValue>,
    #[serde(rename = "winddir-avg2m")]
    pub winddir_avg2m: Vec<SensorValue>,
    #[serde(rename = "windspd-avg10m")]
    pub windspd_avg10m: Vec<SensorValue>,
    #[serde(rename = "winddir-avg10m")]
    pub winddir_avg10m: Vec<SensorValue>,
    #[serde(rename = "windgustspd-10m")]
    pub windgustspd_10m: Vec<SensorValue>,
    #[serde(rename = "windgustdir-10m")]
    pub windgustdir_10m: Vec<SensorValue>,
    pub humidity: Vec<SensorValue>,
    pub dewpoint: Vec<SensorValue>,
    pub temp: Vec<SensorValue>,
    #[serde(rename = "rain-1h")]
    pub rain_1h: Vec<SensorValue>,
    pub dailyrain: Vec<SensorValue>,
    pub barom: Vec<SensorValue>,
    pub uv: Vec<SensorValue>,
}

#[derive(Debug, Serialize)]
pub struct Info {
    pub make: String,
    pub model: String,
    pub software: String,
    pub version: String,
    pub latitude: f64,
    pub longitude: f64,
    pub elevation: f64,
    pub district: String,
    pub city: String,
    pub region: String,
    pub country: String,
    #[serde(rename = "rapid-weather")]
    pub rapid_weather: bool,
}

#[derive(Debug, Deserialize)]
pub struct Request {
    pub action: String,
}
