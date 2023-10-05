use clap::Parser;
use std::{
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use color_eyre::{eyre::Context, install, Result};
use mqtt::{Mqtt, Request};
use ordoo::or_do;
use paho_mqtt::{Client, ConnectOptions};
use rppal::uart::Uart;
use scode_rs::{error::ScodeError, CodeSend};
use sensor::Sensors;
use station::{
    request_all_sensors_code, request_autos_code, request_sensor_code, set_clock_code, CodeHandler,
    CommandManager,
};

use crate::{
    conf::Conf,
    mqtt::{SensorValue, Update},
    sensor::Sensor,
    station::StationReader,
};

mod conf;
mod mqtt;
mod sensor;
mod station;

fn get_updates(sensors: Arc<Mutex<Sensors>>, commands: Arc<CommandManager>) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut count = 0;
    for sensor in sensors.lock().unwrap().iter() {
        let code = request_sensor_code(sensor.id, true);
        commands.command_guarentee(code, tx.clone(), Duration::from_secs(1));
        count += 1;
    }

    for _ in 0..count {
        rx.recv()?;
    }
    Ok(())
}

enum ChannelType {
    Code(CodeSend),
    CodeErr(ScodeError),
    Request(Request),
}

impl From<CodeSend> for ChannelType {
    fn from(value: CodeSend) -> Self {
        Self::Code(value)
    }
}

impl From<ScodeError> for ChannelType {
    fn from(value: ScodeError) -> Self {
        Self::CodeErr(value)
    }
}

impl From<Request> for ChannelType {
    fn from(value: Request) -> Self {
        Self::Request(value)
    }
}

#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    #[arg(help = "Path to station.toml")]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    install()?;

    let args = Args::parse();

    let path = args.config.unwrap_or("station.toml".into());
    let conf = Conf::load(&path).with_context(|| format!("Could not open {path:?}"))?;

    let (tx, rx) = mpsc::channel::<ChannelType>();
    let (station_tx, on_send) = mpsc::channel::<CodeSend>();
    let commands = Arc::new(CommandManager::new(station_tx));

    let mut client = Client::new(conf.mqtt.host)?;
    client.connect(ConnectOptions::new_v5())?;
    if let Some(timeout) = conf.mqtt.timeout {
        client.set_timeout(Duration::from_secs_f32(timeout));
    }

    let mqtt = Arc::new(Mqtt::new(client, conf.mqtt.id.clone()));
    let r = mqtt.clone();
    let t = tx.clone();
    thread::spawn(move || r.listen(t));
    mqtt.subscribe_requests()?;

    let (update, on_update) = mpsc::channel::<bool>();

    let uart = Uart::with_path(
        conf.serial.path,
        conf.serial.baudrate,
        conf.serial.parity.into(),
        conf.serial.databits,
        conf.serial.stopbits,
    )?;
    let sensors = Arc::new(Mutex::new(Sensors::new()));

    let mut foo = StationReader::new(uart, tx, on_send);
    thread::spawn(move || foo.main());

    let mut code_handler = CodeHandler::new();
    code_handler.callback(commands.on_command());
    code_handler.callback(Sensors::sensor_callback(&sensors));
    code_handler.callback(Sensors::autos_callback(&sensors));

    let cmd = commands.clone();
    let mut rapid = false;
    let mut rapid_due = Instant::now();
    let mut rapid_update_due = Instant::now();
    let mut update_due = Instant::now();
    let mqt = mqtt.clone();
    thread::spawn(move || loop {
        let cmd_due = cmd.earliest_due();
        let mut timeout = match cmd.earliest_due() {
            Some(due) => {
                if due < update_due {
                    due
                } else {
                    update_due
                }
            }
            None => update_due,
        };
        if rapid {
            timeout = timeout.min(rapid_due).min(rapid_update_due);
        }

        let now = Instant::now();
        if timeout < now {
            if let Some(due) = cmd_due {
                if due <= now {
                    cmd.update();
                }
            }
            if update_due <= now {
                update_due = Instant::now() + Duration::from_secs(60);
                update.send(false).unwrap();
            }
            if rapid && rapid_update_due <= now {
                rapid_update_due = Instant::now() + Duration::from_millis(2500);
                update.send(true).unwrap();
            }
            if rapid && rapid_due <= now {
                rapid = false;
            }
            continue;
        }
        let cmd = or_do!(rx.recv_timeout(timeout - now), _ => {
            let now = Instant::now();
            if let Some(due) = cmd_due {
                if due <= now {
                    cmd.update();
                }
            }
            if update_due <= now {
                update_due = Instant::now() + Duration::from_secs(60);
                update.send(false).unwrap();
            }
            if rapid && rapid_update_due <= now {
                rapid_update_due = Instant::now() + Duration::from_millis(2500);
                update.send(true).unwrap();
            }
            if rapid && rapid_due <= now {
                rapid = false;
            }
            continue
        });

        match cmd {
            ChannelType::Code(code) => {
                code_handler.code(code);
            }
            ChannelType::CodeErr(err) => eprintln!("{err}"),
            ChannelType::Request(r) => match r.action.as_ref() {
                "info" => mqt
                    .publish_info(mqtt::Info {
                        make: conf.make.clone(),
                        model: conf.model.clone(),
                        software: env!("CARGO_PKG_NAME").into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        latitude: conf.latitude,
                        longitude: conf.longitude,
                        elevation: conf.elevation,
                        district: conf.district.clone(),
                        city: conf.city.clone(),
                        region: conf.region.clone(),
                        country: conf.country.clone(),
                        rapid_weather: true,
                    })
                    .unwrap(),
                "rapid-weather" => {
                    rapid = true;
                    rapid_due = Instant::now() + Duration::from_secs(60);
                    rapid_update_due = Instant::now();
                }
                _ => {}
            },
        }
    });

    let (tx, rx) = mpsc::channel();

    commands.command(set_clock_code());
    commands.command_guarentee(
        request_all_sensors_code(),
        tx.clone(),
        Duration::from_secs(1),
    );
    commands.command_guarentee(request_autos_code(), tx.clone(), Duration::from_secs(1));
    rx.recv()?;
    rx.recv()?;

    fn map_sensor(val: Option<&Sensor>) -> Vec<SensorValue> {
        val.into_iter()
            .map(|v| SensorValue {
                unit: v.unit.to_string(),
                value: v.value,
            })
            .collect()
    }

    loop {
        let rapid = on_update.recv().unwrap();
        get_updates(sensors.clone(), commands.clone())?;

        let s = sensors.lock().unwrap();

        let temp = s.get("temperature");
        let humi = s.get("humidity");

        // https://www.omnicalculator.com/physics/dew-point#how-to-calculate-dew-point-how-to-calculate-relative-humidity
        let dewp = if let Some(temp) = temp {
            if let Some(humi) = humi {
                const B: f32 = 243.04;
                const A: f32 = 17.625;
                let t = temp.value;
                let rh = humi.value / 100.0;
                let a = rh.ln() + (A * t / (B + t));
                vec![SensorValue {
                    value: (B * a) / (A - a),
                    unit: temp.unit.to_string(),
                }]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let update = Update {
            time: chrono::Local::now().to_rfc3339(),
            id: conf.mqtt.id.to_owned(),
            winddir: map_sensor(s.get("wind heading")),
            windspd: map_sensor(s.get("wind speed")),
            windgustspd_2m: map_sensor(s.get("gust 2m wind speed")),
            windgustdir_2m: map_sensor(s.get("gust 2m wind heading")),
            windspd_avg2m: map_sensor(s.get("avg 2m wind speed")),
            winddir_avg2m: map_sensor(s.get("avg 2m wind heading")),
            windspd_avg10m: map_sensor(s.get("avg 10m wind speed")),
            winddir_avg10m: map_sensor(s.get("avg 10m wind heading")),
            windgustspd_10m: map_sensor(s.get("gust 10m wind speed")),
            windgustdir_10m: map_sensor(s.get("gust 10m wind heading")),
            humidity: map_sensor(s.get("humidity")),
            temp: map_sensor(s.get("temperature")),
            rain_1h: map_sensor(s.get("rain hour")),
            dailyrain: map_sensor(s.get("rain day")),
            barom: map_sensor(s.get("pressure")),
            uv: map_sensor(s.get("uv")),
            dewpoint: dewp,
        };

        mqtt.publish_update(update, rapid)?;
    }
}
